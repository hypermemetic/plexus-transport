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

## Tests

### Unit — `plexus-transport/tests/extractors/origin.rs`

All tests use `make_ctx` from REQ-1 tests — no server, no WS.

```rust
const ALLOWED: &[&str] = &["https://app.example.com", "http://localhost:5173"];

// Allowlisted origin → Ok
#[test] fn allowed_origin_passes() {
    let ctx = make_ctx(&[("origin", "https://app.example.com")]);
    assert!(require_allowed_origin(ALLOWED)(&ctx).is_ok());
}

// Second entry in allowlist also passes
#[test] fn second_allowed_origin_passes() {
    let ctx = make_ctx(&[("origin", "http://localhost:5173")]);
    assert!(require_allowed_origin(ALLOWED)(&ctx).is_ok());
}

// Origin not in list → Err with -32001 semantics
#[test] fn disallowed_origin_fails() {
    let ctx = make_ctx(&[("origin", "https://evil.com")]);
    let err = require_allowed_origin(ALLOWED)(&ctx).unwrap_err();
    assert!(matches!(err, PlexusError::Unauthenticated(_)));
    assert!(err.to_string().contains("evil.com"));
}

// No Origin header (CLI/synapse) → Ok with empty ValidOrigin
#[test] fn no_origin_passes() {
    let ctx = make_ctx(&[]);
    assert!(require_allowed_origin(ALLOWED)(&ctx).is_ok());
}

// Empty allowlist → skip check entirely, all origins pass
#[test] fn empty_allowlist_passes_everything() {
    let ctx = make_ctx(&[("origin", "https://anything.com")]);
    assert!(require_allowed_origin(&[])(&ctx).is_ok());
}

// extract_origin — reads header without validation
#[test] fn extract_origin_reads_header() {
    let ctx = make_ctx(&[("origin", "https://app.example.com")]);
    assert_eq!(extract_origin(&ctx), Some("https://app.example.com".into()));
}
#[test] fn extract_origin_none_when_absent() {
    assert_eq!(extract_origin(&make_ctx(&[])), None);
}
```

### Integration — method-level enforcement

Using a test server with one protected method (`list`) and one unprotected method (`health`):

```
// Case 1: WS upgrade with Origin: https://app.example.com → list() returns data
// Case 2: WS upgrade with Origin: https://evil.com → list() returns -32001
//         error message must contain "Origin" and the disallowed value
// Case 3: WS upgrade with no Origin header → list() returns data (CLI path)
// Case 4: health() called with disallowed Origin → succeeds (no origin check on this method)
//         confirms extraction is per-method, not global
```

### Integration — Playwright (uscis-web)

```
// All 99 existing tests must continue to pass — browser sends correct Origin
// Add one test: simulated wrong-origin connection
//   page.evaluate(() => new WebSocket('ws://127.0.0.1:44902?token=...', [], {origin: 'https://evil.com'}))
//   assert WebSocket closes immediately or first RPC call returns -32001
```

### Unit — `init_allowed_origins` OnceLock (FormVeritas)

```rust
#[test] fn init_allowed_origins_configures_check() {
    init_allowed_origins(vec!["http://localhost:5173".into()]);
    let ctx_ok  = make_ctx(&[("origin", "http://localhost:5173")]);
    let ctx_bad = make_ctx(&[("origin", "https://evil.com")]);
    assert!(check_origin(&ctx_ok).is_ok());
    assert!(check_origin(&ctx_bad).is_err());
}

#[test] fn uninitialised_allows_all() {
    // Before init_allowed_origins is called, check_origin returns Ok for anything
    let ctx = make_ctx(&[("origin", "https://anything.com")]);
    assert!(check_origin(&ctx).is_ok());
}
```
