#[derive(Debug)]
struct AuthReq {
    auth_token: String,
    origin: Option<String>,
}

fn parse_cookie<'a>(cookie_str: &'a str, name: &str) -> Option<&'a str> {
    cookie_str.split(';').find_map(|part| {
        let part = part.trim();
        part.strip_prefix(name)
            .and_then(|rest| rest.strip_prefix('='))
    })
}

fn extract(cookies: &str, headers: &[(&str, &str)]) -> Result<AuthReq, String> {
    let auth_token = parse_cookie(cookies, "access_token")
        .ok_or_else(|| "Authentication required: access_token cookie missing".to_string())?;
    let origin = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("origin"))
        .map(|(_, v)| v.to_string());
    Ok(AuthReq {
        auth_token: auth_token.to_string(),
        origin,
    })
}

fn main() {
    // Missing cookie → Err with clear message
    let err = extract("other=val", &[]).unwrap_err();
    assert!(
        err.contains("access_token"),
        "Expected access_token in error, got: {}",
        err
    );
    println!("missing cookie error: {}", err);

    // Present cookie → Ok
    let req = extract("access_token=tok123", &[]).unwrap();
    assert_eq!(req.auth_token, "tok123");
    assert!(req.origin.is_none());

    // Optional absent → None
    let req = extract("access_token=tok123", &[]).unwrap();
    assert!(req.origin.is_none(), "absent optional should be None");

    // Optional present → Some
    let req = extract(
        "access_token=tok123",
        &[("origin", "https://app.example.com")],
    )
    .unwrap();
    assert_eq!(req.origin.as_deref(), Some("https://app.example.com"));

    // Multiple cookies
    let req = extract("session=abc; access_token=tok456; other=xyz", &[]).unwrap();
    assert_eq!(req.auth_token, "tok456");

    println!("S-03: OK — required field gates correctly, Option<T> fields are truly optional");
}
