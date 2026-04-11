//! REQ-3: ValidOrigin test that requires uninitialised OnceLock state.
//!
//! MUST be in a separate file (= separate test binary = fresh process) so the
//! OnceLock is never touched by `configured::*` tests before this runs.
//!
//! Run with: cargo test --test origin_uninitialised

use plexus_core::plexus::PlexusRequestField;
use plexus_transport::request::{RawRequestContext, origin::ValidOrigin};

fn make_raw(headers: &[(&str, &str)]) -> RawRequestContext {
    let mut h = http::HeaderMap::new();
    for &(k, v) in headers {
        h.insert(
            http::header::HeaderName::from_bytes(k.as_bytes()).unwrap(),
            v.parse().unwrap(),
        );
    }
    RawRequestContext {
        headers: h,
        uri: "/".parse().unwrap(),
        auth: None,
        peer: None,
    }
}

/// Before init_allowed_origins is called, all origins must pass.
#[test]
fn uninitialised_allows_all() {
    let ctx = make_raw(&[("origin", "https://anything.com")]);
    assert!(
        ValidOrigin::extract_from_raw(&ctx).is_ok(),
        "uninitialised allowlist must pass all origins"
    );
}
