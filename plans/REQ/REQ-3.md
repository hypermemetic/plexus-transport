# REQ-3: Origin Validation via Request Struct Field Type

**blocked_by:** [REQ-2]
**unlocks:** []

## Status: Planned

## Goal

Implement AUTH-18 (Origin header validation) using the `PlexusRequest` derive (REQ-1). Origin validation is not a middleware concern and not a standalone extractor function — it's encoded in the **type** of a request struct field. A `ValidOrigin` newtype implements `PlexusRequestField`, carrying the validation logic. Using `ValidOrigin` instead of `String` for the origin field means the allowlist check runs automatically at extraction time.

## Why Not Middleware, and Why Not a Standalone Extractor

The original REQ-1 design used `#[activation_param(require_allowed_origin(ALLOWED_ORIGINS))] _: ValidOrigin` — a standalone extractor function passed as an expression argument. That design is gone. With `PlexusRequest` derive, the approach is different: the struct field type itself carries the validation logic. This is consistent with how Axum extractors work and easier to compose.

## Shape

### `PlexusRequestField` trait (plexus-transport)

A trait for newtype field types that carry their own extraction + validation logic:

```rust
// src/request/derive.rs
pub trait PlexusRequestField: Sized {
    fn extract_from_raw(ctx: &RawRequestContext) -> Result<Self, PlexusError>;
}
```

The `PlexusRequest` derive calls `T::extract_from_raw(ctx)` for any field whose type implements `PlexusRequestField` and has no explicit annotation. Primitive types (`String`, `Option<String>`, etc.) use the field annotation instead.

### `ValidOrigin` (plexus-transport)

```rust
// src/request/origin.rs
pub struct ValidOrigin(pub String);

impl PlexusRequestField for ValidOrigin {
    fn extract_from_raw(ctx: &RawRequestContext) -> Result<Self, PlexusError> {
        // Reads origin from headers and validates against the configured allowlist.
        // None origin → Ok(ValidOrigin("")) — non-browser client (CLI, synapse), pass through
        // Origin in allowlist → Ok(ValidOrigin(origin))
        // Origin not in allowlist → Err(PlexusError::Unauthenticated(...))
        let allowed = ALLOWED_ORIGINS.get().map(|v| v.as_slice()).unwrap_or(&[]);
        match ctx.headers.get(http::header::ORIGIN).and_then(|v| v.to_str().ok()) {
            None => Ok(ValidOrigin(String::new())),
            Some(o) if allowed.is_empty() || allowed.contains(&o) => Ok(ValidOrigin(o.to_string())),
            Some(o) => Err(PlexusError::Unauthenticated(format!("Origin '{}' is not allowed", o))),
        }
    }
}

static ALLOWED_ORIGINS: OnceLock<Vec<String>> = OnceLock::new();

pub fn init_allowed_origins(origins: Vec<String>) {
    ALLOWED_ORIGINS.set(origins).ok();
}
```

No arguments to the type — the allowlist is configured globally at startup via `init_allowed_origins`. The `ValidOrigin` type needs no parameters; it's self-contained.

### Usage in the request struct

```rust
// FormVeritas: src/auth/clients_request.rs
#[derive(PlexusRequest, schemars::JsonSchema)]
struct ClientsRequest {
    #[from_cookie("access_token")]
    auth_token: String,              // required: missing → call fails

    origin: Option<ValidOrigin>,     // uses ValidOrigin's extraction + validation logic
                                     // None if no origin (CLI path); Err if origin disallowed
    #[from_peer]
    peer_addr: Option<SocketAddr>,
}
```

The `PlexusRequest` derive sees `Option<ValidOrigin>` with no explicit annotation. Since `ValidOrigin: PlexusRequestField`, the derive generates `let origin = ValidOrigin::extract_from_raw(ctx).ok();` (wrapped in Option). Validation failure on `Option<ValidOrigin>` still propagates as `Err` — the `Option` only affects the None-when-absent path, not the validation-failure path.

Wait — this needs to be explicit. The derive rules for `PlexusRequestField` types:

- `field: ValidOrigin` (non-optional) — `ValidOrigin::extract_from_raw(ctx)?` — validation failure = call fails
- `field: Option<ValidOrigin>` — this doesn't mean "skip validation if origin is present". It means the field is optional only in the sense that missing origin → `Ok(None)`. If origin is present but invalid, extraction still returns `Err`. To express this, `ValidOrigin::extract_from_raw` returns `Ok(ValidOrigin(""))` for None origin — so wrapping in `Option` would need a different convention. Recommendation: use `field: ValidOrigin` (non-Option) since `ValidOrigin::extract_from_raw` already handles the None-origin case gracefully.

Revised usage:

```rust
#[derive(PlexusRequest, schemars::JsonSchema)]
struct ClientsRequest {
    #[from_cookie("access_token")]
    auth_token: String,

    origin: ValidOrigin,    // non-optional; but ValidOrigin::extract_from_raw returns Ok for absent origin
    #[from_peer]
    peer_addr: Option<SocketAddr>,
}
```

`origin` appears in `required` in the JSON Schema, but in practice extraction never fails for absent origin — only for present-but-disallowed origin. This is correct.

### FormVeritas startup

```rust
// src/main.rs
fn main() {
    let allowed: Vec<String> = std::env::var("ALLOWED_ORIGINS")
        .unwrap_or_default()
        .split(',')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    plexus_transport::request::origin::init_allowed_origins(allowed);
    // ...
}
```

## Files

