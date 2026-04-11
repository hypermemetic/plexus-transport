use futures::stream::Stream;
use plexus_core::Activation;

struct StripTestActivation;

// Test 1: verify baseline method param schema before we add #[activation_param].
// This documents current behavior so we can compare after adding the attribute.
#[plexus_macros::activation(namespace = "s06_strip_test", version = "1.0.0", crate_path = "plexus_core")]
impl StripTestActivation {
    #[plexus_macros::method(description = "list")]
    async fn list(
        &self,
        // #[activation_param] auth_token: String,  // uncomment to test — expected compile error
        search: Option<String>,
    ) -> impl Stream<Item = String> + Send + 'static {
        async_stream::stream! {
            yield search.unwrap_or_else(|| "all".to_string());
        }
    }
}

fn main() {
    let activation = StripTestActivation;
    let plugin = activation.plugin_schema();
    let plugin_json = serde_json::to_value(&plugin).unwrap();

    let methods = plugin_json["methods"].as_array().unwrap();
    let list_method = methods.iter().find(|m| m["name"].as_str() == Some("list")).unwrap();

    let params = &list_method["params"];
    println!(
        "list params schema: {}",
        serde_json::to_string_pretty(params).unwrap()
    );

    // search should be a param
    let has_search = params["properties"]["search"].is_object();
    println!("has search param: {}", has_search);

    println!("\nS-06: OK — baseline param schema established");
    println!("NOTE: #[activation_param] not yet in parse.rs");
    println!("      When added, auth_token should NOT appear in params schema");
}
