use std::{
    collections::HashMap,
    env, io,
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use axum::{
    extract::{Query, Request, State},
    http::{header, request::Parts, HeaderMap, HeaderValue, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Redirect, Response},
    Json,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use url::{form_urlencoded, Host, Url};

use crate::{
    auth::{valid_subject, AccessTokens, Customer},
    AppState,
};

const CSRF_HEADER: &str = "x-csrf-token";
const LOGIN_TTL: Duration = Duration::from_secs(5 * 60);
const SESSION_TTL: Duration = Duration::from_secs(8 * 60 * 60);
const MAX_PENDING_LOGINS: usize = 1000;
const MAX_SESSIONS: usize = 10_000;

pub(crate) struct WebSessions {
    provider: OidcProvider,
    origin: HeaderValue,
    return_url: Url,
    secure: bool,
    store: Mutex<SessionStore>,
}

struct OidcProvider {
    client: reqwest::Client,
    client_id: String,
    client_secret: String,
    authorization_url: Url,
    token_url: Url,
    redirect_uri: Url,
    issuer: String,
    key: DecodingKey,
    validation: Validation,
    access_tokens: AccessTokens,
}

#[derive(Default)]
struct SessionStore {
    pending: HashMap<String, PendingLogin>,
    sessions: HashMap<String, WebSession>,
}

struct PendingLogin {
    state: String,
    nonce: String,
    verifier: String,
    expires: Instant,
}

struct WebSession {
    customer: Customer,
    csrf_token: String,
    expires: Instant,
}

#[derive(Deserialize)]
pub(crate) struct AuthorizationResponse {
    code: Option<String>,
    state: String,
    iss: Option<String>,
    error: Option<String>,
}

#[derive(Deserialize)]
struct TokenResponse {
    id_token: String,
    access_token: String,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Audience {
    One(String),
    Multiple(Vec<String>),
}

#[derive(Deserialize)]
struct Identity {
    iss: String,
    sub: String,
    aud: Audience,
    azp: Option<String>,
    iat: u64,
    exp: u64,
    nonce: String,
    typ: Option<String>,
}

#[derive(Serialize)]
struct SessionInfo {
    csrf_token: String,
}

impl WebSessions {
    pub(crate) fn from_env(
        access_tokens: Option<&AccessTokens>,
    ) -> Result<Option<Arc<Self>>, Box<dyn std::error::Error + Send + Sync>> {
        const SETTINGS: [&str; 7] = [
            "OIDC_WEB_CLIENT_ID",
            "OIDC_WEB_CLIENT_SECRET",
            "OIDC_WEB_AUTHORIZATION_URL",
            "OIDC_WEB_TOKEN_URL",
            "OIDC_WEB_REDIRECT_URI",
            "OIDC_WEB_ORIGIN",
            "OIDC_WEB_ALLOW_HTTP_LOOPBACK",
        ];
        if SETTINGS.iter().all(|key| env::var_os(key).is_none()) {
            return Ok(None);
        }
        let access_tokens = access_tokens.ok_or_else(|| {
            invalid_config(
                "web OIDC requires OIDC_ISSUER, OIDC_API_AUDIENCE and OIDC_PUBLIC_KEY_FILE",
            )
        })?;
        let allow_http_loopback = match env::var("OIDC_WEB_ALLOW_HTTP_LOOPBACK").as_deref() {
            Err(env::VarError::NotPresent) | Ok("false") => false,
            Ok("true") => true,
            _ => {
                return Err(
                    invalid_config("OIDC_WEB_ALLOW_HTTP_LOOPBACK must be true or false").into(),
                )
            }
        };
        let client_id = required_setting("OIDC_WEB_CLIENT_ID")?;
        if client_id == required_setting("OIDC_API_AUDIENCE")? {
            return Err(
                invalid_config("OIDC_WEB_CLIENT_ID must differ from OIDC_API_AUDIENCE").into(),
            );
        }
        let client_secret = required_setting("OIDC_WEB_CLIENT_SECRET")?;
        let authorization_url = endpoint("OIDC_WEB_AUTHORIZATION_URL", allow_http_loopback)?;
        let token_url = endpoint("OIDC_WEB_TOKEN_URL", allow_http_loopback)?;
        let redirect_uri = endpoint("OIDC_WEB_REDIRECT_URI", allow_http_loopback)?;
        let return_url = endpoint("OIDC_WEB_ORIGIN", allow_http_loopback)?;
        endpoint("OIDC_ISSUER", allow_http_loopback)?;
        let issuer = required_setting("OIDC_ISSUER")?;
        if return_url.path() != "/"
            || redirect_uri.path() != "/auth/callback"
            || return_url.origin() != redirect_uri.origin()
        {
            return Err(invalid_config(
                "OIDC_WEB_REDIRECT_URI must be /auth/callback on OIDC_WEB_ORIGIN",
            )
            .into());
        }
        let origin = HeaderValue::from_str(&return_url.origin().ascii_serialization())?;
        let secure = return_url.scheme() == "https";
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_issuer(&[&issuer]);
        validation.set_audience(&[&client_id]);
        validation.set_required_spec_claims(&["iss", "aud", "exp", "sub"]);
        validation.validate_nbf = true;
        validation.leeway = 0;
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(10))
            .build()?;

        Ok(Some(Arc::new(Self {
            provider: OidcProvider {
                client,
                client_id,
                client_secret,
                authorization_url,
                token_url,
                redirect_uri,
                issuer,
                key: access_tokens.signing_key(),
                validation,
                access_tokens: access_tokens.clone(),
            },
            origin,
            return_url,
            secure,
            store: Mutex::new(SessionStore::default()),
        })))
    }

    pub(crate) fn customer(&self, request: &Parts) -> Result<Customer, StatusCode> {
        let write = !matches!(request.method, Method::GET | Method::HEAD);
        let id = cookie(&request.headers, self.session_cookie()).ok_or(StatusCode::UNAUTHORIZED)?;
        let mut store = self.store()?;
        let session = live_session(&mut store, id)?;
        if write {
            self.validate_write(&request.headers, session)?;
        }
        Ok(session.customer.clone())
    }

    fn validate_write(&self, headers: &HeaderMap, session: &WebSession) -> Result<(), StatusCode> {
        let origin = single_header(headers, header::ORIGIN.as_str());
        let csrf = single_header(headers, CSRF_HEADER);
        if origin != Some(&self.origin)
            || !csrf.is_some_and(|value| token_eq(value.as_bytes(), &session.csrf_token))
        {
            return Err(StatusCode::FORBIDDEN);
        }
        Ok(())
    }

    fn store(&self) -> Result<MutexGuard<'_, SessionStore>, StatusCode> {
        self.store
            .lock()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
    }

    fn session_cookie(&self) -> &'static str {
        if self.secure {
            "__Host-nexa_session"
        } else {
            "nexa_session_dev"
        }
    }

    fn login_cookie(&self) -> &'static str {
        if self.secure {
            "__Host-nexa_login"
        } else {
            "nexa_login_dev"
        }
    }

    fn set_cookie(&self, name: &str, value: &str, ttl: Duration) -> HeaderValue {
        let secure = if self.secure { "; Secure" } else { "" };
        HeaderValue::from_str(&format!(
            "{name}={value}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}{secure}",
            ttl.as_secs(),
        ))
        .expect("cookie names and generated values contain only valid header bytes")
    }
}

