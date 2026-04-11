use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(JsonSchema, Serialize, Deserialize)]
struct MixedRequest {
    // required (non-optional)
    #[schemars(extend("x-plexus-source" = {"from": "cookie", "key": "access_token"}))]
    auth_token: String,

    // optional
    #[schemars(extend("x-plexus-source" = {"from": "header", "key": "origin"}))]
    origin: Option<String>,

    // derived + optional
    #[schemars(extend("x-plexus-source" = {"from": "derived"}))]
    peer_addr: Option<String>,
}

fn main() {
    let schema = schemars::schema_for!(MixedRequest);
    let v = serde_json::to_value(&schema).unwrap();
    println!("{}", serde_json::to_string_pretty(&v).unwrap());

    let required = v["required"].as_array().cloned().unwrap_or_default();
    let req_names: Vec<&str> = required.iter().filter_map(|s| s.as_str()).collect();
    println!("\nrequired fields: {:?}", req_names);

    assert!(
        req_names.contains(&"auth_token"),
        "auth_token (String) should be required"
    );
    assert!(
        !req_names.contains(&"origin"),
        "origin (Option<String>) should NOT be required"
    );
    assert!(
        !req_names.contains(&"peer_addr"),
        "peer_addr (Option, derived) should NOT be required"
    );

    // Verify x-plexus-source on each field
    assert_eq!(
        v["properties"]["auth_token"]["x-plexus-source"]["from"],
        json!("cookie")
    );
    assert_eq!(
        v["properties"]["origin"]["x-plexus-source"]["from"],
        json!("header")
    );
    assert_eq!(
        v["properties"]["peer_addr"]["x-plexus-source"]["from"],
        json!("derived")
    );

    println!("\nS-08: OK — schemars correctly puts String in required, Option<T> not required");
    println!("      x-plexus-source present on all fields");
}
