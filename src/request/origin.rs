//! Origin validation for Plexus HTTP/WebSocket handlers.
//!
//! `ValidOrigin` is a newtype that implements `PlexusRequestField` to extract and
//! validate the HTTP `Origin` header against a configurable allowlist.
//!
//! # Usage
//!
//! Call [`init_allowed_origins`] once at startup with the list of permitted origins.
//! Then include `origin: ValidOrigin` as a field in any `#[derive(PlexusRequest)]` struct —
//! the macro will call `ValidOrigin::extract_from_raw(ctx)?` automatically.
//!
//! ## Empty-string sentinel
//!
//! When the `Origin` header is absent (non-browser clients: CLI, synapse, etc.), the
//! extraction succeeds and returns `ValidOrigin("")`. Callers can check `origin.0.is_empty()`
//! to distinguish browser from non-browser clients.
//!
//! ## Uninitialised / empty allowlist
//!
//! If [`init_allowed_origins`] has never been called, or was called with an empty list,
//! **all** origins are permitted. This is the safe default for servers that do not need
//! CORS enforcement.

use std::sync::OnceLock;

use plexus_core::{
    plexus::PlexusError,
    request::{PlexusRequestField, RawRequestContext},
};

/// The allowlist set once at startup via [`init_allowed_origins`].
static ALLOWED_ORIGINS: OnceLock<Vec<String>> = OnceLock::new();

/// Initialise the allowlist of permitted `Origin` header values.
///
/// This function is a no-op if called more than once (the `OnceLock` only stores
/// the first value). Call it once at application startup before serving any requests.
///
/// Passing an empty `Vec` is equivalent to not calling this function at all —
/// all origins will be permitted.
pub fn init_allowed_origins(origins: Vec<String>) {
    let _ = ALLOWED_ORIGINS.set(origins);
}

/// A validated `Origin` header value.
///
/// - `ValidOrigin("")` — no `Origin` header was present (non-browser client).
/// - `ValidOrigin(origin)` — the origin was present and is in the allowlist.
///
/// Extraction fails with `PlexusError::Unauthenticated` if the `Origin` header is
/// present but not in the allowlist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidOrigin(pub String);

impl PlexusRequestField for ValidOrigin {
    fn extract_from_raw(
        ctx: &RawRequestContext,
    ) -> Result<Self, PlexusError> {
        // Retrieve the Origin header value, if any.
        let origin_opt = ctx
            .headers
            .get("origin")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_owned());

        let origin = match origin_opt {
            // No Origin header → non-browser client, always pass through.
            None => return Ok(ValidOrigin(String::new())),
            Some(o) => o,
        };

        // Check the allowlist.
        match ALLOWED_ORIGINS.get() {
            // Not initialised or empty → all origins pass.
            None => Ok(ValidOrigin(origin)),
            Some(list) if list.is_empty() => Ok(ValidOrigin(origin)),
            Some(list) => {
                if list.iter().any(|allowed| allowed == &origin) {
                    Ok(ValidOrigin(origin))
                } else {
                    Err(PlexusError::Unauthenticated(format!(
                        "Origin '{}' is not allowed",
                        origin
                    )))
                }
            }
        }
    }
}
