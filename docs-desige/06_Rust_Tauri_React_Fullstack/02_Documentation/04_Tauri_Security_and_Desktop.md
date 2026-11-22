# Tauri 2 Desktop App — Architecture & Security

## Responsibilities

- Tauri Rust: device filesystem / printer integration only when needed; HTTPS API requests; optional OS-level credential store; signing and updater integration; session lifecycle; native notifications.
- React/TS WebView: user interactions, forms, tables, product/order screens, reuse `@nexa/ui` and `@nexa/contracts`.
- Avoid bundling all microservices on device or direct access to PostgreSQL/RabbitMQ from desktop.

## Desktop Authentication (production design)

1. Tauri Rust generates PKCE code verifier/challenge and cryptographic state + nonce.
2. Open **system browser**, authorize with OIDC provider (public/native client; no embedded secret).
3. Receive redirect via registered custom scheme or loopback URI; verify state, nonce, issuer and exact redirect.
4. Exchange code + code verifier; validate ID token and access token audience/issuer.
5. Store refresh token in OS-backed secure credential store where available; keep short-lived access tokens in Rust process memory.
6. Rust invokes gateway using `Authorization: Bearer ...`; React UI receives business data, not credentials.
7. Logout revokes session when supported, clears secure store and local caches.

OAuth system browser + PKCE is recommended for native apps in RFC 8252. Do not expose refresh token to TypeScript/localStorage, and do not ship client secrets in binaries.

## Browser authentication

Use server-side OIDC code flow + BFF session with `Secure; HttpOnly; SameSite=Lax/Strict` cookies and CSRF tokens on writes. For cross-site deployments, deliberately configure SameSite and CORS; never set `*` + credentials. Gateway validates session and performs RBAC on protected actions.

## Tauri security

- Set `app.security.capabilities` in `src-tauri/tauri.conf.json` explicitly. Define narrow permissions in `src-tauri/capabilities/*.json`; command exposure also needs Rust-level authorization and input validation.
- Use strict Content Security Policy; bundle scripts locally; no remote JS/CSS unless reviewed.
- Avoid broad FS or shell permissions, avoid remote content in privileged WebViews; security boundaries can merge for overlapping capability files.
- Use signed update distribution. Tauri updater requires signed artifacts and appropriate permissions; add signing keys/endpoint in production CI, not repository.
- For new desktop capabilities (barcode/printer/receipt/export), create targeted Tauri commands, validate all user inputs, and avoid arbitrary paths/commands.
- Add platform validation for macOS notarization, Windows signing, Linux package prerequisites.

## Offline strategy

**MVP: online checkout only.** Optional read-only product cache or draft order entries can be persisted with local SQLite, but stale price or stock must be rechecked online. POS/offline payment is a separate major design requiring reconciliation, secure hardware/payment compliance, and explicit conflict handling. If offline is needed later: encrypted SQLite per tenant/user, local queue with unique IDs, transactional sync, conflict policy, cache expiry, remote logout revocation.

## Command sample

```ts
import { invoke } from '@tauri-apps/api/core';
const products = await invoke<ProductDto[]>('list_products');
```

```rust
#[tauri::command]
async fn list_products() -> Result<Vec<ProductDto>, String> {
    // Use authenticated Rust API client in production.
    // Do not allow arbitrary user-provided URL or leaked tokens.
    todo!()
}
```

This code sketch is illustrative; the starter includes a development-only HTTP example.
