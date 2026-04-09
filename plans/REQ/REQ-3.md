# REQ-3: Origin Validation via `#[from_request]` Extractor

**blocked_by:** [REQ-2]
**unlocks:** []

## Status: Planned

## Goal

Implement AUTH-18 (Origin header validation) using the REQ extractor system rather than as hardcoded middleware logic. This demonstrates the pattern and closes the cross-origin WebSocket attack vector.

## Why Not Middleware

Hardcoding Origin validation in `CombinedAuthMiddleware` makes it:
- Global (same allowlist for all methods — no per-activation overrides)
- Untestable without a real HTTP request cycle
- Invisible in the method signature (callers can't see which methods enforce it)

With REQ-2, Origin validation is a typed parameter: it appears in the method signature, fails with a semantic error, and can be unit-tested against a fake `RequestContext`.

## Shape

### Extractor (plexus-transport)

```rust
// src/extractors/origin.rs

pub struct ValidOrigin(pub String);

/// Returns Ok(ValidOrigin) if the Origin header is in the allowlist,
/// Ok(ValidOrigin("")) if no Origin header (non-browser client),
/// Err(Unauthenticated) if Origin is present but not allowed.
pub fn require_allowed_origin(
    allowed: &'static [&'static str],
) -> impl Fn(&RequestContext) -> Result<ValidOrigin, PlexusError> {
    move |ctx| {
        match ctx.headers.get(http::header::ORIGIN).and_then(|v| v.to_str().ok()) {
            None => Ok(ValidOrigin(String::new())),  // CLI/synapse — no origin check
            Some(o) if allowed.contains(&o) => Ok(ValidOrigin(o.to_string())),
            Some(o) => Err(PlexusError::Unauthenticated(
                format!("Origin '{}' is not allowed", o)
            )),
        }
    }
}

/// Simple optional extractor — just reads the Origin header, no validation.
pub fn extract_origin(ctx: &RequestContext) -> Option<String> {
    ctx.headers.get(http::header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}
```

### Usage in FormVeritas

```rust
// In any activation that should enforce Origin:
use plexus_transport::extractors::require_allowed_origin;

const ALLOWED_ORIGINS: &[&str] = &[
    "https://app.formveritas.com",
    "http://localhost:5173",
];

#[plexus_macros::hub_method(description = "List clients")]
async fn list(
    &self,
    #[from_request(require_allowed_origin(ALLOWED_ORIGINS))] _origin: ValidOrigin,
    #[from_auth(self.db.validate_user)] user: ValidUser,
    search: Option<String>,
) -> impl Stream<Item = ClientEvent> + Send + 'static {
    // ...
}
```

The `_origin: ValidOrigin` parameter is purely for the side-effect of validation. If Origin is disallowed, the method never executes — the `?` in the generated code propagates the error as `-32001`.

### Configuration via env var (FormVeritas)

For the allowlist to be runtime-configurable (not hardcoded as a `&'static [&str]`), use a lazy static or thread-local:

```rust
// src/auth/origin.rs
use std::sync::OnceLock;

static ALLOWED_ORIGINS: OnceLock<Vec<String>> = OnceLock::new();

pub fn init_allowed_origins(origins: Vec<String>) {
    ALLOWED_ORIGINS.set(origins).ok();
}

pub fn check_origin(ctx: &RequestContext) -> Result<(), PlexusError> {
    let allowed = ALLOWED_ORIGINS.get().map(|v| v.as_slice()).unwrap_or(&[]);
    if allowed.is_empty() { return Ok(()); }  // not configured — skip check
    // ... same logic ...
}
```

## Files

- `plexus-transport/src/extractors/mod.rs` — new module, re-export extractors
- `plexus-transport/src/extractors/origin.rs` — `extract_origin`, `require_allowed_origin`, `ValidOrigin`
- `plexus-transport/src/extractors/cookies.rs` — `extract_cookie(name)`, `CookieJar`
- `plexus-transport/src/extractors/network.rs` — `extract_peer_addr`, `extract_uri`
- `plexus-transport/src/lib.rs` — `pub mod extractors`
- `FormVeritasV2/src/main.rs` — call `init_allowed_origins` from `ALLOWED_ORIGINS` env var

## Acceptance Criteria

- [ ] Request with `Origin: https://evil.com` to a method using `require_allowed_origin` returns `-32001`
- [ ] Request with `Origin: http://localhost:5173` succeeds
- [ ] Request with no `Origin` (synapse CLI, integration tests) succeeds
- [ ] Extractor unit tests work against `RequestContext { headers: HeaderMap::new(), .. }` with no HTTP server
- [ ] Playwright tests unaffected (browser sends correct origin)
