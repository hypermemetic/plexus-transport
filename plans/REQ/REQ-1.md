# REQ-1: Request Context — Forward HTTP Request to Method Dispatch

**epic:** REQ
**unlocks:** [REQ-2, REQ-3]

## Status: Planned

## Goal

Make the full HTTP upgrade request available through the Plexus dispatch chain, enabling per-method extraction of arbitrary request data — cookies, Origin, custom headers, URI — via composable transformer functions.

## Problem

Today the transport middleware does one thing: extract a JWT token → produce an `AuthContext` → insert into Extensions. Everything else in the HTTP upgrade request (cookies, Origin, all other headers, URI) is discarded before the method handler runs.

This means:
- Origin validation must be hardcoded in the middleware (not per-method configurable)
- Cookie extraction is only for auth tokens; other cookie values are inaccessible
- There is no way to write a method that inspects the `Referer`, `User-Agent`, or any custom header
- The `#[from_auth]` macro is auth-specific when it could be a general extraction mechanism

The goal is to unify these into a single composable pattern:

```rust
// Any typed value extractable from the HTTP request
#[from_request(extract_origin)]    origin: Option<String>,
#[from_request(extract_cookie("session_id"))] session: Option<String>,
#[from_request(self.db.validate_user)]        user: ValidUser,
```

## Architecture

### Layer 1 — `RequestContext` type (plexus-transport)

A struct capturing the parts of the HTTP upgrade request that are useful post-handshake:

```rust
pub struct RequestContext {
    pub headers: http::HeaderMap,
    pub uri:     http::Uri,
    pub auth:    Option<AuthContext>,  // populated by existing SessionValidator
    pub peer:    Option<std::net::SocketAddr>,
}
```

The existing `CombinedAuthMiddleware` inserts `Arc<AuthContext>` into Extensions. Extend it to also insert `Arc<RequestContext>` containing the full header map, URI, and resolved auth together.

### Layer 2 — `RequestContext` in Extensions (plexus-core)

`arc_into_rpc_module` currently extracts `Arc<AuthContext>` from Extensions. Change it to extract `Arc<RequestContext>` instead. `AuthContext` is still available as `ctx.auth`. This is a non-breaking change for `#[from_auth]` callers — they continue to receive `AuthContext` via `ctx.auth`.

### Layer 3 — Extractor functions

An extractor is any function (or closure) with this signature:

```rust
Fn(&RequestContext) -> Result<T, PlexusError>
// or
Fn(&RequestContext) -> T  (for infallible extractors)
// or
Fn(&RequestContext) -> Option<T>  (for optional extractors)
```

Standard extractors shipped with plexus-transport:

```rust
// Origin header → validated String
pub fn extract_origin(ctx: &RequestContext) -> Option<String>;

// Named cookie value
pub fn extract_cookie<'a>(name: &'a str) 
    -> impl Fn(&RequestContext) -> Option<String> + 'a;

// Peer IP address  
pub fn extract_peer_addr(ctx: &RequestContext) -> Option<std::net::SocketAddr>;

// Full URI
pub fn extract_uri(ctx: &RequestContext) -> http::Uri;

// Raw header value
pub fn extract_header(name: &'static str) 
    -> impl Fn(&RequestContext) -> Option<String>;
```

### Layer 4 — Macro generalization (plexus-macros)

Generalize `#[from_auth(expr)]` to `#[from_request(expr)]` where `expr` receives a `&RequestContext` instead of `&AuthContext`.

`#[from_auth(expr)]` becomes sugar for `#[from_request(|ctx| expr(ctx.auth.as_ref().ok_or(Unauthenticated)?)))]` — i.e. it extracts the auth field and panics semantically if absent.

The macro codegen:
```rust
// Before (from_auth):
let auth_ctx = auth.ok_or_else(|| Unauthenticated("..."))?;
let user = self.db.validate_user(&auth_ctx).await?;

// After (from_request — same behavior, different source):
let req_ctx = request_context.ok_or_else(|| Unauthenticated("no request context"))?;
let user = self.db.validate_user(req_ctx.auth.as_ref().ok_or_else(|| Unauthenticated("..."))?).await?;
```

### Layer 5 — Origin validation via extractor (AUTH-18 implementation path)

With REQ in place, Origin validation is not a middleware concern — it becomes a method-level extractor:

```rust
pub fn require_origin(allowed: &[&str]) -> impl Fn(&RequestContext) -> Result<(), PlexusError> {
    let allowed = allowed.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    move |ctx| {
        let origin = extract_origin(ctx);
        match origin {
            None => Ok(()),  // non-browser client, allow
            Some(o) if allowed.contains(&o) => Ok(()),
            Some(o) => Err(PlexusError::Unauthenticated(format!("Origin not allowed: {}", o))),
        }
    }
}

// Usage:
#[from_request(require_origin(&["https://app.formveritas.com"]))]
_origin_check: (),
```

## Tickets

```
REQ-1  (this)   RequestContext type, Extensions wiring, extractor stdlib
REQ-2           Macro: generalize #[from_auth] → #[from_request]
REQ-3           AUTH-18 implementation using REQ extractors (Origin validation)
```

## Dependency DAG

```
REQ-1
  ├── REQ-2 (macro generalization)
  │     └── REQ-3 (origin validation via extractor)
  └── (any future extractor built on RequestContext)
```

## Files to Modify

| File | Repo | Change |
|------|------|--------|
| `src/websocket.rs` | plexus-transport | Insert `Arc<RequestContext>` into Extensions alongside `Arc<AuthContext>` |
| `src/lib.rs` | plexus-transport | Export `RequestContext`, standard extractors |
| `src/plexus/plexus.rs` | plexus-core | Extract `Arc<RequestContext>` instead of bare `Arc<AuthContext>` |
| `src/parse.rs` | plexus-macros | Add `from_request` as synonym for `from_auth` with `RequestContext` input |
| `src/codegen/activation.rs` | plexus-macros | Generate `req_ctx` extraction, pass to resolver exprs |

## Acceptance Criteria

- [ ] `RequestContext` is available in Extensions after any authenticated or unauthenticated WS connection
- [ ] `extract_origin`, `extract_cookie`, `extract_peer_addr` work in unit tests against a fake `RequestContext`
- [ ] `#[from_auth(expr)]` continues to work unchanged (backward compatible)
- [ ] `#[from_request(extract_origin)]` compiles and returns `Option<String>` at method call time
- [ ] No HTTP headers are cloned unnecessarily for connections that don't use `#[from_request]`
