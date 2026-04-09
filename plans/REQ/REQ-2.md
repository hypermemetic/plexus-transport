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
