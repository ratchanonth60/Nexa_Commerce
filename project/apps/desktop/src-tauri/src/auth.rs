use std::{
    env,
    net::{Ipv4Addr, SocketAddrV4},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{
    extract::{Query, State as CallbackState},
    http::{header, HeaderMap, Method, StatusCode},
    response::IntoResponse,
    routing::get,
    Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use jsonwebtoken::{
    decode, decode_header,
    jwk::{AlgorithmParameters, JwkSet, KeyAlgorithm, KeyOperations, PublicKeyUse},
    Algorithm, DecodingKey, Validation,
};
use reqwest::{Client, Url};
use ring::{
    digest::{digest, SHA256},
    rand::{SecureRandom, SystemRandom},
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use tokio::{net::TcpListener, process::Command, sync::oneshot, time::timeout};

const CALLBACK_PORT: u16 = 17842;
const CALLBACK_PATH: &str = "/oidc/callback";
const LOGIN_TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Default)]
pub(crate) struct DesktopSession {
    current: Mutex<Session>,
    login: tokio::sync::Mutex<()>,
}

#[derive(Default)]
struct Session {
    revision: u64,
    access: Option<Access>,
}

#[derive(Clone)]
struct Access {
    token: String,
    expires_at: u64,
    operator: bool,
}

#[derive(Serialize)]
pub(crate) struct SessionStatus {
    authenticated: bool,
    operator: bool,
    expires_at: Option<u64>,
}

impl Session {
    fn status(&self) -> SessionStatus {
        SessionStatus {
            authenticated: self.access.is_some(),
            operator: self.access.as_ref().is_some_and(|access| access.operator),
            expires_at: self.access.as_ref().map(|access| access.expires_at),
        }
    }
}

impl DesktopSession {
    fn update<T>(&self, function: impl FnOnce(&mut Session) -> T) -> Result<T, String> {
        let mut current = self
            .current
            .lock()
            .map_err(|_| "Desktop session unavailable")?;
        if current
            .access
            .as_ref()
            .is_some_and(|access| access.expires_at <= now())
        {
            current.access = None;
        }
        Ok(function(&mut current))
    }

    pub(crate) fn bearer(&self) -> Result<String, String> {
        self.update(|current| current.access.as_ref().map(|access| access.token.clone()))?
            .ok_or_else(|| "Sign in to use the cart.".into())
    }

    pub(crate) fn operator_bearer(&self) -> Result<String, String> {
        self.update(|current| {
            let access = current
                .access
                .as_ref()
                .ok_or("Sign in to use the backoffice.")?;
            if !access.operator {
                return Err("This account does not have operator access.".into());
            }
            Ok(access.token.clone())
        })?
    }
}

#[tauri::command]
pub(crate) fn get_session(
    session: tauri::State<'_, DesktopSession>,
) -> Result<SessionStatus, String> {
    session.update(|current| current.status())
}

#[tauri::command]
pub(crate) fn sign_out(session: tauri::State<'_, DesktopSession>) -> Result<SessionStatus, String> {
    session.update(|current| {
        current.revision = current.revision.wrapping_add(1);
        current.access = None;
        current.status()
    })
}

#[tauri::command]
pub(crate) async fn sign_in(
    session: tauri::State<'_, DesktopSession>,
) -> Result<SessionStatus, String> {
    let _login = session
        .login
        .try_lock()
        .map_err(|_| "Sign-in is already in progress.")?;
    let (revision, status) = session.update(|current| (current.revision, current.status()))?;
    if status.authenticated {
        return Ok(status);
    }

    let config = OidcConfig::load()?;
    let access = config.authorize().await?;
    session.update(|current| {
        if revision != current.revision {
            return Err("Sign-in was cancelled.".into());
        }
        current.access = Some(access);
        Ok(current.status())
    })?
}

struct OidcConfig {
    issuer: String,
    client_id: String,
    api_audience: String,
    scopes: String,
    keycloak_tokens: bool,
}

#[derive(Deserialize)]
struct Discovery {
    issuer: String,
    authorization_endpoint: Url,
    token_endpoint: Url,
    jwks_uri: Url,
    #[serde(default)]
    authorization_response_iss_parameter_supported: bool,
}

#[derive(Deserialize)]
struct Tokens {
    id_token: String,
    access_token: String,
    token_type: String,
}

#[derive(Clone, Deserialize)]
struct IdClaims {
    sub: String,
    nonce: String,
    iat: u64,
    aud: Audience,
    azp: Option<String>,
}

#[derive(Clone, Deserialize)]
#[serde(untagged)]
enum Audience {
    One(String),
    Many(Vec<String>),
}

#[derive(Clone, Deserialize)]
struct AccessClaims {
    sub: String,
    exp: u64,
    typ: Option<String>,
    #[serde(default)]
    nexa_roles: Vec<String>,
}

impl OidcConfig {
    fn load() -> Result<Self, String> {
        fn required(name: &str) -> Result<String, String> {
            env::var(name)
                .ok()
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| format!("Set {name} before starting the desktop app."))
        }

        let scopes = env::var("OIDC_DESKTOP_SCOPES").unwrap_or_else(|_| "openid".into());
        if !scopes
            .split_ascii_whitespace()
            .any(|scope| scope == "openid")
        {
            return Err("OIDC_DESKTOP_SCOPES must include openid.".into());
        }

        Ok(Self {
            issuer: required("OIDC_ISSUER")?,
            client_id: required("OIDC_DESKTOP_CLIENT_ID")?,
            api_audience: required("OIDC_API_AUDIENCE")?,
            scopes,
            keycloak_tokens: env::var("OIDC_KEYCLOAK_ACCESS_TOKENS").as_deref() == Ok("true"),
        })
    }

    async fn authorize(&self) -> Result<Access, String> {
        let issuer = Url::parse(&self.issuer).map_err(|_| "Invalid OIDC issuer URL.")?;
        validate_provider_url(&issuer)?;
        if issuer.query().is_some() {
            return Err("OIDC issuer URL must not have a query.".into());
        }
        let client = Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|_| "Could not initialize the OIDC client.")?;

        let discovery_url = format!(
            "{}/.well-known/openid-configuration",
            self.issuer.trim_end_matches('/')
        );
        let provider: Discovery = provider_json(
            client.get(discovery_url),
            "Could not read OIDC provider discovery.",
        )
        .await?;
        if provider.issuer != self.issuer {
            return Err("OIDC discovery returned a different issuer.".into());
        }
        for endpoint in [
            &provider.authorization_endpoint,
            &provider.token_endpoint,
            &provider.jwks_uri,
        ] {
            validate_provider_url(endpoint)?;
            if issuer.scheme() == "https" && endpoint.scheme() != "https" {
                return Err("An HTTPS OIDC issuer requires HTTPS provider endpoints.".into());
            }
        }

        let login_state = random_secret()?;
        let nonce = random_secret()?;
        let verifier = random_secret()?;
        let challenge = URL_SAFE_NO_PAD.encode(digest(&SHA256, verifier.as_bytes()));
        let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, CALLBACK_PORT))
            .await
            .map_err(|_| {
                format!("Could not bind the desktop sign-in callback on port {CALLBACK_PORT}.")
            })?;
        let redirect_uri = format!("http://127.0.0.1:{CALLBACK_PORT}{CALLBACK_PATH}");
        let mut authorization_url = provider.authorization_endpoint;
        authorization_url.query_pairs_mut().extend_pairs([
            ("client_id", self.client_id.as_str()),
            ("response_type", "code"),
            ("redirect_uri", redirect_uri.as_str()),
            ("scope", self.scopes.as_str()),
            ("state", login_state.as_str()),
            ("nonce", nonce.as_str()),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
        ]);

        let code = wait_for_browser(
            listener,
            authorization_url,
            login_state,
            provider.issuer,
            provider.authorization_response_iss_parameter_supported,
        )
        .await?;
        let tokens: Tokens = provider_json(
            client.post(provider.token_endpoint).form(&[
                ("grant_type", "authorization_code"),
                ("code", code.as_str()),
                ("client_id", self.client_id.as_str()),
                ("redirect_uri", redirect_uri.as_str()),
                ("code_verifier", verifier.as_str()),
            ]),
            "OIDC code exchange failed.",
        )
        .await?;
        if !tokens.token_type.eq_ignore_ascii_case("Bearer") {
            return Err("The provider returned an unsupported token type.".into());
        }

        let keys: JwkSet = provider_json(
            client.get(provider.jwks_uri),
            "Could not read OIDC signing keys.",
        )
        .await?;
        let (identity, _) =
            verify_token::<IdClaims>(&tokens.id_token, &keys, &self.issuer, &self.client_id)
                .ok_or("OIDC ID token validation failed.")?;
        let multiple_audiences = match &identity.aud {
            Audience::One(audience) => audience != &self.client_id,
            Audience::Many(audiences) => audiences.len() != 1,
        };
        if identity.sub.is_empty()
            || identity.nonce != nonce
            || identity.iat > now().saturating_add(60)
            || ((multiple_audiences || identity.azp.is_some())
                && identity.azp.as_deref() != Some(&self.client_id))
        {
            return Err("OIDC ID token claims did not match the sign-in request.".into());
        }

        let (access, token_type) = verify_token::<AccessClaims>(
            &tokens.access_token,
            &keys,
            &self.issuer,
            &self.api_audience,
        )
        .ok_or("OIDC access token validation failed.")?;
        let valid_type = matches!(token_type.as_deref(), Some("at+jwt" | "application/at+jwt"))
            || (self.keycloak_tokens
                && token_type.as_deref() == Some("JWT")
                && access.typ.as_deref() == Some("Bearer"));
        if !valid_type || access.sub.is_empty() {
            return Err("The provider returned an invalid API access token.".into());
        }

        Ok(Access {
            token: tokens.access_token,
            expires_at: access.exp,
            operator: access.nexa_roles.iter().any(|role| role == "nexa-operator"),
        })
    }
}

