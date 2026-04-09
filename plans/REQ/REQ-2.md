# REQ-2: Macro — Generalize `#[from_auth]` to `#[from_request]`

**blocked_by:** [REQ-1]
**unlocks:** [REQ-3]

## Status: Planned

## Goal

Generalize the `#[from_auth(expr)]` macro attribute into `#[from_request(expr)]` where the resolver expression receives a `&RequestContext` instead of `&AuthContext`. Keep `#[from_auth]` working as sugar over `#[from_request]`.

## Current Codegen (`#[from_auth]`)

```rust
// Input:
async fn list(&self, #[from_auth(self.db.validate_user)] user: ValidUser, ...) -> ...

// Generated (simplified):
let auth_ctx = auth.ok_or_else(|| PlexusError::Unauthenticated("..."))?;
let user = self.db.validate_user(&auth_ctx).await
    .map_err(|e| PlexusError::ExecutionError(e.to_string()))?;
```

The resolver expression receives `&AuthContext`.

## Target Codegen (`#[from_request]`)

```rust
// Input:
async fn list(&self, #[from_request(extract_origin)] origin: Option<String>, ...) -> ...

// Generated:
let req_ctx = request_context  // Arc<RequestContext> from Extensions
    .as_deref()
    .ok_or_else(|| PlexusError::Unauthenticated("No request context"))?;
let origin = extract_origin(req_ctx);
```

The resolver expression receives `&RequestContext`.

## `#[from_auth]` as Sugar

```rust
// Input:
#[from_auth(self.db.validate_user)] user: ValidUser

// Desugars to:
#[from_request(|ctx| {
    let auth = ctx.auth.as_ref()
        .ok_or_else(|| PlexusError::Unauthenticated("Authentication required"))?;
    self.db.validate_user(auth)
})] user: ValidUser
```

Implementable by detecting `from_auth` in `parse.rs` and converting it to an `AuthResolver` with a wrapper closure during codegen. No change to the user-facing `#[from_auth]` syntax — it remains identical.

## Parse Changes (`src/parse.rs`)

Add `from_request` to `extract_from_auth_attr()`:

```rust
pub enum ResolverKind {
    FromAuth(Expr),     // existing — resolver receives &AuthContext
    FromRequest(Expr),  // new — resolver receives &RequestContext
}

pub struct AuthResolver {
    pub param_name: Ident,
    pub kind: ResolverKind,
}
```

## Codegen Changes (`src/codegen/activation.rs`)

Generate different extraction code depending on `ResolverKind`:

```rust
match &resolver.kind {
    ResolverKind::FromAuth(expr) => {
        // existing codegen — extract auth from req_ctx.auth
        quote! {
            let auth_ref = req_ctx.auth.as_ref()
                .ok_or_else(|| PlexusError::Unauthenticated("Authentication required".to_string()))?;
            let #param_name = (#expr)(auth_ref).await
                .map_err(|e| PlexusError::ExecutionError(e.to_string()))?;
        }
    }
    ResolverKind::FromRequest(expr) => {
        // new codegen — pass full RequestContext to resolver
        quote! {
            let #param_name = (#expr)(req_ctx);
        }
    }
}
```

For `FromRequest`, if the resolver returns `Result<T>`, add `.map_err(...)`. If it returns `Option<T>` or `T`, use directly. The macro infers this from the parameter type or requires a trait bound.

## Backward Compatibility

- All existing `#[from_auth(expr)]` usages continue to compile and behave identically
- No changes required in consumer crates (FormVeritas, etc.) for existing code
- New `#[from_request(expr)]` is purely additive

## Files

- `plexus-macros/src/parse.rs` — add `ResolverKind`, update `extract_from_auth_attr`
- `plexus-macros/src/codegen/activation.rs` — branch on `ResolverKind` in codegen
- `plexus-core/src/plexus/plexus.rs` — pass `Arc<RequestContext>` (from REQ-1) instead of `Arc<AuthContext>` to dispatch; update `auth` extraction to `req_ctx.auth`

## Acceptance Criteria

- [ ] `#[from_request(extract_origin)]` on a method param compiles and injects `Option<String>`
- [ ] `#[from_auth(self.db.validate_user)]` continues to work unchanged
- [ ] A method can mix `#[from_auth]` and `#[from_request]` parameters
- [ ] The generated code does not clone `RequestContext` unnecessarily
- [ ] `cargo test` passes in plexus-macros

## Tests

### Compile tests — `plexus-macros/tests/compile/` (trybuild)

These verify the macro accepts or rejects specific syntax. Each is a `.rs` file that must compile (or fail with the expected error).

**`from_request_infallible.rs`** — must compile:
```rust
#[hub_methods(namespace = "test")]
impl MyActivation {
    async fn echo(
        &self,
        #[from_request(extract_origin)] origin: Option<String>,
    ) -> impl Stream<Item = String> { stream! { yield origin.unwrap_or_default(); } }
}
```

**`from_request_fallible.rs`** — must compile (resolver returns `Result`):
```rust
#[hub_methods(namespace = "test")]
impl MyActivation {
    async fn guarded(
        &self,
        #[from_request(require_allowed_origin(&["http://localhost"]))] _: ValidOrigin,
    ) -> impl Stream<Item = String> { stream! { yield "ok".into(); } }
}
```

**`from_auth_still_works.rs`** — must compile (backward compat):
```rust
#[hub_methods(namespace = "test")]
impl MyActivation {
    async fn authed(
        &self,
        #[from_auth(self.db.validate_user)] user: ValidUser,
    ) -> impl Stream<Item = String> { stream! { yield user.id().to_string(); } }
}
```

**`mixed_from_auth_and_from_request.rs`** — must compile:
```rust
async fn mixed(
    &self,
    #[from_request(extract_origin)] origin: Option<String>,
    #[from_auth(self.db.validate_user)] user: ValidUser,
    name: String,
) -> ...
```

**`from_request_stripped_from_schema.rs`** — verify the generated RPC trait excludes the `#[from_request]` param. The jsonrpsee RPC trait method must have signature `fn echo(&self, origin: Option<String>)` stripped to `fn echo(&self)`.

### Unit — generated code correctness (`plexus-macros/tests/codegen.rs`)

Expand the macro and assert the generated token stream contains the right extraction call:

```rust
// The generated dispatch for `#[from_request(extract_origin)] origin: Option<String>` must contain:
//   let origin = extract_origin(req_ctx);
// NOT:
//   let origin = extract_origin(&auth_ctx);   ← wrong source

// The generated dispatch for `#[from_auth(self.db.validate_user)] user: ValidUser` must contain:
//   let auth_ref = req_ctx.auth.as_ref().ok_or_else(|| Unauthenticated(...))?;
//   let user = self.db.validate_user(auth_ref).await...
```

### Runtime — method receives correct value

Using a test server with `TestSessionValidator` and a stub activation:

```
// from_request — Origin header present:
// client sets Origin: http://test.local on WS upgrade
// calls echo_origin()
// assert response == Some("http://test.local")

// from_request — Origin header absent:
// client sets no Origin header
// calls echo_origin()
// assert response == None

// from_auth — no token:
// client connects without token
// calls authed_method()
// assert error code == -32001, message contains "Authentication required"

// from_auth — valid token:
// client connects with valid test token
// calls authed_method()
// assert response contains user_id from token
```
