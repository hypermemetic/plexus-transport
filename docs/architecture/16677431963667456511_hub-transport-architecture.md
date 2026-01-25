# hub-transport Architecture

**Date**: 2026-01-24
**Status**: Implemented
**Context**: Extract substrate's transport layer into reusable library

## Problem Statement

Substrate's transport infrastructure (WebSocket, stdio, MCP HTTP) was hardcoded to work with Plexus. This created several issues:

1. **Code duplication** - Any project wanting to host an activation needed to copy ~200 lines of transport code
2. **Plexus-specific** - Single plugins couldn't easily be hosted as standalone servers
3. **Maintenance burden** - Transport updates needed to be synced across projects
4. **No reusability** - The pattern couldn't be applied to nested hubs or custom activations

## Key Insight

**Plexus is just an Activation with routing.**

There's nothing transport-special about Plexus. It implements the `Activation` trait like any plugin. The transport layer should work generically with `impl Activation`, whether that's:
- Single plugin (JsExec, Echo)
- Plexus hub (routes to children)
- Nested hub (Solar, HyperforgeHub)

## Architecture

### Core Abstraction

```rust
pub struct TransportServer<A: Activation> {
    activation: Arc<A>,
    config: TransportConfig,
    rpc_converter: Option<RpcConverter<A>>,
}
```

Generic over `A: Activation` - works with **any** activation type.

### The RPC Conversion Problem

The `Activation` trait defines:
```rust
fn into_rpc_methods(self) -> Methods where Self: Sized;
```

This consumes `self`, but we need to:
1. Keep the `Arc<A>` alive for the lifetime of the server
2. Support multiple transports that might need the RPC module
3. Preserve `Arc` references for `Weak<Plexus>` held by activations

**Solution: Callback-based conversion**

```rust
pub type RpcConverter<A> = Box<dyn FnOnce(Arc<A>) -> Result<RpcModule<()>> + Send>;
```

Users provide a converter function that knows how to handle their specific activation type:

```rust
// For Plexus - preserves Arc for Weak references
let rpc_converter = |arc: Arc<Plexus>| {
    Plexus::arc_into_rpc_module(arc)
};

// For simple plugins - can just unwrap and convert
let rpc_converter = |arc: Arc<JsExec>| {
    Ok(Arc::try_unwrap(arc).unwrap_or_else(|a| (*a).clone()).into_rpc_methods())
};
```

This pattern:
- Delegates Arc lifecycle management to the user
- Supports both Arc-preserving (Plexus) and consuming (simple plugins) patterns
- Keeps the library generic without type system gymnastics

### Transport Modules

#### stdio.rs
Line-delimited JSON-RPC over stdin/stdout (MCP-compatible).

```rust
pub async fn serve_stdio(module: RpcModule<()>, config: StdioConfig) -> Result<()>
```

**Key features:**
- Reads requests from stdin, writes responses to stdout
- Spawns tasks for subscription notifications
- Buffer size configurable (default: 1024)
- Logs to stderr to keep stdout clean

#### websocket.rs
JSON-RPC WebSocket server using `jsonrpsee`.

```rust
pub async fn serve_websocket(module: RpcModule<()>, config: WebSocketConfig) -> Result<ServerHandle>
```

**Key features:**
- Returns `ServerHandle` for lifecycle control
- Supports subscriptions natively
- Can run alongside other transports

#### mcp/bridge.rs
Generic MCP protocol handler.

```rust
pub struct ActivationMcpBridge<A: Activation> {
    activation: Arc<A>,
}

impl<A: Activation> ServerHandler for ActivationMcpBridge<A>
```

**Key features:**
- Generic over `Activation` trait (not Plexus-specific)
- Converts `Activation::plugin_schema()` to MCP tools
- Routes `tools/call` to `Activation::call()`
- Streams `PlexusStreamItem` events via MCP logging/progress notifications
- Buffers data for final result

**Schema transformation:**
```rust
schemas_to_rmcp_tools(activation.plugin_schema()) -> Vec<Tool>
```

