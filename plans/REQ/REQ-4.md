# REQ-4: Hub-Level Request Declaration

**blocked_by:** [REQ-2]
**unlocks:** [REQ-5]

## Status: Planned

## Goal

Declare the request shape once on the hub, not on every method. The `request = Type` argument to `hub_methods` defines the security posture and extraction contract for the entire activation — readable at a glance, enforced uniformly, visible in the wire schema.

## Problem

With REQ-2, a method that needs auth + origin checking still requires per-method `#[from_request]` annotations. With a hub serving 5-10 methods that all share the same request shape, that's repetitive. More importantly, forgetting `#[from_request] auth_token: String` on a new method produces silent misbehavior: the method runs with no auth gate at all.

The hub annotation should declare what every method in the hub sees on upgrade. Methods opt into individual fields via `#[from_request]` only if they need the value — but the extraction and validation happen uniformly, before any method runs.

## Shape

```rust
// Defined once — the request shape for this hub
#[derive(PlexusRequest, schemars::JsonSchema)]
struct ClientsRequest {
    /// JWT from Keycloak auth flow
    #[from_cookie("access_token")]
    auth_token: String,           // required: extraction failure = every method in hub fails

    origin: ValidOrigin,          // ValidOrigin::extract_from_raw validates allowlist

    #[from_peer]
    peer_addr: Option<SocketAddr>,

    #[from_auth_context]
    auth: Option<AuthContext>,    // populated by CombinedAuthMiddleware
}

#[hub_methods(
    namespace = "clients",
    version = "1.0.0",
    request = ClientsRequest,    // ← hub-level default; all methods use this
)]
impl ClientsActivation {
    async fn list(
        &self,
        #[from_request] auth_token: String,              // opt into auth_token field
        #[from_auth(self.db.validate_user)] user: ValidUser,   // stage-3 sugar unchanged
        search: Option<String>,
    ) -> impl Stream<Item = ClientEvent> + Send + 'static { ... }

    async fn get(
        &self,
        #[from_request] auth_token: String,
        id: String,
    ) -> impl Stream<Item = ClientEvent> + Send + 'static { ... }

    // Public method within an authenticated hub:
    #[hub_method(request = ())]
    async fn health(&self) -> impl Stream<Item = String> { stream! { yield "ok".into(); } }
}
```

## Semantics

### Hub-level extraction

Before any method in the hub is dispatched:

1. `ClientsRequest::extract(&raw_ctx)?` is called
2. If extraction fails (e.g. `auth_token` cookie absent, or `ValidOrigin` validation fails), the error is returned immediately — the method body never runs
3. The extracted struct is held for the duration of the dispatch call

Methods then use `#[from_request] field_name: Type` to pull values from the struct. Methods that don't use `#[from_request]` are still protected by extraction failure — they just don't receive any field values.

This means `auth_token: String` on `ClientsRequest` is effectively a hub-wide auth gate. No per-method `#[from_request] auth_token` needed to get the protection — but methods that need the token value must opt in.

### Per-method override: `#[hub_method(request = ())]`

Use `()` as the request type for public methods within an otherwise-authenticated hub:

```rust
#[hub_method(request = ())]
async fn health(&self) -> impl Stream<Item = String> { ... }
```

