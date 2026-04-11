# REQ-7: synapse-cc TypeScript codegen renders auth annotations

**blocked_by:** [REQ-5, REQ-6]
**unlocks:** []
**status:** Idea

## Why this exists

REQ-5 makes synapse (Haskell CLI) read `psRequest` from the wire schema.
REQ-6 surfaces per-method auth params via `x-plexus-source` on
`MethodSchema.params`. After both land, the schema carries enough
information for any tool to know which methods require auth and which
inputs come from cookies / headers / RPC params.

`synapse-cc` is the TypeScript codegen that generates client SDK code
for browser apps (FormVeritas's frontend uses it). Today it reads
`MethodSchema.params` and produces typed client methods, but it has no
notion of "this method requires authentication" because the param
schema doesn't carry that information.

After REQ-6, the codegen has access to `x-plexus-source` on every param
and can generate a richer client.

## Goal (sketch — not yet a contract)

For every generated client method, emit:

1. **JSDoc annotation** like `@requiresAuth` when any param has
   `source.from === "auth"`, with the resolver name as a tag value.
2. **Cookie/header helper imports** when params come from those
   sources, so the generated client can pull values from
   `document.cookie` automatically.
3. **Type-narrowed method signatures** that omit derived params from
   the caller-facing argument list (clients shouldn't pass `scope`,
   it's resolved server-side).

A method like:

```rust
async fn list(
    &self,
    #[from_auth(self.db.validate_user)] scope: TenantScope<ValidUser>,
    #[activation_param] origin: ValidOrigin,
    search: Option<String>,
    status: Option<ClientStatus>,
) -> impl Stream<Item = ClientEvent>
```

would generate TypeScript like:

```typescript
/**
 * List clients
 * @requiresAuth (resolver: validate_user)
 * @reads-cookie access_token
 * @reads-header origin
 */
export async function list(
  client: PlexusClient,
  params: { search?: string; status?: ClientStatus }
): AsyncIterable<ClientEvent> {
  // scope and origin are derived server-side from cookies/headers,
  // not passed in the request body
  return client.subscribe('clients.list', params);
}
```

## Status

This ticket is a placeholder for the consumer work. The exact contract
needs to be designed once REQ-6 lands and the schema shape is concrete.
The TypeScript codegen surface, the JSDoc tag conventions, and the
generated helper functions are all open design questions.

Mark `status: Ready` when REQ-5 and REQ-6 are both complete and you're
ready to design the codegen contract.
