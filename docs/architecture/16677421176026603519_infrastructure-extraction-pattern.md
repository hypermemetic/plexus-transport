# Infrastructure Extraction Pattern

**Date**: 2026-01-24
**Status**: Reference Guide
**Context**: Lessons learned from extracting hub-transport from substrate

## Overview

This document describes the pattern for extracting infrastructure code (transport layers, storage backends, middleware) from a specific application into a reusable library. It captures the process used to create `hub-transport` from substrate's transport code.

**Key Insight**: Infrastructure code often becomes tightly coupled to specific types even when the underlying logic is generic. Extracting it requires identifying the abstraction boundary and handling lifecycle/type system challenges.

## When to Extract

### Good Candidates for Extraction

✅ **Code duplication across projects**
- Same transport setup in multiple binaries
- Copy-paste of session management
- Repeated HTTP/WebSocket server boilerplate

✅ **Generic patterns with specific coupling**
- WebSocket server hardcoded to one type
- MCP bridge specific to Plexus but logic is generic
- Storage layer that could work with any data type

✅ **Stable, well-tested infrastructure**
- Battle-tested in production
- Clear boundaries and responsibilities
- Minimal changes over time

✅ **High reuse potential**
- Multiple projects need the same functionality
- Plugins want standalone hosting
- Testing harnesses need isolated servers

### Poor Candidates for Extraction

❌ **Business logic**
- Domain-specific workflows
- Application state machines
- Custom validation rules

❌ **Rapidly changing code**
- Experimental features
- Frequent rewrites
- Unclear requirements

❌ **Single-use code**
- One-off scripts
- Application-specific glue
- Tight integration with app internals

## The Extraction Process

### Phase 1: Identify the Abstraction Boundary

**Goal**: Find the trait/interface that makes the code generic.

#### Steps

1. **List the specific types used**
   ```rust
   // Before: substrate/src/main.rs
   let plexus = build_plexus().await;
   serve_websocket(plexus).await;  // ← hardcoded to Plexus
   ```

2. **Find the common interface**
   ```rust
   // What do ALL potential types implement?
   pub trait Activation {
       fn call(&self, method: &str, params: Value) -> PlexusStream;
       fn plugin_schema(&self) -> PluginSchema;
       // ...
   }
   ```

3. **Verify generalization is possible**
   - Can JsExec, Echo, Solar all use this transport?
   - Is there anything Plexus-specific that can't be abstracted?
   - Are there hidden assumptions about the type?

**hub-transport example:**
```rust
// The abstraction boundary
pub struct TransportServer<A: Activation> { ... }
                          ^^^^^^^^^^^^^^^^
// Generic over the trait, not the concrete type
```

### Phase 2: Identify Lifecycle Challenges

**Goal**: Understand ownership, Arc/Weak patterns, and self-consumption issues.

#### Common Lifecycle Patterns

**Pattern 1: Arc Preservation for Weak References**

```rust
// Problem: Activations hold Weak<Plexus> for cross-activation calls
struct ChildActivation {
    plexus: Weak<Plexus>,  // ← Must stay valid!
}

// Solution: Keep Arc alive throughout server lifetime
pub struct TransportServer<A> {
    activation: Arc<A>,  // ← Arc stays here
    // ...
}
```

**Pattern 2: Self-Consuming Methods**

```rust
// Problem: Trait method consumes self
trait Activation {
    fn into_rpc_methods(self) -> Methods;  // ← Takes ownership!
}

// Solution: Callback pattern
type RpcConverter<A> = Box<dyn FnOnce(Arc<A>) -> Result<RpcModule<()>>>;

// User provides conversion logic
let converter = |arc: Arc<Plexus>| {
    Plexus::arc_into_rpc_module(arc)  // ← Keeps Arc alive
};
```

**Pattern 3: Multiple Borrows**

```rust
// Problem: Multiple transports need the same activation
let ws = serve_websocket(&activation);  // Borrow 1
let mcp = serve_mcp_http(&activation);  // Borrow 2 (conflict!)

// Solution: Arc for shared ownership
let activation = Arc::new(MyActivation::new());
let ws = serve_websocket(activation.clone());
let mcp = serve_mcp_http(activation.clone());
```

#### Analysis Checklist

- [ ] Does the type hold Weak references that must stay valid?
- [ ] Are there self-consuming methods (into_*, consume, etc.)?
- [ ] Do multiple components need simultaneous access?
- [ ] Is there a clone() implementation? Is it cheap or expensive?
- [ ] Are there 'static lifetime requirements?

