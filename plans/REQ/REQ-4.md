# REQ-4: Hub-Level Validators and Extractors

**blocked_by:** [REQ-2]
**unlocks:** []

## Status: Planned

## Goal

Declare validators and extractors once on the hub, not on every method. The hub annotation becomes the security posture statement for the entire activation — readable at a glance, enforced uniformly.

## Problem

With REQ-2, a method that needs auth + origin checking looks like:

```rust
async fn list(
    &self,
    #[from_request(require_allowed_origin(ALLOWED_ORIGINS))] _: ValidOrigin,
    #[from_auth(self.db.validate_user)] user: ValidUser,
    search: Option<String>,
) -> ...

async fn get(
    &self,
    #[from_request(require_allowed_origin(ALLOWED_ORIGINS))] _: ValidOrigin,
    #[from_auth(self.db.validate_user)] user: ValidUser,
    id: String,
) -> ...
```

Every method in the hub repeats the same two extraction lines. It's noisy, and if you add a new method and forget either annotation you silently have an unprotected endpoint.

The security properties of the hub should be declared once at the hub level, not scattered across every method.

## Shape

Two distinct concepts on the hub annotation:

**Validators** — pure gate, no value produced. Every method must pass these or the call is rejected before the method body runs. They are invisible to method parameters.

**Extractors** — produce a typed value from `RequestContext`. The value is computed once per call at hub dispatch time and made available to methods that opt in via `#[from_hub(name)]`.

```rust
#[plexus_macros::hub_methods(
    namespace = "clients",
    version = "1.0.0",
    description = "Client management",

    // Validators: run for every method, must return Ok(()) or the call fails
    validate = [
        require_cors(ALLOWED_ORIGINS),   // rejects wrong Origin
        require_authenticated,           // rejects missing auth
    ],

    // Extractors: run for every method, value available via #[from_hub]
    extract = [
        peer_addr: Option<SocketAddr> = extract_peer_addr,
        origin:    Option<String>     = extract_origin,
    ],
)]
impl ClientsActivation {
    // Every method in this hub is:
    //   1. Gated by require_cors + require_authenticated — no boilerplate needed
    //   2. Has peer_addr and origin available to opt into

    async fn list(
        &self,
        #[from_hub] peer_addr: Option<SocketAddr>,  // opt-in to hub extractor
        #[from_auth(self.db.validate_user)] user: ValidUser,
        search: Option<String>,
    ) -> ...

    async fn get(
        &self,
        // No #[from_hub] — doesn't need peer_addr or origin
        #[from_auth(self.db.validate_user)] user: ValidUser,
        id: String,
    ) -> ...
}
```

## Semantics

### Validators

- Signature: `Fn(&RequestContext) -> Result<(), PlexusError>`
- Run in declaration order before any extractor or method body
- First failure short-circuits: method returns that error, nothing else runs
- Not visible as method parameters — they are purely side-effect gates
- Example builtins: `require_authenticated`, `require_cors(origins)`, `require_role(role)`

### Extractors

- Signature: `Fn(&RequestContext) -> T` (infallible) or `Fn(&RequestContext) -> Result<T, PlexusError>` (fallible)
- Run after all validators pass
- Produce a named typed value stored in a per-call hub context
- Methods opt in via `#[from_hub(name)]` — the param type must match the declared extractor type
- Methods that don't use `#[from_hub]` pay no cost — the extractor still runs (cheap header reads) but the value is dropped

### `require_authenticated` builtin

A simple validator that checks `ctx.auth.is_some()`:

```rust
pub fn require_authenticated(ctx: &RequestContext) -> Result<(), PlexusError> {
    ctx.auth.as_ref()
        .map(|_| ())
        .ok_or_else(|| PlexusError::Unauthenticated("Authentication required".into()))
}
```

This makes `require_authenticated` on the hub equivalent to having `#[from_auth]` on every method, but without requiring a concrete user type — useful for hubs where some methods do their own auth resolution.

## Codegen

The `hub_methods` macro receives the `validate` and `extract` lists and generates a hub dispatch wrapper:

```rust
// Generated around every method dispatch:
async fn __dispatch_list(&self, params: Value, req_ctx: Option<Arc<RequestContext>>) -> PlexusStream {
    // 1. Run validators
    let ctx = req_ctx.as_deref();
    require_cors(ALLOWED_ORIGINS)(ctx)?;
    require_authenticated(ctx)?;

    // 2. Run extractors
    let hub_peer_addr: Option<SocketAddr> = extract_peer_addr(ctx);
    let hub_origin: Option<String> = extract_origin(ctx);

    // 3. Deserialize method params and call method
    let search: Option<String> = ...;
    self.list(hub_peer_addr, user, search).await
    //         ^^^^^^^^^^^^^^^^^^^^^^^^^
    //         #[from_hub] params injected automatically
}
```

Methods with `#[from_hub(name)]` receive the pre-extracted value. Methods without it don't receive it (the value is computed but not passed).

### Request Contract Inference