impl OidcProvider {
    async fn exchange_code(
        &self,
        code: &str,
        login: &PendingLogin,
    ) -> Result<(Customer, u64), StatusCode> {
        let encode_basic =
            |value: &str| form_urlencoded::byte_serialize(value.as_bytes()).collect::<String>();
        let response = self
            .client
            .post(self.token_url.clone())
            .basic_auth(
                encode_basic(&self.client_id),
                Some(encode_basic(&self.client_secret)),
            )
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", self.redirect_uri.as_str()),
                ("code_verifier", login.verifier.as_str()),
            ])
            .send()
            .await
            .map_err(|_| StatusCode::BAD_GATEWAY)?;
        if matches!(
            response.status(),
            StatusCode::BAD_REQUEST | StatusCode::UNAUTHORIZED
        ) {
            tracing::warn!(status = %response.status(), "OIDC provider rejected the code exchange");
            return Err(StatusCode::UNAUTHORIZED);
        }
        if !response.status().is_success() {
            return Err(StatusCode::BAD_GATEWAY);
        }
        let tokens = response
            .json::<TokenResponse>()
            .await
            .map_err(|_| StatusCode::BAD_GATEWAY)?;
        let id_header = decode_header(&tokens.id_token).map_err(|_| StatusCode::UNAUTHORIZED)?;
        if !matches!(id_header.typ.as_deref(), Some("JWT") | None) {
            return Err(StatusCode::UNAUTHORIZED);
        }
        let identity = decode::<Identity>(&tokens.id_token, &self.key, &self.validation)
            .map_err(|error| {
                tracing::warn!(kind = ?error.kind(), "OIDC ID token validation failed");
                StatusCode::UNAUTHORIZED
            })?
            .claims;
        let trusted_audience = match &identity.aud {
            Audience::One(audience) => audience == &self.client_id,
            Audience::Multiple(audiences) => audiences.as_slice() == [self.client_id.as_str()],
        };
        if !trusted_audience
            || !matches!(identity.typ.as_deref(), Some("ID") | None)
            || identity
                .azp
                .as_ref()
                .is_some_and(|azp| azp != &self.client_id)
            || identity.iat > epoch_time()?.as_secs() + 60
            || !valid_subject(&identity.sub)
            || !token_eq(identity.nonce.as_bytes(), &login.nonce)
        {
            tracing::warn!("OIDC identity claims did not match the web login");
            return Err(StatusCode::UNAUTHORIZED);
        }
        let (customer, access_expires) = self
            .access_tokens
            .customer(&tokens.access_token)
            .ok_or_else(|| {
                tracing::warn!("OIDC access token validation failed for the web login");
                StatusCode::UNAUTHORIZED
            })?;
        if customer.issuer != identity.iss || customer.subject != identity.sub {
            return Err(StatusCode::UNAUTHORIZED);
        }
        Ok((customer, identity.exp.min(access_expires)))
    }
}

