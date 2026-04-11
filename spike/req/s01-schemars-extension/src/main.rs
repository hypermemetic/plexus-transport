use schemars::JsonSchema;
use serde_json::json;

#[derive(JsonSchema)]
struct Req {
    /// JWT from Keycloak
    #[schemars(extend("x-plexus-source" = {"from": "cookie", "key": "access_token"}))]
    auth_token: String,

    #[schemars(extend("x-plexus-source" = {"from": "header", "key": "origin"}))]
    origin: Option<String>,

    #[schemars(extend("x-plexus-source" = {"from": "derived"}))]
    peer_addr: Option<String>, // using String for simplicity (SocketAddr needs extra derives)
}

fn main() {
    let schema = schemars::schema_for!(Req);
    let v = serde_json::to_value(&schema).unwrap();
    println!("{}", serde_json::to_string_pretty(&v).unwrap());

    // Verify auth_token has x-plexus-source
    let src = &v["properties"]["auth_token"]["x-plexus-source"];
    assert_eq!(src["from"], json!("cookie"), "auth_token source should be cookie");
    assert_eq!(src["key"], json!("access_token"), "auth_token key should be access_token");

    // Verify origin
    let src = &v["properties"]["origin"]["x-plexus-source"];
    assert_eq!(src["from"], json!("header"));

    // Verify peer_addr
    let src = &v["properties"]["peer_addr"]["x-plexus-source"];
    assert_eq!(src["from"], json!("derived"));

    println!("\nS-01: OK — #[schemars(extend(...))] works for x-plexus-source");
}