### Phase 3: Create the Library Structure

**Goal**: Set up the crate with clean module organization.

#### Directory Structure

```
hub-transport/
├── Cargo.toml              # Dependencies and features
├── .gitignore              # Ignore target/, Cargo.lock
├── README.md               # User-facing docs
├── docs/
│   └── architecture/       # Design docs
│       ├── __index.md
│       └── [timestamp]_name.md
├── src/
│   ├── lib.rs              # Public API exports
│   ├── config.rs           # Configuration types
│   ├── server.rs           # Main server logic
│   ├── [transport1].rs     # Individual transport modules
│   ├── [transport2].rs
│   └── [subsystem]/        # Optional: complex subsystems
│       ├── mod.rs
│       └── components.rs
└── examples/               # Usage examples
    ├── single_plugin.rs
    └── full_hub.rs
```

#### Cargo.toml Best Practices

```toml
[package]
name = "hub-transport"
version = "0.1.0"
edition = "2021"
description = "Generic transport layer for activations"

[dependencies]
# Core trait definitions (required)
hub-core = { path = "../hub-core" }

# Always-on dependencies
tokio = { version = "1.0", features = ["full"] }
anyhow = "1.0"

# Optional heavy dependencies (feature-gated)
sqlx = { version = "0.8", features = ["sqlite"], optional = true }

[features]
default = []
sqlite-sessions = ["sqlx", "tokio-stream"]

[dev-dependencies]
# Testing dependencies
```

**Key decisions:**
- Feature flags for heavy dependencies (sqlx)
- Path dependencies for local crates
- Minimal required dependencies by default

### Phase 4: Extract and Generalize Code

**Goal**: Copy code from the application and make it generic.

#### Step-by-Step Extraction

**1. Copy the code as-is**
```bash
cp substrate/src/stdio_transport.rs hub-transport/src/stdio.rs
```

**2. Replace concrete types with generics**
```rust
// Before
pub async fn serve_stdio(plexus: Arc<Plexus>) -> Result<()> {
    let module = plexus.into_rpc_module();
    // ...
}

// After
pub async fn serve_stdio(module: RpcModule<()>) -> Result<()> {
    // Same logic, but takes the already-converted module
    // ...
}
```

**3. Extract configuration**
```rust
// Before: hardcoded values
let buffer_size = 1024;
let addr = "127.0.0.1:4444".parse()?;

// After: configuration struct
pub struct StdioConfig {
    pub subscription_buffer_size: usize,
}

impl Default for StdioConfig {
    fn default() -> Self {
        Self { subscription_buffer_size: 1024 }
    }
}
```

**4. Parameterize type-specific behavior**
```rust
// Before: calls Plexus::list_plugin_schemas()
let schemas = plexus.list_plugin_schemas();

// After: uses trait method on generic type
let schema = activation.plugin_schema();
```

#### Generalization Patterns

**Pattern: Generic Bridge**
```rust
// Before: PlexusMcpBridge
pub struct PlexusMcpBridge {
    plexus: Arc<Plexus>,
}

// After: ActivationMcpBridge<A>
pub struct ActivationMcpBridge<A: Activation> {
    activation: Arc<A>,
}

impl<A: Activation> ServerHandler for ActivationMcpBridge<A> {
    fn list_tools(&self, ...) -> ... {
        let schema = self.activation.plugin_schema();  // ← Trait method
        schemas_to_rmcp_tools(vec![schema])
    }
}
```

**Pattern: Namespace Extraction**
```rust
// Before: hardcoded server name
server_info: Implementation::from_build_env(),  // ← "substrate"

// After: use activation's identity
server_info.name = self.activation.namespace().to_string();
server_info.version = self.activation.version().to_string();
```

**Pattern: Method Name Handling**
```rust
// Before: assumes "namespace.method" format
plexus.route(method_name, args);

// After: strip namespace if present
let method = if method_name.contains('.') {
    method_name.split('.').nth(1).unwrap_or(method_name)
} else {
    method_name
};
activation.call(method, args);
```

### Phase 5: Design the Public API

**Goal**: Create a clean, ergonomic API that hides complexity.

#### Builder Pattern for Configuration