pub(crate) async fn login(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    let web = state.web_sessions.ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let binding = random_token()?;
    let pending = PendingLogin {
        state: random_token()?,
        nonce: random_token()?,
        verifier: random_token()?,
        expires: Instant::now() + LOGIN_TTL,
    };
    let mut url = web.provider.authorization_url.clone();
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(pending.verifier.as_bytes()));
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", &web.provider.client_id)
        .append_pair("redirect_uri", web.provider.redirect_uri.as_str())
        .append_pair("scope", "openid")
        .append_pair("state", &pending.state)
        .append_pair("nonce", &pending.nonce)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256");
    let mut store = web.store()?;
    store
        .pending
        .retain(|_, login| login.expires > Instant::now());
    if let Some(old_binding) = cookie(&headers, web.login_cookie()) {
        store.pending.remove(old_binding);
    }
    if store.pending.len() >= MAX_PENDING_LOGINS {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    let binding_cookie = web.set_cookie(web.login_cookie(), &binding, LOGIN_TTL);
    store.pending.insert(binding, pending);
    Ok((
        [(header::SET_COOKIE, binding_cookie)],
        Redirect::to(url.as_str()),
    )
        .into_response())
}

pub(crate) async fn callback(
    State(state): State<AppState>,
    Query(reply): Query<AuthorizationResponse>,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    let web = state.web_sessions.ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let binding = cookie(&headers, web.login_cookie()).ok_or(StatusCode::BAD_REQUEST)?;
    let pending = {
        let mut store = web.store()?;
        let valid = store.pending.get(binding).is_some_and(|login| {
            login.expires > Instant::now() && token_eq(reply.state.as_bytes(), &login.state)
        });
        if !valid {
            return Err(StatusCode::BAD_REQUEST);
        }
        store
            .pending
            .remove(binding)
            .ok_or(StatusCode::BAD_REQUEST)?
    };
    if reply.error.is_some()
        || reply
            .iss
            .is_some_and(|issuer| issuer != web.provider.issuer)
    {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let code = reply
        .code
        .filter(|code| !code.is_empty())
        .ok_or(StatusCode::BAD_REQUEST)?;
    let (customer, expires) = web.provider.exchange_code(&code, &pending).await?;
    let now = Instant::now();
    let lifetime = Duration::from_secs(expires)
        .saturating_sub(epoch_time()?)
        .min(SESSION_TTL);
    if lifetime.as_secs() == 0 {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let id = random_token()?;
    let session = WebSession {
        customer,
        csrf_token: random_token()?,
        expires: now + lifetime,
    };
    let mut store = web.store()?;
    store
        .sessions
        .retain(|_, session| session.expires > Instant::now());
    if let Some(old_id) = cookie(&headers, web.session_cookie()) {
        store.sessions.remove(old_id);
    }
    if store.sessions.len() >= MAX_SESSIONS {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    let session_cookie = web.set_cookie(web.session_cookie(), &id, lifetime);
    store.sessions.insert(id, session);
    let mut response = Redirect::to(web.return_url.as_str()).into_response();
    response
        .headers_mut()
        .append(header::SET_COOKIE, session_cookie);
    response.headers_mut().append(
        header::SET_COOKIE,
        web.set_cookie(web.login_cookie(), "", Duration::ZERO),
    );
    Ok(response)
}

pub(crate) async fn session(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    let web = state.web_sessions.ok_or(StatusCode::UNAUTHORIZED)?;
    let id = cookie(&headers, web.session_cookie()).ok_or(StatusCode::UNAUTHORIZED)?;
    let mut store = web.store()?;
    let session = live_session(&mut store, id)?;
    Ok(Json(SessionInfo {
        csrf_token: session.csrf_token.clone(),
    })
    .into_response())
}

pub(crate) async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    let web = state.web_sessions.ok_or(StatusCode::UNAUTHORIZED)?;
    let id = cookie(&headers, web.session_cookie()).ok_or(StatusCode::UNAUTHORIZED)?;
    let mut store = web.store()?;
    let session = live_session(&mut store, id)?;
    web.validate_write(&headers, session)?;
    store.sessions.remove(id);
    Ok((
        StatusCode::NO_CONTENT,
        [(
            header::SET_COOKIE,
            web.set_cookie(web.session_cookie(), "", Duration::ZERO),
        )],
    )
        .into_response())
}

pub(crate) async fn private_response(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response
}

fn live_session<'a>(store: &'a mut SessionStore, id: &str) -> Result<&'a WebSession, StatusCode> {
    if store
        .sessions
        .get(id)
        .is_some_and(|session| session.expires <= Instant::now())
    {
        store.sessions.remove(id);
    }
    store.sessions.get(id).ok_or(StatusCode::UNAUTHORIZED)
}

