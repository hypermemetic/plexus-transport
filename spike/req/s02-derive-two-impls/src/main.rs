use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde_json::json;

// No derives — all manual
struct ClientsRequest {
    auth_token: String,
    origin: Option<String>,
    peer_addr: Option<String>,
}

// Simulate what PlexusRequest::extract() would do
impl ClientsRequest {
    fn extract(cookies: &str, headers: &[(&str, &str)], peer: Option<&str>) -> Result<Self, String> {
        let auth_token = parse_cookie(cookies, "access_token")
            .ok_or("access_token cookie required")?
            .to_string();
        let origin = headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("origin"))
            .map(|(_, v)| v.to_string());
        Ok(Self {
            auth_token,
            origin,
            peer_addr: peer.map(|s| s.to_string()),
        })
    }
}

// Manually implement JsonSchema (what the derive would generate)
impl JsonSchema for ClientsRequest {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "ClientsRequest".into()
    }
    fn json_schema(_gen: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "object",
            "properties": {
                "auth_token": {
                    "type": "string",
                    "description": "JWT from Keycloak",
                    "x-plexus-source": {"from": "cookie", "key": "access_token"}
                },
                "origin": {
                    "type": ["string", "null"],
                    "x-plexus-source": {"from": "header", "key": "origin"}
                },
                "peer_addr": {
                    "type": ["string", "null"],
                    "x-plexus-source": {"from": "derived"}
                }
            },
            "required": ["auth_token"]
        })
    }
}

fn parse_cookie<'a>(cookie_str: &'a str, name: &str) -> Option<&'a str> {
    cookie_str.split(';').find_map(|part| {
        let part = part.trim();
        part.strip_prefix(name)
            .and_then(|rest| rest.strip_prefix('='))
    })
}

fn main() {
    // Extraction
    let req = ClientsRequest::extract(
        "access_token=tok123",
        &[("origin", "https://app.example.com")],
        Some("1.2.3.4:5678"),
    )
    .unwrap();
    assert_eq!(req.auth_token, "tok123");
    assert_eq!(req.origin.as_deref(), Some("https://app.example.com"));
    assert_eq!(req.peer_addr.as_deref(), Some("1.2.3.4:5678"));

    // Missing cookie → Err
    assert!(ClientsRequest::extract("other=val", &[], None).is_err());

    // Schema
    let schema = schemars::schema_for!(ClientsRequest);
    let v = serde_json::to_value(&schema).unwrap();
    println!("{}", serde_json::to_string_pretty(&v).unwrap());

    assert_eq!(v["properties"]["auth_token"]["x-plexus-source"]["from"], json!("cookie"));
    assert_eq!(v["required"], json!(["auth_token"]));
    assert!(v["properties"]["peer_addr"]["x-plexus-source"]["from"] == json!("derived"));

    println!("\nS-02: OK — manual JsonSchema impl with x-plexus-source works; extraction and schema coexist");
}
