# REQ-1: `PlexusRequest` Derive — Typed HTTP Upgrade Request as First-Class Input

**blocked_by:** [REQ-0]
**epic:** REQ
**unlocks:** [REQ-2, REQ-3, REQ-4]

## Status: Complete

## Goal

Make the full HTTP upgrade request available as a typed, derive-generated struct. The struct definition IS the request contract — field annotations declare where data comes from, field optionality declares whether missing data is fatal, and `#[derive(schemars::JsonSchema)]` emits the wire schema with `x-plexus-source` extensions. No separate `ContractEntry` type needed.

The core insight: the HTTP upgrade request is a deserializable input, parallel to JSON-RPC params. You define a typed struct for it. A derive macro generates the extraction logic from field-level source annotations.

## Problem

Today the transport middleware does one thing: extract a JWT token → produce an `AuthContext` → insert into Extensions. Everything else in the HTTP upgrade request (cookies, Origin, all other headers, URI) is discarded before the method handler runs. The REQ-1 design in an earlier draft addressed this with standalone extractor functions and a bespoke `ContractEntry` wire format. Both are unnecessary:

- Standalone extractors (`extract_origin`, `extract_cookie`, etc.) are replaced by field annotations on the request struct
- `ContractEntry`/`ContractSource` are replaced by the struct's derived JSON Schema with `x-plexus-source` extensions
- The schema IS the contract — no parallel type needed

## Architecture

### Layer 1 — `RawRequestContext` (plexus-transport, internal)

Internal struct capturing the parts of the HTTP upgrade request useful post-handshake. Not user-visible; it's the input to `PlexusRequest::extract`.

```rust
// src/request/raw.rs — internal, not re-exported
pub struct RawRequestContext {
    pub headers: http::HeaderMap,
    pub uri:     http::Uri,
    pub auth:    Option<AuthContext>,   // populated by CombinedAuthMiddleware
    pub peer:    Option<std::net::SocketAddr>,
}
```

The existing `CombinedAuthMiddleware` inserts `Arc<AuthContext>` into Extensions. Extend it to also insert `Arc<RawRequestContext>` containing the full header map, URI, and resolved auth. `arc_into_rpc_module` extracts `Arc<RawRequestContext>` instead of bare `Arc<AuthContext>`.

### Layer 2 — `PlexusRequest` derive macro (plexus-transport + plexus-macros)

A user-defined struct with `#[derive(PlexusRequest, schemars::JsonSchema)]` gets:

1. `fn extract(ctx: &RawRequestContext) -> Result<Self, PlexusError>` — generated extraction logic
2. `x-plexus-source` schemars attributes on each field — drives JSON Schema output

**Field annotations:**

| Annotation | Source | Extraction behavior |
|------------|--------|---------------------|
| `#[from_cookie("name")]` | Cookie header | Parse `name=value` from Cookie string |
| `#[from_header("name")]` | HTTP header | Read named header, UTF-8 decode |
| `#[from_query("name")]` | WS upgrade URI | Read named query param |
| `#[from_peer]` | Network state | Copy `RawRequestContext::peer` |
| `#[from_auth_context]` | CombinedAuthMiddleware | Copy `RawRequestContext::auth` — for internal use |
| *(none)* | n/a | Field must implement `Default`; always succeeds |

**Optionality rules:**

- `field: String` (non-optional) — extraction failure returns `Err(PlexusError::Unauthenticated(...))`. Call does not proceed.
- `field: Option<T>` — extraction failure or missing data returns `Ok(None)`. Call continues.

This collapses the "validator" concept entirely. `require_authenticated` becomes `auth_token: String` on the request struct. If the cookie is absent, extraction fails and the call is rejected — no separate validator needed.

**Example:**

```rust
// src/auth/clients_request.rs (FormVeritas crate)
#[derive(PlexusRequest, schemars::JsonSchema)]
struct ClientsRequest {
    /// JWT from Keycloak auth flow
    #[from_cookie("access_token")]
    auth_token: String,              // required: extraction failure = call fails

    /// Caller's IP address (server-derived)
    #[from_peer]
    peer_addr: Option<SocketAddr>,   // optional: missing is fine

    /// Request origin header
    #[from_header("origin")]
    origin: Option<String>,          // optional
}
```

**Custom extractor fallback:** For exotic cases where an annotation isn't enough, use `#[from_request(my_fn)]` where `my_fn: fn(&RawRequestContext) -> Result<T, PlexusError>`. The macro calls `my_fn(ctx)?` and skips schema source annotation (treated as `x-plexus-source: derived`).

### Layer 3 — `#[activation_param]` on method params (plexus-macros)

This is an activation-level concept. The request struct is extracted once at activation dispatch time, before routing to individual methods. After the activation extracts its `RequestStruct`, individual method params annotated with `#[activation_param]` pull named fields from that already-extracted struct:

```rust
async fn list(
    &self,
    #[activation_param] auth_token: String,    // pulls ClientsRequest::auth_token
    search: Option<String>,                // normal RPC param
) -> impl Stream<Item = ClientEvent> { ... }
```