fn validate_provider_url(url: &Url) -> Result<(), String> {
    let loopback = matches!(url.host(), Some(url::Host::Ipv4(host)) if host.is_loopback())
        || matches!(url.host(), Some(url::Host::Ipv6(host)) if host.is_loopback());
    if url.host().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || !(url.scheme() == "https" || (url.scheme() == "http" && loopback))
    {
        return Err("OIDC provider URLs require HTTPS or a numeric HTTP loopback address.".into());
    }
    Ok(())
}

async fn provider_json<T: DeserializeOwned>(
    request: reqwest::RequestBuilder,
    error: &str,
) -> Result<T, String> {
    let response = request.send().await.map_err(|_| error)?;
    if !response.status().is_success() {
        return Err(error.into());
    }
    response.json().await.map_err(|_| error.into())
}

fn verify_token<T: Clone + DeserializeOwned>(
    token: &str,
    keys: &JwkSet,
    issuer: &str,
    audience: &str,
) -> Option<(T, Option<String>)> {
    let header = decode_header(token).ok()?;
    if header.alg != Algorithm::RS256 {
        return None;
    }
    let mut matching_keys = keys.keys.iter().filter(|key| {
        matches!(key.algorithm, AlgorithmParameters::RSA(_))
            && key
                .common
                .key_algorithm
                .is_none_or(|algorithm| algorithm == KeyAlgorithm::RS256)
            && key
                .common
                .public_key_use
                .as_ref()
                .is_none_or(|usage| usage == &PublicKeyUse::Signature)
            && key
                .common
                .key_operations
                .as_ref()
                .is_none_or(|ops| ops.contains(&KeyOperations::Verify))
            && key.common.key_id.as_ref() == header.kid.as_ref()
    });
    let key = matching_keys.next()?;
    if matching_keys.next().is_some() {
        return None;
    }
    let key = DecodingKey::from_jwk(key).ok()?;
    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_issuer(&[issuer]);
    validation.set_audience(&[audience]);
    validation.set_required_spec_claims(&["iss", "aud", "exp", "sub"]);
    validation.validate_nbf = true;
    validation.leeway = 0;
    let claims = decode::<T>(token, &key, &validation).ok()?.claims;
    Some((claims, header.typ))
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn random_secret() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| "Could not generate a sign-in secret.")?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