fn invalid_config(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

fn required_setting(name: &str) -> Result<String, io::Error> {
    env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| invalid_config(format!("set {name} when configuring web OIDC")))
}

fn endpoint(name: &str, allow_http_loopback: bool) -> Result<Url, io::Error> {
    let error = || {
        invalid_config(format!("{name} must be an HTTPS URL, or HTTP on loopback with OIDC_WEB_ALLOW_HTTP_LOOPBACK=true"))
    };
    let url = Url::parse(&required_setting(name)?).map_err(|_| error())?;
    let loopback = match url.host() {
        Some(Host::Domain(host)) => host == "localhost",
        Some(Host::Ipv4(ip)) => ip.is_loopback(),
        Some(Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    };
    let valid_scheme =
        url.scheme() == "https" || (url.scheme() == "http" && allow_http_loopback && loopback);
    if valid_scheme
        && url.has_host()
        && url.username().is_empty()
        && url.password().is_none()
        && url.fragment().is_none()
        && url.query().is_none()
    {
        Ok(url)
    } else {
        Err(error())
    }
}

fn random_token() -> Result<String, StatusCode> {
    let mut bytes = [0; 32];
    getrandom::getrandom(&mut bytes).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn epoch_time() -> Result<Duration, StatusCode> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

fn token_eq(candidate: &[u8], token: &str) -> bool {
    bool::from(candidate.ct_eq(token.as_bytes()))
}

fn single_header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a HeaderValue> {
    let mut values = headers.get_all(name).iter();
    let value = values.next()?;
    values.next().is_none().then_some(value)
}

fn cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let mut matches = headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|header| header.to_str().ok())
        .flat_map(|header| header.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .filter_map(|(key, value)| (key == name).then_some(value));
    let value = matches.next()?;
    (value.len() == 43 && matches.next().is_none()).then_some(value)
}