```rust
pub struct TransportServerBuilder<A: Activation> {
    activation: Arc<A>,
    config: TransportConfig,
    rpc_converter: Option<RpcConverter<A>>,
}

impl<A: Activation> TransportServerBuilder<A> {
    pub fn new<F>(activation: Arc<A>, rpc_converter: F) -> Self
    where
        F: FnOnce(Arc<A>) -> Result<RpcModule<()>> + Send + 'static,
    {
        Self {
            activation,
            config: TransportConfig::default(),
            rpc_converter: Some(Box::new(rpc_converter)),
        }
    }

    pub fn with_websocket(mut self, port: u16) -> Self {
        self.config.websocket = Some(WebSocketConfig::new(port));
        self
    }

    pub fn with_stdio(mut self) -> Self {
        self.config.stdio = Some(StdioConfig::default());
        self
    }

    pub async fn build(self) -> Result<TransportServer<A>> { ... }
}
```

**API Design Principles:**

1. **Method chaining** - Each `with_*` returns `Self`
2. **Sensible defaults** - `new()` provides default config
3. **Type safety** - Builder only allows valid configurations
4. **Async build** - Defer expensive setup to `build()`
5. **Explicit errors** - Return `Result` from fallible operations

#### Entry Point Design

```rust
// Simple entry point
impl<A: Activation> TransportServer<A> {
    pub fn builder<F>(activation: Arc<A>, rpc_converter: F) -> TransportServerBuilder<A>
    where
        F: FnOnce(Arc<A>) -> Result<RpcModule<()>> + Send + 'static,
    {
        TransportServerBuilder::new(activation, rpc_converter)
    }
}

// Usage
TransportServer::builder(activation, converter)
    .with_websocket(8888)
    .build().await?
    .serve().await?;
```

**Why this design:**
- Single starting point: `TransportServer::builder()`
- Clear lifecycle: `builder()` → `build()` → `serve()`
- Composable: Add/remove transports easily
- Discoverable: IDE autocomplete guides users

### Phase 6: Handle Type System Challenges

**Goal**: Solve compilation errors and lifetime issues.

#### Challenge 1: FnOnce in Struct

**Problem:**
```rust
pub struct Builder<A> {
    converter: FnOnce(Arc<A>) -> Result<...>,  // ← Can't store FnOnce!
}
```

**Solution: Box<dyn FnOnce>**
```rust
type RpcConverter<A> = Box<dyn FnOnce(Arc<A>) -> Result<RpcModule<()>> + Send>;

pub struct Builder<A> {
    converter: Option<RpcConverter<A>>,  // ← Boxed, Option for take()
}

// Use it
let converter = self.converter.take().ok_or(...)?;
let module = converter(activation)?;
```

#### Challenge 2: Clone Bounds on Generics

**Problem:**
```rust
#[derive(Clone)]
pub struct Bridge<A: Activation> {
    activation: Arc<A>,  // ← Derive fails if A: !Clone
}
```

**Solution: Manual Clone Implementation**
```rust
impl<A: Activation> Clone for Bridge<A> {
    fn clone(&self) -> Self {
        Self {
            activation: self.activation.clone(),  // ← Arc::clone, not A::clone
        }
    }
}
```

#### Challenge 3: Version Conflicts

**Problem:**
```rust
// substrate uses jsonrpsee 0.25
// hub-transport uses jsonrpsee 0.26
// Conflict on RpcModule<()> type!
```

**Solution: Version Alignment**
```toml
# hub-transport/Cargo.toml
[dependencies]
jsonrpsee = { version = "0.26", features = ["server"] }

# substrate/Cargo.toml
[dependencies]
jsonrpsee = { version = "0.26", features = ["server", "client"] }
```

**Always check:** `cargo tree` to detect version conflicts early.

#### Challenge 4: Feature Flag Configuration

**Problem:**
```rust
#[cfg(feature = "sqlite-sessions")]
pub struct SqliteManager { ... }

// How to make this conditional in API?
```

**Solution: Enum with Conditional Variants**
```rust
pub enum SessionStorage {
    InMemory,
    #[cfg(feature = "sqlite-sessions")]
    Sqlite { path: PathBuf },
}

impl Default for SessionStorage {
    fn default() -> Self {
        Self::InMemory
    }
}
```

### Phase 7: Migrate the Application

**Goal**: Replace application code with library usage.

#### Migration Steps

**1. Add dependency**
```toml
# substrate/Cargo.toml
[dependencies]
hub-transport = { path = "../hub-transport" }
```

**2. Replace imports**
```rust
// Before
use crate::mcp_bridge::PlexusMcpBridge;

// After
use hub_transport::{TransportServer, McpHttpConfig};
```

