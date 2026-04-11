trait PlexusRequestField: Sized {
    fn source_annotation() -> serde_json::Value;
    fn extract_from(headers: &[(&str, &str)], peer: Option<&str>) -> Result<Self, String>;
}

// Blanket impl for Option<T>: absent → None, present-but-invalid → Err (NOT None!)
impl<T: PlexusRequestField> PlexusRequestField for Option<T> {
    fn source_annotation() -> serde_json::Value {
        T::source_annotation()
    }
    fn extract_from(headers: &[(&str, &str)], peer: Option<&str>) -> Result<Self, String> {
        // Check if the relevant header is present at all.
        // This is type-specific so we delegate to T and interpret the result:
        // - Ok(v) from T → Some(v) for Option
        // - Err from T where field is ABSENT → None (truly optional)
        // - Err from T where field is PRESENT but INVALID → propagate Err
        // We can't distinguish these without knowing which header T reads.
        // This is the design question: use a separate `is_absent` check.
        match T::extract_from(headers, peer) {
            Ok(v) => Ok(Some(v)),
            Err(e) if e.contains("absent") || e.contains("missing") || e.contains("not present") => {
                Ok(None)
            }
            Err(e) => Err(e), // present-but-invalid propagates
        }
    }
}

const ALLOWED: &[&str] = &["https://app.example.com", "http://localhost:5173"];

#[derive(Debug)]
struct ValidOrigin(pub String);

impl PlexusRequestField for ValidOrigin {
    fn source_annotation() -> serde_json::Value {
        serde_json::json!({"from": "header", "key": "origin"})
    }
    fn extract_from(headers: &[(&str, &str)], _peer: Option<&str>) -> Result<Self, String> {
        match headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("origin"))
            .map(|(_, v)| *v)
        {
            None => Err("origin header not present".to_string()), // absent — marked as "not present"
            Some(o) if ALLOWED.contains(&o) => Ok(ValidOrigin(o.to_string())),
            Some(o) => Err(format!("Origin '{}' not allowed", o)), // present but invalid
        }
    }
}

fn main() {
    // Non-optional ValidOrigin: allowed origin → Ok
    assert!(ValidOrigin::extract_from(&[("origin", "https://app.example.com")], None).is_ok());

    // Non-optional ValidOrigin: disallowed → Err
    let err = ValidOrigin::extract_from(&[("origin", "https://evil.com")], None).unwrap_err();
    assert!(
        err.contains("evil.com"),
        "error should mention the bad origin: {}",
        err
    );
    println!("disallowed origin error: {}", err);

    // Non-optional ValidOrigin: absent → Err
    let result = ValidOrigin::extract_from(&[], None);
    println!(
        "absent origin (non-optional): {:?}",
        result
            .as_ref()
            .map(|v| &v.0)
            .map_err(|e| e.as_str())
    );
    // DESIGN QUESTION: should absent origin be Ok("") or Err for non-optional ValidOrigin?
    // For Option<ValidOrigin>, absent → None.

    // Option<ValidOrigin>: allowed → Some
    let result =
        Option::<ValidOrigin>::extract_from(&[("origin", "https://app.example.com")], None);
    assert!(result.unwrap().is_some());

    // CRITICAL: Option<ValidOrigin>: disallowed → Err (NOT None — silent security hole!)
    let result = Option::<ValidOrigin>::extract_from(&[("origin", "https://evil.com")], None);
    assert!(
        result.is_err(),
        "disallowed origin should be Err even for Option<ValidOrigin>, got: {:?}",
        result.map(|_| "Ok")
    );
    println!("Option<ValidOrigin> with disallowed origin correctly returns Err");

    // Option<ValidOrigin>: absent → None
    let result = Option::<ValidOrigin>::extract_from(&[], None);
    assert!(
        result.unwrap().is_none(),
        "absent origin should be None for Option<ValidOrigin>"
    );

    println!("\nS-04: OK — PlexusRequestField trait works; Option<T> absent→None, present-invalid→Err");
    println!("DESIGN NOTE: ValidOrigin absent (non-optional) needs clarification: Ok(\"\") for CLI path vs Err");
}
