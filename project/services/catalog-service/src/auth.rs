use std::{env, fs, io, sync::Arc};

use axum::{
    extract::FromRequestParts,
    http::{header, request::Parts, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::Deserialize;

use crate::AppState;

#[derive(Clone)]
pub(crate) struct AccessTokens(Arc<Verifier>);

struct Verifier {
    key: DecodingKey,
    validation: Validation,
}

#[derive(Deserialize)]
struct Claims {
    iss: String,
    sub: String,
}

impl AccessTokens {
    pub(crate) fn from_env() -> Result<Option<Self>, Box<dyn std::error::Error>> {
        let issuer = env::var("OIDC_ISSUER").ok();
        let audience = env::var("OIDC_API_AUDIENCE").ok();
        let key_file = env::var_os("OIDC_PUBLIC_KEY_FILE");

        match (issuer, audience, key_file) {
            (None, None, None) => Ok(None),
            (Some(issuer), Some(audience), Some(key_file))
                if !issuer.is_empty() && !audience.is_empty() =>
            {
                let key = DecodingKey::from_rsa_pem(&fs::read(key_file)?)?;
                let mut validation = Validation::new(Algorithm::RS256);
                validation.set_issuer(&[issuer]);
                validation.set_audience(&[audience]);
                validation.set_required_spec_claims(&["iss", "aud", "exp", "sub"]);
                validation.validate_nbf = true;
                validation.leeway = 0;
                Ok(Some(Self(Arc::new(Verifier { key, validation }))))
            }
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "set OIDC_ISSUER, OIDC_API_AUDIENCE and OIDC_PUBLIC_KEY_FILE together",
            )
            .into()),
        }
    }

    fn customer(&self, token: &str) -> Option<Customer> {
        let header = decode_header(token).ok()?;
        if !matches!(header.typ.as_deref(), Some("at+jwt" | "application/at+jwt")) {
            return None;
        }
        let claims = decode::<Claims>(token, &self.0.key, &self.0.validation)
            .ok()?
            .claims;
        if claims.sub.is_empty() || claims.sub.len() > 255 || claims.sub.contains('\0') {
            return None;
        }
        Some(Customer {
            issuer: claims.iss,
            subject: claims.sub,
        })
    }
}

pub(crate) struct Customer {
    pub(crate) issuer: String,
    pub(crate) subject: String,
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"))],
    )
        .into_response()
}

impl FromRequestParts<AppState> for Customer {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let verifier = state
            .access_tokens
            .as_ref()
            .ok_or_else(|| StatusCode::SERVICE_UNAVAILABLE.into_response())?;
        let authorization = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|header| header.to_str().ok())
            .ok_or_else(unauthorized)?;
        let (scheme, token) = authorization.split_once(' ').ok_or_else(unauthorized)?;
        if !scheme.eq_ignore_ascii_case("Bearer") {
            return Err(unauthorized());
        }
        verifier.customer(token).ok_or_else(unauthorized)
    }
}