**3. Replace implementation**
```rust
// Before: ~200 lines of transport setup
let ws_server = Server::builder().build(addr).await?;
let ws_handle = ws_server.start(module);

let mcp_plexus = build_plexus().await;
let bridge = PlexusMcpBridge::new(mcp_plexus);
let session_manager = SqliteSessionManager::new(...).await?;
let service = StreamableHttpService::new(...);
let app = Router::new().nest_service("/mcp", service);
// ... etc

// After: ~20 lines using builder API
let rpc_converter = |arc| Plexus::arc_into_rpc_module(arc);

TransportServer::builder(plexus, rpc_converter)
    .with_websocket(args.port)
    .with_mcp_http(args.port + 1)
    .build().await?
    .serve().await?;
```

**4. Remove old code**
```bash
# Remove functions that moved to library
git rm src/stdio_transport.rs
git rm src/mcp_bridge.rs  # If no other users

# Update lib.rs exports
# Remove re-exports that are now in hub-transport
```

**5. Test equivalence**
```bash
# Verify WebSocket works
wscat -c ws://localhost:4444

# Verify MCP HTTP works
curl http://localhost:4445/mcp

# Verify stdio works
echo '{"jsonrpc":"2.0","id":1,"method":"plexus.schema"}' | cargo run --bin substrate -- --stdio
```

### Phase 8: Document and Test

**Goal**: Ensure library is usable and maintainable.

#### Documentation Checklist

- [ ] **README.md** - Overview, installation, usage examples
- [ ] **API docs** - Rustdoc on all public items
- [ ] **Architecture doc** - Design decisions, patterns, rationale
- [ ] **Examples** - Working code for common use cases
- [ ] **Migration guide** - How to upgrade from old pattern

#### Testing Strategy

**Unit Tests**
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_defaults() {
        let config = StdioConfig::default();
        assert_eq!(config.subscription_buffer_size, 1024);
    }

    #[test]
    fn test_schema_transformation() {
        let schema = create_test_schema();
        let tools = schemas_to_rmcp_tools(vec![schema]);
        assert_eq!(tools.len(), 3);
    }
}
```

**Integration Tests**
```rust
// tests/integration.rs
#[tokio::test]
async fn test_websocket_transport() {
    let activation = Arc::new(TestActivation::new());
    let converter = |arc| Ok(arc.into_rpc_methods());

    let server = TransportServer::builder(activation, converter)
        .with_websocket(0)  // Random port
        .build().await.unwrap();

    // Test connection and RPC call
    // ...
}
```

**Manual Testing Checklist**
- [ ] Standalone plugin works (JsExec)
- [ ] Full hub works (Plexus)
- [ ] Nested hub works (Solar)
- [ ] All three transports functional
- [ ] MCP client integration (Claude Desktop)
- [ ] WebSocket client integration
- [ ] Stdio mode with MCP Inspector

## Common Pitfalls and Solutions

### Pitfall 1: Forgetting to Preserve Arc References

**Problem:**
```rust
// This drops the Arc, invalidating Weak references!
let activation = Arc::new(plexus);
let module = Arc::try_unwrap(activation).unwrap().into_rpc_methods();
// Weak<Plexus> now invalid!
```

**Solution:**
```rust
// Keep Arc alive, clone when needed
pub struct TransportServer<A> {
    activation: Arc<A>,  // ← Original Arc stays here
}

