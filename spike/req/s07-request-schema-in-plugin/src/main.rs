use futures::stream::Stream;
use plexus_core::Activation;

struct SchemaTestActivation;

#[plexus_macros::activation(
    namespace = "s07_schema_test",
    version = "1.0.0",
    description = "Schema test",
    crate_path = "plexus_core"
)]
impl SchemaTestActivation {
    #[plexus_macros::method(description = "list clients")]
    async fn list(&self, search: Option<String>) -> impl Stream<Item = String> + Send + 'static {
        async_stream::stream! { yield search.unwrap_or_default(); }
    }
}

fn main() {
    let activation = SchemaTestActivation;
    let schema = Activation::plugin_schema(&activation);
    let json = serde_json::to_string_pretty(&schema).unwrap();
    println!("Current plugin_schema output:\n{}", json);

    let v: serde_json::Value = serde_json::from_str(&json).unwrap();

    // Document current state:
    let has_request_field = v.get("request").is_some();
    println!("\nhas 'request' field in plugin_schema: {}", has_request_field);

    if has_request_field {
        println!("S-07: request field already present (unexpected)");
    } else {
        println!("S-07: OK — baseline established, 'request' field absent (expected)");
        println!("NOTE: Need to add request field to PluginSchema type and plugin_schema() codegen");
    }

    // Show current fields
    if let Some(obj) = v.as_object() {
        println!(
            "Current plugin_schema keys: {:?}",
            obj.keys().collect::<Vec<_>>()
        );
    }
}