**Method routing:**
```rust
// Extract method name (remove namespace if present)
let method = if method_name.contains('.') {
    method_name.split('.').nth(1).unwrap_or(method_name)
} else {
    method_name
};

// Call activation
activation.call(method, arguments).await
```

#### mcp/session.rs
Session management for MCP HTTP (copied from substrate).

**Two implementations:**
- `LocalSessionManager` - In-memory, simple, default
- `SqliteSessionManager` - Persistent, survives restarts, opt-in via `sqlite-sessions` feature

**Note**: Session storage is for MCP protocol resumption (which client sessions are active), NOT plugin domain data (Arbor trees, Cone state, etc.).

#### mcp/server.rs
MCP HTTP server setup using `axum` and `rmcp`.

```rust
pub async fn serve_mcp_http<A: Activation>(
    activation: Arc<A>,
    config: McpHttpConfig,
) -> Result<JoinHandle<...>>
```

**Key features:**
- Creates `ActivationMcpBridge<A>` from activation
- Sets up `StreamableHttpService` with session manager
- Axum router at `/mcp` with debug/fallback handlers
- Returns `JoinHandle` for async lifecycle

### Builder Pattern

```rust
pub struct TransportServerBuilder<A: Activation> {
    activation: Arc<A>,
    config: TransportConfig,
    rpc_converter: Option<RpcConverter<A>>,
}
```

**API design:**
- Method chaining for clean configuration
- No transport enabled by default
- Stdio and WebSocket mutually exclusive (stdio blocks)
- WebSocket and MCP HTTP can run simultaneously

**Build logic:**
```rust
pub async fn build(self) -> Result<TransportServer<A>>
```

**Serve logic:**
```rust
pub async fn serve(mut self) -> Result<()> {
    // If stdio: block on stdin (primary transport)
    if let Some(stdio_config) = self.config.stdio {
        return serve_stdio(module, stdio_config).await;
    }

    // Otherwise: spawn WebSocket and/or MCP HTTP
    // Wait for completion with tokio::select!
}
```

## Configuration Design

### Modular Configs

Each transport has its own config type:

```rust
pub struct WebSocketConfig { pub addr: SocketAddr }
pub struct StdioConfig { pub subscription_buffer_size: usize }
pub struct McpHttpConfig {
    pub addr: SocketAddr,
    pub session_storage: SessionStorage,
}
```

### TransportConfig Container

```rust
pub struct TransportConfig {
    pub websocket: Option<WebSocketConfig>,
    pub stdio: Option<StdioConfig>,
    pub mcp_http: Option<McpHttpConfig>,
}
```

`Option<T>` pattern makes it clear which transports are enabled.

### Session Storage Enum

```rust
pub enum SessionStorage {
    InMemory,
    #[cfg(feature = "sqlite-sessions")]
    Sqlite { path: PathBuf },
}
```

**Design choice:** In-memory by default, SQLite opt-in.

**Rationale:**
- Simpler for development (no files)
- Most use cases don't need persistent MCP sessions
- Production users can enable feature flag

## Design Decisions

### 1. Generic over Activation (not Plexus-specific)

**Rationale:** User insight that "Plexus is just another Activation with routing." No need for special handling.

**Impact:**
- Single API works for plugins, hubs, nested hubs
- No code duplication per activation type
- MCP bridge works generically

### 2. Callback-based RPC conversion

**Rationale:** The `into_rpc_methods()` self-consumption pattern conflicts with Arc lifetime preservation.

**Alternatives considered:**
- `Clone` bound on `Activation` - forces all activations to be cloneable (heavy)
- Associated type for RPC module - type system complexity
- Arc unwrap with clone fallback - loses Weak reference validity

**Choice:** Callback lets each activation handle conversion appropriately.

### 3. Builder pattern with method chaining

**Rationale:**
- Clean API for optional transports
- Extensible for future transport types
- Self-documenting configuration