struct BrowserCallback {
    state: String,
    issuer: String,
    require_issuer: bool,
    reply: Mutex<Option<oneshot::Sender<Result<String, String>>>>,
}

#[derive(Deserialize)]
struct CallbackQuery {
    state: String,
    code: Option<String>,
    error: Option<String>,
    iss: Option<String>,
}

async fn browser_callback(
    CallbackState(callback): CallbackState<Arc<BrowserCallback>>,
    method: Method,
    headers: HeaderMap,
    Query(query): Query<CallbackQuery>,
) -> impl IntoResponse {
    let (status, message) = handle_callback(&callback, method, &headers, query);
    (
        status,
        [
            (header::CACHE_CONTROL, "no-store"),
            (header::REFERRER_POLICY, "no-referrer"),
        ],
        message,
    )
}

fn handle_callback(
    callback: &BrowserCallback,
    method: Method,
    headers: &HeaderMap,
    query: CallbackQuery,
) -> (StatusCode, &'static str) {
    let expected_host = format!("127.0.0.1:{CALLBACK_PORT}");
    if method != Method::GET
        || headers
            .get(header::HOST)
            .and_then(|host| host.to_str().ok())
            != Some(expected_host.as_str())
        || query.state != callback.state
        || match query.iss.as_deref() {
            Some(issuer) => issuer != callback.issuer,
            None => callback.require_issuer && query.code.is_some(),
        }
    {
        return (StatusCode::BAD_REQUEST, "Invalid desktop sign-in callback.");
    }
    let (result, response_text) = match (query.code, query.error) {
        (Some(code), None) if !code.is_empty() => {
            (Ok(code), "Return to Nexa Commerce to finish signing in.")
        }
        (None, Some(_)) => (
            Err("The identity provider did not complete sign-in.".into()),
            "Sign-in did not complete. Return to Nexa Commerce to try again.",
        ),
        _ => return (StatusCode::BAD_REQUEST, "Invalid desktop sign-in callback."),
    };
    if let Ok(mut reply) = callback.reply.lock() {
        if let Some(reply) = reply.take() {
            let _ = reply.send(result);
            return (StatusCode::OK, response_text);
        }
    }
    (
        StatusCode::GONE,
        "This desktop sign-in request has expired.",
    )
}