For each extractor in the `extract = [...]` list, the macro also infers a `ContractEntry` (see REQ-1 Layer 3b) to include in the wire schema. This tells clients what HTTP-level data the hub needs on WS upgrade — NOT the Rust type the extractor produces.

```rust
// The macro looks up the extractor function name in the stdlib registry to determine
// ContractSource. Unknown functions default to Derived.
fn infer_contract(extractor_fn: &str, extract_key: Option<&str>) -> ContractEntry {
    match extractor_fn {
        "extract_origin" | "require_allowed_origin" =>
            ContractEntry { source: Header, key: Some("origin".into()), required: false, .. }
        "extract_peer_addr" =>
            ContractEntry { source: Derived, key: None, required: false, .. }
        "extract_cookie" =>
            ContractEntry { source: Cookie, key: extract_key.map(Into::into), required: false, .. }
        "extract_header" =>
            ContractEntry { source: Header, key: extract_key.map(Into::into), required: false, .. }
        _ =>
            ContractEntry { source: Derived, key: None, required: false,
                            description: format!("server-derived: {}", extractor_fn) }
    }
}
```

Validators are NOT included in the request contract. They are pure gates. However, `require_authenticated` implies that the `access_token` cookie must be present — the macro emits a `ContractEntry { source: Cookie, key: Some("access_token"), required: true }` when it sees `require_authenticated` in the `validate` list. This is the one exception where a validator drives a contract entry, because the cookie is what auth validation checks.

The serialized wire schema for `security` becomes:

```json
{
  "security": {
    "validators": [
      {"name": "require_authenticated"},
      {"name": "require_cors", "params": ["https://app.formveritas.com"]}
    ],
    "request_contract": [
      {"source": "Cookie", "key": "access_token", "required": true,  "description": "JWT auth token"},
      {"source": "Derived", "key": null,           "required": false, "description": "server-derived: extract_peer_addr"},
      {"source": "Header",  "key": "origin",       "required": false, "description": "Origin header"}
    ]
  }
}
```

Note: `seTypeName` / Rust type strings do NOT appear in the wire schema. The schema says "I need a cookie named `access_token`", not "I produce a value of type `Option<AuthContext>`". Type names are a server-side implementation detail.

## Why Not Just Middleware

Middleware runs at the transport layer — before routing, before method dispatch, with no knowledge of which activation or method is being called. Hub-level validators run after routing, inside the activation, with full knowledge of the hub's identity and configuration. This means:

- Different hubs can have different Origin allowlists
- An internal admin hub can skip CORS entirely (no browser access)
- A public read-only hub can skip auth
- Validators can reference `self` (e.g. `self.config.allowed_origins`)

## Relation to Existing `#[from_auth]`

`#[from_auth(expr)]` on individual methods continues to work and is not replaced. Hub-level `validate = [require_authenticated]` + method-level `#[from_auth(self.db.validate_user)]` are complementary:

- Hub validator gates the call (rejects unauthenticated requests early)
- Method `#[from_auth]` resolves the AuthContext into a domain type (ValidUser)

A hub can use both: the validator provides a uniform early rejection, the per-method extractor provides a typed domain object.

## Files

| File | Repo | Change |
|------|------|--------|
| `plexus-macros/src/parse.rs` | plexus-macros | Parse `validate = [...]` and `extract = [...]` in `hub_methods` attr |
| `plexus-macros/src/codegen/activation.rs` | plexus-macros | Generate validator/extractor dispatch wrapper; call `infer_contract` per extractor |
| `plexus-macros/src/codegen/hub.rs` | plexus-macros | Hub-level context struct holding extracted values |
| `plexus-transport/src/extractors/validators.rs` | plexus-transport | `require_authenticated`, `require_cors`, `require_role` |
| `plexus-transport/src/extractors/contract.rs` | plexus-transport | `ContractEntry`, `ContractSource`, stdlib extractor → ContractEntry mapping |
| `plexus-transport/src/lib.rs` | plexus-transport | Re-export validators, `ContractEntry`, `ContractSource` |

## Acceptance Criteria

- [ ] Hub with `validate = [require_authenticated]` — every method in the hub rejects unauthenticated calls without any per-method annotation
- [ ] Hub with `validate = [require_cors(ORIGINS)]` — wrong-Origin calls rejected at every method
- [ ] Hub with `extract = [peer_addr: Option<SocketAddr> = extract_peer_addr]` — methods that declare `#[from_hub] peer_addr: Option<SocketAddr>` receive the value
- [ ] Methods that do not declare `#[from_hub]` compile and run correctly — hub extractors don't affect their signatures
- [ ] A hub can declare both validators and extractors simultaneously
- [ ] Existing hubs with no `validate`/`extract` annotations are unaffected (backward compatible)
- [ ] Hub with `extract = [peer_addr: Option<SocketAddr> = extract_peer_addr]` serializes as `ContractEntry { source: Derived }` in the wire schema — NOT as `{ type_name: "Option<SocketAddr>" }`
- [ ] Hub with `validate = [require_authenticated]` serializes a `ContractEntry { source: Cookie, key: "access_token", required: true }` in `request_contract`
- [ ] `cargo test` passes in plexus-macros