This tells the macro to skip `ClientsRequest::extract` for `health` and call it directly. The method cannot use `#[from_request]` (there's no struct to pull from), but it's callable without any cookies.

Custom per-method request type is also allowed: `#[hub_method(request = PublicRequest)]` where `PublicRequest: PlexusRequest`.

### Validators are gone

The old `validate = [require_authenticated, require_cors(ALLOWED_ORIGINS)]` syntax is replaced by the request struct. Validators are not a separate concept:

- `require_authenticated` → `auth_token: String` on the struct (extraction fails if absent)
- `require_cors(ALLOWED_ORIGINS)` → `origin: ValidOrigin` on the struct (`ValidOrigin::extract_from_raw` validates)

Both cases: non-optional field + typed extraction logic = the same guarantee a validator provided, expressed as data.

### Extractors are gone

The old `extract = [peer_addr: Option<SocketAddr> = extract_peer_addr]` syntax is replaced by struct fields with source annotations:

- `#[from_peer] peer_addr: Option<SocketAddr>` on the struct

No separate extractor registry or `#[from_hub]` opt-in name needed. Methods opt in by declaring `#[from_request] peer_addr: Option<SocketAddr>`.

## Codegen

The `hub_methods` macro receives `request = ClientsRequest` and generates a dispatch wrapper per method:

```rust
async fn __dispatch_list(
    &self,
    params: Value,
    raw_ctx: Option<Arc<RawRequestContext>>,
) -> PlexusStream {
    // Stage 1: extract request struct (hub-level default)
    let ctx = raw_ctx.as_deref().ok_or(PlexusError::Unauthenticated("no request context".into()))?;
    let req = ClientsRequest::extract(ctx)?;   // extraction failure short-circuits here

    // Stage 2: inject #[from_request] fields
    let auth_token: String = req.auth_token.clone();    // #[from_request] auth_token

    // Stage 3: resolve #[from_auth] params
    let auth_ref = req.auth.as_ref()
        .ok_or_else(|| PlexusError::Unauthenticated("Authentication required".to_string()))?;
    let user: ValidUser = self.db.validate_user(auth_ref).await
        .map_err(|e| PlexusError::ExecutionError(e.to_string()))?;

    // Stage 4: deserialize normal RPC params
    let search: Option<String> = from_params(&params, "search")?;

    // Stage 5: call method
    self.list(auth_token, user, search).await
}

// Public method with #[hub_method(request = ())] — no extraction
async fn __dispatch_health(
    &self,
    params: Value,
    _raw_ctx: Option<Arc<RawRequestContext>>,
) -> PlexusStream {
    self.health().await
}
```

## Schema

The `plugin_schema()` method now includes a `request` field — the JSON Schema of the hub's request struct:

```rust
fn plugin_schema(&self) -> PluginSchema {
    PluginSchema {
        namespace: "clients",
        version: "1.0.0",
        // ...methods...
        request: Some(serde_json::to_value(schemars::schema_for!(ClientsRequest)).unwrap()),
    }
}
```

The `request` field is an opaque `Value` in `PluginSchema` — it's the full `schemars::schema_for!(T)` output including `x-plexus-source` extensions. Synapse (REQ-5) reads this directly; no custom Haskell types needed to represent the Rust struct shape.

Wire example:

```json
{
  "namespace": "clients",
  "version": "1.0.0",
  "request": {
    "type": "object",
    "properties": {
      "auth_token": {
        "type": "string",
        "description": "JWT from Keycloak auth flow",
        "x-plexus-source": { "from": "cookie", "key": "access_token" }
      },
      "origin": {
        "type": "string",
        "x-plexus-source": { "from": "header", "key": "origin" }
      },
      "peer_addr": {
        "type": "string",
        "x-plexus-source": { "from": "derived" }
      },
      "auth": {
        "x-plexus-source": { "from": "derived" }
      }
    },
    "required": ["auth_token", "origin"]
  },
  "methods": { ... }
}
```

`ContractEntry`, `ContractSource`, `SecurityExtractor`, and `PluginSecurity.validators` are NOT in the schema — they are replaced by this JSON Schema blob.

## Files

| File | Repo | Change |
|------|------|--------|
| `plexus-macros/src/parse.rs` | plexus-macros | Parse `request = Type` in `hub_methods` attr; parse `#[hub_method(request = Type)]` override |
| `plexus-macros/src/codegen/activation.rs` | plexus-macros | Generate `RequestType::extract(ctx)?` at top of each dispatch; skip for `request = ()` methods |
| `plexus-macros/src/codegen/schema.rs` | plexus-macros | Include `schemars::schema_for!(RequestType)` in `plugin_schema()` output |
| `plexus-core/src/schema.rs` | plexus-core | Add `request: Option<Value>` to `PluginSchema` Rust struct |
| `plexus-transport/src/request/mod.rs` | plexus-transport | `PlexusRequest` trait; hub dispatch infrastructure |
| `FormVeritasV2/src/auth/clients_request.rs` | FormVeritas | `ClientsRequest` struct (new file) |
| `FormVeritasV2/src/activations/clients/activation.rs` | FormVeritas | Add `request = ClientsRequest` to hub; remove per-method validator/extractor annotations |

## Acceptance Criteria

- [ ] Hub with `request = ClientsRequest` where `auth_token: String` rejects unauthenticated calls (no cookie) for every method without per-method annotations
- [ ] Hub with `origin: ValidOrigin` in request struct rejects wrong-Origin calls for every method
- [ ] `#[hub_method(request = ())]` makes a specific method public within an authenticated hub
- [ ] `#[from_request] auth_token: String` in a method body injects the correct value
- [ ] Method schema includes `request` field (JSON Schema of the request struct)
- [ ] `required` in the `request` schema matches non-Option fields on `ClientsRequest`
- [ ] `x-plexus-source` annotations present in the `request` schema
- [ ] Existing hubs with no `request` annotation compile unchanged
- [ ] `cargo test` passes in plexus-macros and FormVeritas

## Tests

### Compile tests — `plexus-macros/tests/compile/` (trybuild)

**`hub_request_type.rs`** — must compile:
```rust
#[derive(PlexusRequest, schemars::JsonSchema)]
struct TestRequest {
    #[from_cookie("access_token")]
    auth_token: String,
}

#[hub_methods(namespace = "test", request = TestRequest)]
impl TestHub {
    async fn list(&self) -> impl Stream<Item = String> { stream! { yield "ok".into(); } }
    async fn get(&self, id: String) -> impl Stream<Item = String> { stream! { yield id; } }
}
// Neither method uses #[from_request] but both are protected by extraction
```

**`hub_request_public_override.rs`** — must compile:
```rust
#[hub_methods(namespace = "test", request = TestRequest)]
impl TestHub {
    async fn protected(&self, #[from_request] auth_token: String) -> ... { ... }

    #[hub_method(request = ())]
    async fn health(&self) -> impl Stream<Item = String> { stream! { yield "ok".into(); } }
}
```

**`hub_no_request_unchanged.rs`** — existing hub with no `request` arg must compile identically:
```rust
#[hub_methods(namespace = "test", version = "1.0.0")]
impl TestHub {
    async fn plain(&self, x: i32) -> impl Stream<Item = i32> { stream! { yield x; } }
}
```

**`hub_request_field_type_mismatch.rs`** — must FAIL:
```rust
// TestRequest has auth_token: String
#[hub_methods(namespace = "test", request = TestRequest)]
impl TestHub {
    async fn bad(&self, #[from_request] auth_token: u32) -> ... { ... }
    // Expected: type error — auth_token is String, not u32
}
```

### Unit — dispatch tests (plexus-macros/tests/hub_dispatch.rs)

Using test server with `TestSessionValidator`:

```
// Extraction gate:
// Hub uses TestRequest with auth_token: String (required cookie)
// Unauthenticated request (no cookie) → any method returns -32001 before body runs
// Authenticated request (cookie present) → method executes normally

// ValidOrigin gate:
// Hub uses request struct with origin: ValidOrigin
// Disallowed Origin → extraction fails → method returns -32001 with "Origin" in message
// No Origin (CLI path) → extraction succeeds (ValidOrigin("")) → method runs

// Per-method public override:
// Hub has request = TestRequest with required auth_token
// health() has #[hub_method(request = ())]
// Call health() without cookie → succeeds (skip extraction)
// Call list() without cookie → fails (extraction runs, cookie absent)

// Field injection:
// Method has #[from_request] auth_token: String
// Client sends cookie access_token=tok123
// Method receives auth_token == "tok123"
```

### Unit — schema output

```rust
#[test] fn plugin_schema_includes_request_field() {
    let hub = TestHub::new();
    let schema = hub.plugin_schema();
    let request = schema.request.expect("request schema should be present");
    let props = &request["properties"];
    assert!(props["auth_token"].is_object());
    let source = &props["auth_token"]["x-plexus-source"];
    assert_eq!(source["from"], "cookie");
    assert_eq!(source["key"], "access_token");
}

#[test] fn plugin_schema_required_matches_non_option_fields() {
    let hub = TestHub::new();
    let schema = hub.plugin_schema();
    let required = schema.request.unwrap()["required"].as_array().unwrap().clone();
    let names: Vec<&str> = required.iter().map(|v| v.as_str().unwrap()).collect();
    assert!(names.contains(&"auth_token"));
    assert!(!names.contains(&"peer_addr"));   // Option<SocketAddr> not required
}
```

### Integration — FormVeritas ClientsActivation

After applying REQ-4 to `ClientsActivation` (removing old `validate`/`extract` from hub annotation, adding `request = ClientsRequest`):

```
// All 99 Playwright tests must still pass
// Unauthenticated websocat call to clients.list → -32001
// websocat with wrong Origin → -32001 with "Origin" in message
// clients hub schema includes "request" JSON Schema blob with x-plexus-source fields
// No per-method auth or origin annotation needed on any of the 5 client methods
```
