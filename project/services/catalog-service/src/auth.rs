use std::{env, fs, io, sync::Arc};

use axum::{
    extract::FromRequestParts,
    http::{header, request::Parts, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use jsonwebtoken::{
    decode, decode_header, get_current_timestamp, Algorithm, DecodingKey, Validation,
};
use serde::Deserialize;

use crate::AppState;

#[derive(Clone)]
pub(crate) struct AccessTokens(Arc<Verifier>);

struct Verifier {
    key: DecodingKey,
    validation: Validation,
    keycloak_access_tokens: bool,
}

#[derive(Deserialize)]
struct Claims {
    iss: String,
    sub: String,
    exp: u64,
    typ: Option<String>,
}

impl AccessTokens {
    pub(crate) fn from_env() -> Result<Option<Self>, Box<dyn std::error::Error>> {
        let keycloak_access_tokens = match env::var("OIDC_KEYCLOAK_ACCESS_TOKENS").as_deref() {
            Err(env::VarError::NotPresent) | Ok("false") => false,
            Ok("true") => true,
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "OIDC_KEYCLOAK_ACCESS_TOKENS must be true or false",
                )
                .into())
            }
        };
        let issuer = env::var("OIDC_ISSUER").ok();
        let audience = env::var("OIDC_API_AUDIENCE").ok();
        let key_file = env::var_os("OIDC_PUBLIC_KEY_FILE");

        match (issuer, audience, key_file) {
            (None, None, None) if !keycloak_access_tokens => Ok(None),
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
                Ok(Some(Self(Arc::new(Verifier {
                    key,
                    validation,
                    keycloak_access_tokens,
                }))))
            }
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "set OIDC_ISSUER, OIDC_API_AUDIENCE and OIDC_PUBLIC_KEY_FILE together",
            )
            .into()),
        }
    }

    pub(crate) fn customer(&self, token: &str) -> Option<(Customer, u64)> {
        let header = decode_header(token).ok()?;
        let keycloak_token = match header.typ.as_deref() {
            Some("at+jwt" | "application/at+jwt") => false,
            Some("JWT") if self.0.keycloak_access_tokens => true,
            _ => {
                tracing::debug!("OIDC access token header type rejected");
                return None;
            }
        };
        let claims = decode::<Claims>(token, &self.0.key, &self.0.validation)
            .map_err(|error| tracing::debug!(kind = ?error.kind(), "OIDC access token validation failed"))
            .ok()?
            .claims;
        if claims.exp <= get_current_timestamp()
            || !valid_subject(&claims.sub)
            || (keycloak_token && claims.typ.as_deref() != Some("Bearer"))
        {
            return None;
        }
        Some((
            Customer {
                issuer: claims.iss,
                subject: claims.sub,
            },
            claims.exp,
        ))
    }

    pub(crate) fn signing_key(&self) -> DecodingKey {
        self.0.key.clone()
    }
}

pub(crate) fn valid_subject(subject: &str) -> bool {
    !subject.is_empty() && subject.len() <= 255 && !subject.contains('\0')
}

#[derive(Clone)]
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
        if let Some(authorization) = parts.headers.get(header::AUTHORIZATION) {
            let verifier = state
                .access_tokens
                .as_ref()
                .ok_or_else(|| StatusCode::SERVICE_UNAVAILABLE.into_response())?;
            let authorization = authorization.to_str().map_err(|_| unauthorized())?;
            let (scheme, token) = authorization.split_once(' ').ok_or_else(unauthorized)?;
            if !scheme.eq_ignore_ascii_case("Bearer") {
                return Err(unauthorized());
            }
            return verifier
                .customer(token)
                .map(|(customer, _)| customer)
                .ok_or_else(unauthorized);
        }

        if let Some(sessions) = &state.web_sessions {
            return sessions.customer(parts).map_err(|status| match status {
                StatusCode::UNAUTHORIZED => unauthorized(),
                _ => status.into_response(),
            });
        }

        if state.access_tokens.is_none() {
            return Err(StatusCode::SERVICE_UNAVAILABLE.into_response());
        }
        Err(unauthorized())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{encode, EncodingKey, Header};
    use serde::Serialize;

    #[test]
    fn keycloak_compatibility_requires_opt_in_and_signed_access_token_claims() {
        #[derive(Serialize)]
        struct Token<'a> {
            iss: &'a str,
            sub: &'a str,
            aud: &'a str,
            exp: u64,
            typ: &'a str,
        }

        let secret = b"unit-test-signing-key";
        let mut validation = Validation::new(Algorithm::HS256);
        validation.set_issuer(&["test-issuer"]);
        validation.set_audience(&["test-api"]);
        validation.set_required_spec_claims(&["iss", "aud", "exp", "sub"]);
        validation.leeway = 0;
        let verifier = |keycloak_access_tokens| {
            AccessTokens(Arc::new(Verifier {
                key: DecodingKey::from_secret(secret),
                validation: validation.clone(),
                keycloak_access_tokens,
            }))
        };
        let sign = |jose_type, token_type, audience, expires| {
            let mut header = Header::new(Algorithm::HS256);
            header.typ = Some(jose_type);
            encode(
                &header,
                &Token {
                    iss: "test-issuer",
                    sub: "customer",
                    aud: audience,
                    exp: expires,
                    typ: token_type,
                },
                &EncodingKey::from_secret(secret),
            )
            .unwrap()
        };
        let expires = get_current_timestamp() + 60;
        let access = sign("JWT".into(), "Bearer", "test-api", expires);
        assert!(verifier(false).customer(&access).is_none());
        let (customer, verified_expires) = verifier(true).customer(&access).unwrap();
        assert_eq!(customer.subject, "customer");
        assert_eq!(verified_expires, expires);

        for (token_type, audience, exp) in [
            ("ID", "test-api", expires),
            ("Refresh", "test-api", expires),
            ("Bearer", "web-client", expires),
            ("Bearer", "test-api", 1),
        ] {
            let token = sign("JWT".into(), token_type, audience, exp);
            assert!(verifier(true).customer(&token).is_none());
        }

        let standard = sign("at+jwt".into(), "", "test-api", expires);
        assert!(verifier(false).customer(&standard).is_some());
        assert!(verifier(true).customer(&standard).is_some());
    }
}
