//! Raw HTTP request context for PlexusRequest extraction.
//!
//! `RawRequestContext` is now defined in `plexus-core` so that the `Activation`
//! trait can reference it without creating a circular dependency. This module
//! re-exports it for backward compatibility.

/// Re-export `RawRequestContext` from plexus-core.
pub use plexus_core::request::RawRequestContext;