// Provide callback for custom conversion
let converter = |arc: Arc<Plexus>| {
    Plexus::arc_into_rpc_module(arc)  // ← Doesn't drop Arc
};
```

### Pitfall 2: Hardcoding Server Identity

**Problem:**
```rust
// All servers report as "hub-transport"
server_info: Implementation::from_build_env(),
```

**Solution:**
```rust
// Use activation's identity
server_info.name = activation.namespace().to_string();
server_info.version = activation.version().to_string();
```

### Pitfall 3: Tight Coupling to Parent Types

**Problem:**
```rust
// Bridge tightly coupled to Plexus
impl ServerHandler for PlexusMcpBridge {
    fn list_tools(&self) -> ... {
        self.plexus.list_plugin_schemas()  // ← Plexus-specific method
    }
}
```

**Solution:**
```rust
// Use trait methods only
impl<A: Activation> ServerHandler for ActivationMcpBridge<A> {
    fn list_tools(&self) -> ... {
        let schema = self.activation.plugin_schema();  // ← Trait method
        schemas_to_rmcp_tools(vec![schema])
    }
}
```

### Pitfall 4: Ignoring Async Context

**Problem:**
```rust
// Synchronous builder, but needs async initialization
pub fn build(self) -> TransportServer<A> {
    // Can't await here!
    let session_manager = SqliteSessionManager::new(...).await?;
}
```

**Solution:**
```rust
// Make build() async
pub async fn build(self) -> Result<TransportServer<A>> {
    // Can await initialization
    if needs_db {
        self.init_database().await?;
    }
    Ok(TransportServer { ... })
}
```

### Pitfall 5: Version Misalignment

**Problem:**
```rust
// Parent uses jsonrpsee 0.25
// Library uses jsonrpsee 0.26
// RpcModule<()> types don't match!
```

**Solution:**
- Check `cargo tree` for version conflicts
- Align major versions across crates
- Document version requirements in README

## Lessons Learned from hub-transport

### What Went Well

✅ **Callback pattern for RPC conversion**
- Elegant solution to self-consumption
- Delegates Arc lifecycle to user
- Works with any conversion strategy

✅ **Builder pattern for configuration**
- Clean, discoverable API
- Easy to add new transports
- Type-safe configuration

✅ **Generic over Activation trait**
- Works with any activation type
- No code duplication per type
- Future-proof design

✅ **Modular transport design**
- Each transport in its own module
- Can be toggled independently
- Easy to add new transports

### What Could Be Improved

⚠️ **RPC converter boilerplate**
- Users must provide converter for each activation
- Could have trait default with Clone bound
- Trade-off: flexibility vs. convenience

⚠️ **Error type proliferation**
- Each module has its own error types
- Could use unified error enum
- Trade-off: specificity vs. simplicity

⚠️ **Feature flag complexity**
- SQLite sessions require feature flag
- Adds complexity to config types
- Trade-off: minimal deps vs. convenience

### Key Insights

1. **Find the right abstraction boundary**
   - Not too specific (hardcoded types)
   - Not too generic (loses utility)
   - `Activation` trait was perfect level

2. **Lifecycle is the hard part**
   - Type system challenges are solvable
   - Arc/Weak patterns need careful thought
   - Callback pattern is powerful tool

3. **Migration is incremental**
   - Extract → generalize → test → migrate
   - Keep old code working during transition
   - Verify equivalence at each step

4. **Documentation is critical**
   - Examples show the "happy path"
   - Architecture docs explain "why"
   - Migration guides ease adoption

## Checklist for Future Extractions

### Planning Phase
- [ ] Identify code duplication or reuse opportunities
- [ ] Find the trait/interface that makes code generic
- [ ] Map out lifecycle requirements (Arc, Weak, 'static)
- [ ] Check for self-consuming methods
- [ ] Verify generalization is possible

### Extraction Phase
- [ ] Create new crate with proper structure
- [ ] Copy code as-is first
- [ ] Replace concrete types with generics
- [ ] Extract configuration into structs
- [ ] Design builder API
- [ ] Handle type system challenges

### Migration Phase
- [ ] Add library as dependency
- [ ] Replace old code with library usage
- [ ] Verify functional equivalence
- [ ] Remove duplicate code
- [ ] Update documentation

### Verification Phase
- [ ] Write unit tests
- [ ] Write integration tests
- [ ] Manual testing with real use cases
- [ ] Performance testing (if critical)
- [ ] Documentation review

### Release Phase
- [ ] README with examples
- [ ] Architecture documentation
- [ ] Migration guide for users
- [ ] Changelog entry
- [ ] Version number (semver)

## Related Patterns

### Plugin Architecture
Extracting infrastructure is similar to plugin systems:
- Define trait for extension points
- Generic over the trait
- Builder for configuration

### Middleware Pattern
Transport layers are middleware:
- Wrap core functionality
- Add cross-cutting concerns
- Composable and reusable

### Strategy Pattern
Transport selection is strategy pattern:
- Multiple implementations (WebSocket, stdio, MCP)
- Swap at runtime via builder
- Uniform interface

## Conclusion

Infrastructure extraction is a powerful refactoring technique that:
- Reduces code duplication
- Improves reusability
- Clarifies abstractions
- Enables testing in isolation

The key challenges are:
1. Finding the right abstraction boundary
2. Handling lifecycle (Arc/Weak/ownership)
3. Creating ergonomic APIs
4. Maintaining equivalence during migration

The hub-transport extraction demonstrates that with careful planning and incremental migration, even complex infrastructure can be extracted successfully.

**Success metrics:**
- 10x reduction in code (200 → 20 lines)
- Works with any Activation type
- All functionality preserved
- Clean, documented API
- Multiple projects can now reuse

Apply this pattern to extract other infrastructure:
- Storage backends
- Authentication layers
- Caching systems
- Monitoring/metrics
- Rate limiting

The pattern scales to any infrastructure that follows a common interface.