The param name must match a field name in the activation's request struct. Type must match. Mismatch = compile error.

No expr argument — the extraction is defined on the struct, not on the method param. `#[activation_param]` is a field accessor, not an extractor. The naming reflects that this is an activation-level concept — the extraction happens at activation dispatch time, and the method is simply accessing a value that was already extracted.

### Layer 4 — `#[from_auth(expr)]` unchanged (plexus-macros)

Reads `auth: Option<AuthContext>` from the extracted request struct (the `#[from_auth_context]` field), passes it to the resolver expression. `ValidUser` is never in the schema.

```rust
async fn list(
    &self,
    #[activation_param] auth_token: String,
    #[from_auth(self.db.validate_user)] user: ValidUser,  // unchanged
    search: Option<String>,
) -> ...
```

### JSON Schema output with `x-plexus-source`

The struct's `schemars::JsonSchema` impl (generated by derive) annotates each field with `x-plexus-source`:

```json
{
  "request": {
    "type": "object",
    "properties": {
      "auth_token": {
        "type": "string",
        "description": "JWT from Keycloak auth flow",
        "x-plexus-source": { "from": "cookie", "key": "access_token" }
      },
      "peer_addr": {
        "type": "string",
        "description": "Caller IP address",
        "x-plexus-source": { "from": "derived" }
      },
      "origin": {
        "type": "string",
        "x-plexus-source": { "from": "header", "key": "origin" }
      }
    },
    "required": ["auth_token"]
  }
}
```

`required` array = non-Option fields. `x-plexus-source` is synthesized by the derive macro via schemars `schema_with` or custom attributes. The wire schema for `psRequest` in `PluginSchema` (REQ-5) is this JSON Schema blob, passed as an opaque `Value`.

## Files to Add/Modify

| File | Repo | Change |
|------|------|--------|
| `src/request/mod.rs` | plexus-transport | New module: re-exports `PlexusRequest` trait, `RawRequestContext`, `PlexusRequestField` |
| `src/request/raw.rs` | plexus-transport | `RawRequestContext` struct (internal) |
| `src/request/derive.rs` | plexus-transport | `PlexusRequest` trait definition; `PlexusRequestField` trait for newtype field types |
| `src/lib.rs` | plexus-transport | `pub mod request;` export |
| `src/websocket.rs` | plexus-transport | Insert `Arc<RawRequestContext>` into Extensions alongside auth |
| `plexus-macros/src/request.rs` | plexus-macros | `#[derive(PlexusRequest)]` proc-macro: parse field annotations, generate `extract()`, generate schemars `x-plexus-source` attributes |
| `plexus-macros/src/lib.rs` | plexus-macros | Export `PlexusRequest` derive |
| `src/plexus/plexus.rs` | plexus-core | Extract `Arc<RawRequestContext>` instead of bare `Arc<AuthContext>`; pass to dispatch |

Note: standalone extractor functions (`extract_origin`, `extract_cookie`, etc.), `ContractEntry`, and `ContractSource` are **not added** — they are replaced by the derive approach.

## Acceptance Criteria

- [ ] `#[derive(PlexusRequest)]` on a struct with `#[from_cookie("access_token")] auth_token: String` generates a working `extract()` that returns `Err` when the cookie is absent
- [ ] Same struct with `origin: Option<String>` annotated `#[from_header("origin")]` returns `Ok(None)` when Origin header is absent
- [ ] `#[from_peer] peer_addr: Option<SocketAddr>` extracts peer address correctly
- [ ] `schemars::schema_for!(ClientsRequest)` output includes `x-plexus-source` on annotated fields
- [ ] `required` array in schema output matches non-Option fields only
- [ ] `#[from_auth(expr)]` continues to work unchanged (backward compatible)
- [ ] `RawRequestContext` is available in Extensions after any WS connection (authenticated or not)
- [ ] No headers are cloned for connections that don't use a `PlexusRequest` activation

## Tests

### Unit — derive-based extraction (`plexus-transport/tests/request_extract.rs`)

Tests construct a `RawRequestContext` directly — no HTTP server, no WebSocket.