async fn wait_for_browser(
    listener: TcpListener,
    authorization_url: Url,
    state: String,
    issuer: String,
    require_issuer: bool,
) -> Result<String, String> {
    let (reply, result) = oneshot::channel();
    let (stop, stopped) = oneshot::channel();
    let callback = Router::new()
        .route(CALLBACK_PATH, get(browser_callback))
        .with_state(Arc::new(BrowserCallback {
            state,
            issuer,
            require_issuer,
            reply: Mutex::new(Some(reply)),
        }));
    let mut server = tauri::async_runtime::spawn(async move {
        axum::serve(listener, callback)
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await
    });

    let outcome = match open_browser(&authorization_url).await {
        Ok(()) => match timeout(LOGIN_TIMEOUT, result).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err("The desktop sign-in callback closed.".into()),
            Err(_) => Err("Desktop sign-in timed out.".into()),
        },
        Err(error) => Err(error),
    };
    let _ = stop.send(());
    if timeout(Duration::from_secs(1), &mut server).await.is_err() {
        server.abort();
    }
    outcome
}

async fn open_browser(url: &Url) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let mut opener = Command::new("/usr/bin/open");
    #[cfg(target_os = "linux")]
    let mut opener = Command::new("xdg-open");
    #[cfg(target_os = "windows")]
    let mut opener = {
        let mut opener = Command::new("rundll32.exe");
        opener.arg("url.dll,FileProtocolHandler");
        opener
    };
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    return Err("System-browser sign-in is unavailable on this platform.".into());

    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    {
        use std::process::Stdio;

        let status = timeout(
            Duration::from_secs(10),
            opener
                .arg(url.as_str())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status(),
        )
        .await
        .map_err(|_| "Opening the system browser timed out.")?
        .map_err(|_| "Could not open the system browser.")?;
        if !status.success() {
            return Err("Could not open the system browser.".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callback_rejects_unrelated_requests_and_consumes_one_matching_code() {
        let (reply, mut received) = oneshot::channel();
        let callback = BrowserCallback {
            state: "expected-state".into(),
            issuer: "http://127.0.0.1:8080/realms/nexa".into(),
            require_issuer: true,
            reply: Mutex::new(Some(reply)),
        };
        let query = |state: &str, issuer: Option<&str>| CallbackQuery {
            state: state.into(),
            code: Some("authorization-code".into()),
            error: None,
            iss: issuer.map(str::to_owned),
        };
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, "unexpected.example".parse().unwrap());

        assert_eq!(
            handle_callback(
                &callback,
                Method::GET,
                &headers,
                query(&callback.state, Some(&callback.issuer)),
            )
            .0,
            StatusCode::BAD_REQUEST,
        );
        headers.insert(
            header::HOST,
            format!("127.0.0.1:{CALLBACK_PORT}").parse().unwrap(),
        );
        for (method, state, issuer) in [
            (Method::GET, "wrong-state", Some(callback.issuer.as_str())),
            (Method::GET, callback.state.as_str(), None),
            (
                Method::GET,
                callback.state.as_str(),
                Some("https://other.example"),
            ),
            (
                Method::HEAD,
                callback.state.as_str(),
                Some(callback.issuer.as_str()),
            ),
        ] {
            assert_eq!(
                handle_callback(&callback, method, &headers, query(state, issuer)).0,
                StatusCode::BAD_REQUEST
            );
        }
        assert!(matches!(
            received.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));

        let valid_query = || query(&callback.state, Some(&callback.issuer));
        assert_eq!(
            handle_callback(&callback, Method::GET, &headers, valid_query()).0,
            StatusCode::OK
        );
        assert_eq!(received.try_recv().unwrap().unwrap(), "authorization-code");
        assert_eq!(
            handle_callback(&callback, Method::GET, &headers, valid_query()).0,
            StatusCode::GONE
        );
    }

    #[test]
    fn backoffice_requires_an_unexpired_operator_session() {
        let session = DesktopSession::default();
        assert!(session.operator_bearer().is_err());

        session
            .update(|current| {
                current.access = Some(Access {
                    token: "customer".into(),
                    expires_at: u64::MAX,
                    operator: false,
                });
            })
            .unwrap();
        assert_eq!(session.bearer().unwrap(), "customer");
        assert!(session.operator_bearer().is_err());

        session
            .update(|current| {
                current.access = Some(Access {
                    token: "operator".into(),
                    expires_at: u64::MAX,
                    operator: true,
                });
            })
            .unwrap();
        assert_eq!(session.operator_bearer().unwrap(), "operator");

        session
            .current
            .lock()
            .unwrap()
            .access
            .as_mut()
            .unwrap()
            .expires_at = 0;
        assert!(session.operator_bearer().is_err());
        assert!(session.bearer().is_err());
        assert!(session.current.lock().unwrap().access.is_none());
    }
}