## Tests

### Compile tests — `plexus-macros/tests/compile/` (trybuild)

**`hub_validate_compiles.rs`** — must compile:
```rust
#[hub_methods(namespace = "test", validate = [require_authenticated])]
impl TestHub {
    async fn list(&self) -> impl Stream<Item = String> { stream! { yield "ok".into(); } }
    async fn get(&self, id: String) -> impl Stream<Item = String> { stream! { yield id; } }
}
// Neither method has #[from_auth] — auth comes entirely from hub validator
```

**`hub_extract_compiles.rs`** — must compile:
```rust
#[hub_methods(namespace = "test",
    extract = [peer_addr: Option<SocketAddr> = extract_peer_addr])]
impl TestHub {
    async fn with_peer(&self, #[from_hub] peer_addr: Option<SocketAddr>) -> ... { ... }
    async fn without_peer(&self, name: String) -> ... { ... }  // no #[from_hub] — must still compile
}
```

**`hub_validate_and_extract_compiles.rs`** — must compile:
```rust
#[hub_methods(namespace = "test",
    validate = [require_authenticated, require_cors(ALLOWED)],
    extract  = [origin: Option<String> = extract_origin])]
impl TestHub {
    async fn method(&self, #[from_hub] origin: Option<String>) -> ... { ... }
}
```

**`hub_no_annotations_unchanged.rs`** — existing hub with no validate/extract must compile identically to before:
```rust
#[hub_methods(namespace = "test", version = "1.0.0", description = "...")]
impl TestHub {
    async fn plain(&self, x: i32) -> impl Stream<Item = i32> { stream! { yield x; } }
}
```

### Unit — validator ordering and short-circuit (`plexus-macros/tests/hub_dispatch.rs`)

Use a test server with `TestSessionValidator`.

```
// Validator short-circuit:
// Hub declares validate = [require_authenticated, require_cors(ALLOWED)]
// Unauthenticated request with allowed Origin:
//   → require_authenticated fires first → returns -32001
//   → require_cors never runs (it would have passed)
// Verify: error is -32001 (not a CORS error)

// Both validators pass:
// Authenticated request with allowed Origin → method body executes → returns data

// Second validator fails:
// Authenticated request with disallowed Origin
//   → require_authenticated passes
//   → require_cors fires → returns -32001 with "Origin" in message

// Validator failure before extractor:
// Hub declares validate = [require_authenticated], extract = [peer_addr = extract_peer_addr]
// Unauthenticated request → validator fails → extractor is never called
// Verify: no panic, no partial extraction
```

### Unit — extractor opt-in behaviour

```
// Hub declares extract = [peer_addr: Option<SocketAddr> = extract_peer_addr]
// Method A declares #[from_hub] peer_addr: Option<SocketAddr>
// Method B declares no #[from_hub]

// Call method A from client at 127.0.0.1:9999:
//   → peer_addr == Some(127.0.0.1:9999)

// Call method B:
//   → method body runs normally, no peer_addr param, no error

// Hub extractor failure:
// Hub declares extract = [user_data: UserData = fallible_extractor] where fallible_extractor returns Err
// Call any method → method returns the extractor's error before body runs
// Verify: error has correct PlexusError variant and message
```

### Unit — `#[from_hub]` type mismatch (compile error)

**`hub_extract_type_mismatch.rs`** — must FAIL to compile with a clear error:
```rust
// Hub declares:  extract = [peer_addr: Option<SocketAddr> = extract_peer_addr]
// Method declares: #[from_hub] peer_addr: String  ← wrong type
// Expected error: type mismatch, expected Option<SocketAddr>, found String
```

### Unit — request contract inference

```rust
// Hub with extract = [peer_addr: Option<SocketAddr> = extract_peer_addr]
// Verify the serialized schema security.request_contract contains:
//   { "source": "Derived", "key": null, "required": false }
// And does NOT contain any Rust type string.

// Hub with validate = [require_authenticated]
// Verify the serialized schema security.request_contract contains:
//   { "source": "Cookie", "key": "access_token", "required": true }

// Hub with extract = [origin: Option<String> = extract_origin]
// Verify the serialized schema security.request_contract contains:
//   { "source": "Header", "key": "origin", "required": false }
```

### Integration — FormVeritas ClientsActivation

After applying REQ-4 to `ClientsActivation`:

```
// Remove all per-method #[from_request(require_allowed_origin)] annotations
// Add to hub annotation: validate = [require_authenticated, require_cors(ALLOWED_ORIGINS)]
// Run all 99 Playwright tests → must still pass
// Run: unauthenticated websocat call to clients.list → -32001
// Run: websocat with wrong Origin → -32001 with "Origin" in message
// Verify: no per-method auth annotation needed on any of the 5 client methods
// Verify: clients hub schema request_contract shows Cookie access_token (required)
```