**Example:**
```rust
TransportServer::builder(activation, converter)
    .with_websocket(8888)
    .with_mcp_http(8889)
    .build().await?
```

### 4. In-memory MCP sessions by default

**Rationale:**
- Simpler (no files created automatically)
- Fast (no I/O)
- Sufficient for most use cases

SQLite opt-in for production deployments that need resumption across restarts.

### 5. Single crate (not multiple)

**Rationale:**
- All transports are lightweight
- Easier to maintain together
- Single version to track
- Simpler dependency graph

**Alternative:** Split into `hub-transport-{stdio,ws,mcp}` would add complexity without clear benefit.

### 6. Arc lifecycle preservation

**Critical insight:** Activations may hold `Weak<Plexus>` for cross-activation calls. The Arc must stay alive.

**Implementation:** Follow `Plexus::arc_into_rpc_module` pattern - store Arc in TransportServer, clone when needed, let RPC handlers keep references.

## Migration Impact

### Substrate Before

```rust
// src/main.rs: ~338 lines total
// ~200 lines of transport code:
// - Manual WebSocket server setup
// - Manual stdio handler
// - Manual MCP HTTP setup
// - Manual session manager creation
// - Manual axum router config
// - Hardcoded to Plexus
```

### Substrate After

```rust
// src/main.rs: ~138 lines total
// ~20 lines of transport code:

let rpc_converter = |arc: Arc<substrate::Plexus>| {
    substrate::Plexus::arc_into_rpc_module(arc)
        .map_err(|e| anyhow::anyhow!("{}", e))
};

let mut builder = TransportServer::builder(plexus, rpc_converter);

if args.stdio {
    builder = builder.with_stdio();
} else {
    builder = builder.with_websocket(args.port);
    if !args.no_mcp {
        builder = builder.with_mcp_http(args.port + 1);
    }
}

builder.build().await?.serve().await
```

**Result:**
- 200 lines → 20 lines (10x reduction)
- All functionality preserved
- Easier to read and maintain
- Reusable pattern

## Future Extensions

### Potential Transport Types

1. **HTTP POST** - Single request/response (no streaming)
2. **gRPC** - Bidirectional streaming
3. **SSE standalone** - Server-sent events without MCP
4. **Unix domain sockets** - IPC for local communication

### Potential Features

1. **Metrics/observability** - Built-in tracing/metrics hooks
2. **Rate limiting** - Per-client request throttling
3. **Auth/authz** - Pluggable authentication layers
4. **Multi-tenancy** - Routing to different activations per client

All extensions would use the same builder pattern:
```rust
builder
    .with_websocket(8888)
    .with_grpc(9000)  // future
    .with_metrics(registry)  // future
```

## Testing Strategy

### Unit Tests
- Schema transformation (plugin schema → MCP tools)
- Error mapping (PlexusError → McpError)
- Config validation

### Integration Tests
- Stdio transport with real activation
- WebSocket client → activation round-trip
- MCP HTTP tool call → activation → response

### Manual Testing
- Deploy substrate with hub-transport
- Verify all three transports work
- Test with Claude Desktop (MCP)
- Test with WebSocket client (synapse)

## Related Documents

- `README.md` - User-facing documentation
- `substrate/docs/architecture/16680569353625987583_stdio-transport-implementation.md` - Original stdio design
- `substrate/docs/architecture/16680091879496180735_rmcp-mcp-bridge.md` - Original MCP bridge

## Conclusion

hub-transport successfully extracts substrate's transport layer into a reusable, generic library. The key innovations are:

1. **Generic over Activation** - Works with any activation type
2. **Callback-based RPC conversion** - Handles Arc lifecycle elegantly
3. **Builder pattern** - Clean, composable configuration
4. **MCP-first design** - Full protocol support with streaming

This library enables:
- Hosting single plugins as standalone servers
- Reusing transport infrastructure across projects
- Simplified main.rs in substrate (200 → 20 lines)
- Foundation for future transport types

The architecture is proven, implemented, and deployed in substrate.
