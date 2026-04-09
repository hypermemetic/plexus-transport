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

- `plexus-macros/src/parse.rs` — parse `validate = [...]` and `extract = [...]` in `hub_methods` attr
- `plexus-macros/src/codegen/activation.rs` — generate validator/extractor dispatch wrapper
- `plexus-macros/src/codegen/hub.rs` — hub-level context struct holding extracted values
- `plexus-transport/src/extractors/validators.rs` — `require_authenticated`, `require_cors`, `require_role`
- `plexus-transport/src/lib.rs` — re-export validators

## Acceptance Criteria

- [ ] Hub with `validate = [require_authenticated]` — every method in the hub rejects unauthenticated calls without any per-method annotation
- [ ] Hub with `validate = [require_cors(ORIGINS)]` — wrong-Origin calls rejected at every method
- [ ] Hub with `extract = [peer_addr: Option<SocketAddr> = extract_peer_addr]` — methods that declare `#[from_hub] peer_addr: Option<SocketAddr>` receive the value
- [ ] Methods that do not declare `#[from_hub]` compile and run correctly — hub extractors don't affect their signatures
- [ ] A hub can declare both validators and extractors simultaneously
- [ ] Existing hubs with no `validate`/`extract` annotations are unaffected (backward compatible)
- [ ] `cargo test` passes in plexus-macros