```rust
fn make_raw(headers: &[(&str, &str)], peer: Option<&str>) -> RawRequestContext {
    let mut h = http::HeaderMap::new();
    for (k, v) in headers {
        h.insert(http::header::HeaderName::from_static(k), v.parse().unwrap());
    }
    RawRequestContext {
        headers: h,
        uri: "/".parse().unwrap(),
        auth: None,
        peer: peer.map(|p| p.parse().unwrap()),
    }
}

#[derive(PlexusRequest)]
struct TestRequest {
    #[from_cookie("access_token")]
    auth_token: String,
    #[from_header("origin")]
    origin: Option<String>,
    #[from_peer]
    peer_addr: Option<SocketAddr>,
}

#[test] fn required_cookie_present() {
    let ctx = make_raw(&[("cookie", "access_token=jwt123")], None);
    let req = TestRequest::extract(&ctx).unwrap();
    assert_eq!(req.auth_token, "jwt123");
}

#[test] fn required_cookie_absent_is_err() {
    let ctx = make_raw(&[], None);
    assert!(TestRequest::extract(&ctx).is_err());
}

#[test] fn optional_header_present() {
    let ctx = make_raw(&[("cookie", "access_token=x"), ("origin", "https://app.example.com")], None);
    let req = TestRequest::extract(&ctx).unwrap();
    assert_eq!(req.origin, Some("https://app.example.com".into()));
}

#[test] fn optional_header_absent() {
    let ctx = make_raw(&[("cookie", "access_token=x")], None);
    let req = TestRequest::extract(&ctx).unwrap();
    assert_eq!(req.origin, None);
}

#[test] fn peer_addr_present() {
    let ctx = make_raw(&[("cookie", "access_token=x")], Some("1.2.3.4:5678"));
    let req = TestRequest::extract(&ctx).unwrap();
    assert_eq!(req.peer_addr, Some("1.2.3.4:5678".parse().unwrap()));
}

#[test] fn peer_addr_absent() {
    let ctx = make_raw(&[("cookie", "access_token=x")], None);
    let req = TestRequest::extract(&ctx).unwrap();
    assert_eq!(req.peer_addr, None);
}

#[test] fn cookie_header_with_multiple_values() {
    let ctx = make_raw(&[("cookie", "session=abc; access_token=tok; other=xyz")], None);
    let req = TestRequest::extract(&ctx).unwrap();
    assert_eq!(req.auth_token, "tok");
}
```

### Unit — JSON Schema output (`plexus-transport/tests/request_schema.rs`)

```rust
#[test] fn schema_has_x_plexus_source_on_cookie_field() {
    let schema = schemars::schema_for!(TestRequest);
    let obj = serde_json::to_value(&schema).unwrap();
    let source = &obj["properties"]["auth_token"]["x-plexus-source"];
    assert_eq!(source["from"], "cookie");
    assert_eq!(source["key"], "access_token");
}

#[test] fn schema_has_derived_source_on_peer_field() {
    let schema = schemars::schema_for!(TestRequest);
    let obj = serde_json::to_value(&schema).unwrap();
    let source = &obj["properties"]["peer_addr"]["x-plexus-source"];
    assert_eq!(source["from"], "derived");
}

#[test] fn schema_required_matches_non_option_fields() {
    let schema = schemars::schema_for!(TestRequest);
    let obj = serde_json::to_value(&schema).unwrap();
    let required = obj["required"].as_array().unwrap();
    let names: Vec<&str> = required.iter().map(|v| v.as_str().unwrap()).collect();
    assert!(names.contains(&"auth_token"));
    assert!(!names.contains(&"origin"));
    assert!(!names.contains(&"peer_addr"));
}
```

### Unit — auth field forwarding

```rust
#[derive(PlexusRequest)]
struct AuthedRequest {
    #[from_cookie("access_token")]
    auth_token: String,
    #[from_auth_context]
    auth: Option<AuthContext>,
}

#[test] fn auth_context_carried_through() {
    let mut ctx = make_raw(&[("cookie", "access_token=x")], None);
    ctx.auth = Some(AuthContext { user_id: "u1".into(), session_id: "s1".into(),
                                  roles: vec![], metadata: Default::default() });
    let req = AuthedRequest::extract(&ctx).unwrap();
    assert_eq!(req.auth.as_ref().unwrap().user_id, "u1");
}

#[test] fn auth_context_none_when_unauthenticated() {
    let ctx = make_raw(&[("cookie", "access_token=x")], None);
    let req = AuthedRequest::extract(&ctx).unwrap();
    assert!(req.auth.is_none());
}
```

### Integration — Extensions wiring (`plexus-transport/tests/integration.rs`)

Start a test server with `TestSessionValidator`. Connect a client. Call a method that uses `#[activation_param]`.

```
// echo_origin method uses #[activation_param] origin: Option<String>
// Case 1: WS upgrade with Origin header → method returns Some("http://test.local")
// Case 2: WS upgrade without Origin header → method returns None
// Case 3: authenticated client → req struct has populated auth field
// Case 4: unauthenticated client + activation with required auth_token field → call returns -32001
```

## Open Design Questions

**Q1: Should `#[from_peer]` fields appear in the JSON Schema?**

Yes. Include them with `x-plexus-source: { "from": "derived" }` so docs show they exist and clients know a peer address is visible server-side. They don't appear in `required`.

**Q2: Can a request struct field have a custom extractor function?**

Yes: `#[from_request(my_fn)]` where `my_fn: fn(&RawRequestContext) -> Result<T, PlexusError>`. The macro generates `my_fn(ctx)?` and annotates the field with `x-plexus-source: { "from": "derived" }` (opaque to the schema). This is the escape hatch for exotic extraction not covered by the stdlib annotations.

**Q3: Should `PlexusRequest` be a trait or a generated inherent impl?**

Trait. Defining `trait PlexusRequest: schemars::JsonSchema` allows activation dispatch code to be generic over `R: PlexusRequest`, enables mocking in tests, and allows per-method override (`#[plexus::method(request = ())]` uses `NoRequest: PlexusRequest`).