| File | Repo | Change |
|------|------|--------|
| `src/request/origin.rs` | plexus-transport | `ValidOrigin`, `PlexusRequestField` impl, `ALLOWED_ORIGINS`, `init_allowed_origins` |
| `src/request/mod.rs` | plexus-transport | Add `pub mod origin; pub use origin::ValidOrigin;` |
| `src/lib.rs` | plexus-transport | Re-export `ValidOrigin`, `init_allowed_origins` |
| `FormVeritasV2/src/main.rs` | FormVeritas | Call `init_allowed_origins` from `ALLOWED_ORIGINS` env var |
| `FormVeritasV2/src/auth/clients_request.rs` | FormVeritas | `ClientsRequest` struct with `ValidOrigin` field |

Note: `extract_origin` (standalone function), `require_allowed_origin` (standalone extractor factory), and `src/extractors/` module are **not added**. The `ValidOrigin` newtype replaces them entirely.

## Acceptance Criteria

- [ ] Request with `Origin: https://evil.com` to a hub using `ClientsRequest` (with `origin: ValidOrigin`) returns `-32001`
- [ ] Request with `Origin: http://localhost:5173` (in allowlist) succeeds
- [ ] Request with no Origin header (synapse CLI, integration tests) succeeds
- [ ] `ValidOrigin::extract_from_raw` can be unit-tested with a hand-constructed `RawRequestContext` — no server, no WebSocket
- [ ] Playwright tests unaffected (browser sends correct Origin)
- [ ] `ALLOWED_ORIGINS=""` (empty env var) causes `init_allowed_origins` to configure an empty list → all origins pass (safe default)

## Tests

### Unit — `plexus-transport/tests/origin.rs`

All tests use `make_raw` from REQ-1 tests.

```rust
fn setup_allowlist() {
    init_allowed_origins(vec![
        "https://app.example.com".into(),
        "http://localhost:5173".into(),
    ]);
}

#[test] fn allowed_origin_passes() {
    setup_allowlist();
    let ctx = make_raw(&[("origin", "https://app.example.com")], None);
    assert!(ValidOrigin::extract_from_raw(&ctx).is_ok());
}

#[test] fn second_allowed_origin_passes() {
    setup_allowlist();
    let ctx = make_raw(&[("origin", "http://localhost:5173")], None);
    assert!(ValidOrigin::extract_from_raw(&ctx).is_ok());
}

#[test] fn disallowed_origin_fails() {
    setup_allowlist();
    let ctx = make_raw(&[("origin", "https://evil.com")], None);
    let err = ValidOrigin::extract_from_raw(&ctx).unwrap_err();
    assert!(matches!(err, PlexusError::Unauthenticated(_)));
    assert!(err.to_string().contains("evil.com"));
}

#[test] fn no_origin_passes() {
    setup_allowlist();
    let ctx = make_raw(&[], None);
    let result = ValidOrigin::extract_from_raw(&ctx).unwrap();
    assert_eq!(result.0, "");  // empty string sentinel for absent origin
}

#[test] fn empty_allowlist_passes_all_origins() {
    init_allowed_origins(vec![]);
    let ctx = make_raw(&[("origin", "https://anything.com")], None);
    assert!(ValidOrigin::extract_from_raw(&ctx).is_ok());
}
```

### Unit — request struct extraction with `ValidOrigin`

```rust
#[derive(PlexusRequest)]
struct TestRequest {
    #[from_cookie("access_token")]
    auth_token: String,
    origin: ValidOrigin,
}

#[test] fn struct_extraction_fails_on_bad_origin() {
    setup_allowlist();
    let ctx = make_raw(&[
        ("cookie", "access_token=tok"),
        ("origin", "https://evil.com"),
    ], None);
    assert!(TestRequest::extract(&ctx).is_err());
}

#[test] fn struct_extraction_succeeds_on_no_origin() {
    setup_allowlist();
    let ctx = make_raw(&[("cookie", "access_token=tok")], None);
    let req = TestRequest::extract(&ctx).unwrap();
    assert_eq!(req.origin.0, "");
}
```

### Integration — method-level enforcement

Using a test server with one `ClientsRequest`-backed hub and one unprotected hub:

```
// Case 1: WS upgrade with Origin: https://app.example.com → list() returns data
// Case 2: WS upgrade with Origin: https://evil.com → list() returns -32001
//         error message must contain "Origin" and "evil.com"
// Case 3: WS upgrade with no Origin header → list() returns data (CLI path)
// Case 4: unprotected hub called with disallowed Origin → succeeds
//         confirms extraction is per-hub, not global
```

### Integration — Playwright (uscis-web)

```
// All 99 existing tests must continue to pass — browser sends correct Origin automatically
// Add: simulated wrong-origin connection test
//   page.evaluate(() => new WebSocket('ws://127.0.0.1:44902?token=...'))
//   but with Origin spoofed to https://evil.com in WS headers (if browser permits)
//   assert WebSocket call returns -32001
```

### Unit — `init_allowed_origins` (FormVeritas)

```rust
#[test] fn init_configures_check() {
    init_allowed_origins(vec!["http://localhost:5173".into()]);
    let ctx_ok  = make_raw(&[("origin", "http://localhost:5173")], None);
    let ctx_bad = make_raw(&[("origin", "https://evil.com")], None);
    assert!(ValidOrigin::extract_from_raw(&ctx_ok).is_ok());
    assert!(ValidOrigin::extract_from_raw(&ctx_bad).is_err());
}

#[test] fn uninitialised_allows_all() {
    // Before init_allowed_origins is called (OnceLock empty), all origins pass
    let ctx = make_raw(&[("origin", "https://anything.com")], None);
    assert!(ValidOrigin::extract_from_raw(&ctx).is_ok());
}
```
