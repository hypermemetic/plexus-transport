//! PlexusRequest trait — re-exported from plexus-core for backward compatibility.
//!
//! The trait is now defined in `plexus_core::request` so that the `Activation`
//! trait's generated dispatch code can reference it via `#crate_path::request::PlexusRequest`
//! without requiring `plexus_transport` as a dependency in activation crates.

/// Re-export `PlexusRequest` from plexus-core.
pub use plexus_core::request::PlexusRequest;
