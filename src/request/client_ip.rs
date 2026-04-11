//! Client IP extraction from proxy headers.
//!
//! In production behind an ingress controller, `ctx.peer` shows the sidecar/proxy IP.
//! This extractor reads `X-Forwarded-For` or `X-Real-IP` to recover the real client IP.

use std::net::IpAddr;
use std::sync::OnceLock;

use plexus_core::{
    plexus::PlexusError,
    request::{PlexusRequestField, RawRequestContext},
};

static TRUST_PROXY: OnceLock<bool> = OnceLock::new();

/// Enable trusting proxy headers (X-Forwarded-For, X-Real-IP).
/// Only enable when running behind a trusted reverse proxy.
pub fn init_trust_proxy_headers(trust: bool) {
    let _ = TRUST_PROXY.set(trust);
}

/// The real client IP address, extracted from proxy headers when trusted.
#[derive(Debug, Clone)]
pub struct ClientIp(pub IpAddr);

impl PlexusRequestField for ClientIp {
    fn extract_from_raw(ctx: &RawRequestContext) -> Result<Self, PlexusError> {
        let trust = TRUST_PROXY.get().copied().unwrap_or(false);

        if trust {
            // X-Forwarded-For: client, proxy1, proxy2 — first entry is the real client
            if let Some(xff) = ctx
                .headers
                .get("x-forwarded-for")
                .and_then(|v| v.to_str().ok())
            {
                if let Some(first) = xff.split(',').next().map(|s| s.trim()) {
                    if let Ok(ip) = first.parse::<IpAddr>() {
                        return Ok(ClientIp(ip));
                    }
                }
            }
            // Fallback: X-Real-IP
            if let Some(xri) = ctx
                .headers
                .get("x-real-ip")
                .and_then(|v| v.to_str().ok())
            {
                if let Ok(ip) = xri.trim().parse::<IpAddr>() {
                    return Ok(ClientIp(ip));
                }
            }
        }

        // No proxy headers or not trusted — use peer address
        match ctx.peer {
            Some(addr) => Ok(ClientIp(addr.ip())),
            None => Ok(ClientIp(IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED))),
        }
    }
}
