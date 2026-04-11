//! Secure transport validation via X-Forwarded-Proto header.
//!
//! In production, TLS is terminated by the ingress controller (nginx/envoy).
//! The application validates the `X-Forwarded-Proto` header to confirm the
//! original connection was secure.

use std::sync::OnceLock;

use plexus_core::{
    plexus::PlexusError,
    request::{PlexusRequestField, RawRequestContext},
};

static REQUIRE_SECURE: OnceLock<bool> = OnceLock::new();

/// Enable secure transport enforcement. When enabled, requests without
/// `X-Forwarded-Proto: https` (or `wss`) are rejected, except from loopback.
pub fn init_require_secure_transport(require: bool) {
    let _ = REQUIRE_SECURE.set(require);
}

/// Validated secure transport marker.
///
/// Extraction succeeds when:
/// - Enforcement is disabled (dev mode), OR
/// - `X-Forwarded-Proto` is `https` or `wss`, OR
/// - Connection is from loopback (local dev without proxy)
#[derive(Debug, Clone)]
pub struct SecureTransport;

impl PlexusRequestField for SecureTransport {
    fn extract_from_raw(ctx: &RawRequestContext) -> Result<Self, PlexusError> {
        let required = REQUIRE_SECURE.get().copied().unwrap_or(false);
        if !required {
            return Ok(SecureTransport);
        }

        let proto = ctx.headers.get("x-forwarded-proto")
            .and_then(|v| v.to_str().ok());

        match proto {
            Some("https") | Some("wss") => Ok(SecureTransport),
            None => {
                // No proxy header -- check if loopback (dev)
                match ctx.peer {
                    Some(addr) if addr.ip().is_loopback() => Ok(SecureTransport),
                    _ => Err(PlexusError::Unauthenticated(
                        "Secure transport required (X-Forwarded-Proto header missing)".into()
                    )),
                }
            }
            Some(other) => Err(PlexusError::Unauthenticated(
                format!("Secure transport required, got X-Forwarded-Proto: {}", other)
            )),
        }
    }
}
