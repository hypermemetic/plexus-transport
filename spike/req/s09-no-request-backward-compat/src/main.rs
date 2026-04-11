use futures::stream::Stream;
use plexus_core::Activation;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// Simulating existing FormVeritas-style activation with no request = Type
struct FormsStub;

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "event", rename_all = "snake_case")]
enum FormEvent {
    List { count: u32 },
    Error { message: String },
}

#[plexus_macros::activation(
    namespace = "forms",
    version = "1.0.0",
    description = "USCIS form index",
    crate_path = "plexus_core"
)]
impl FormsStub {
    #[plexus_macros::method(description = "List tracked forms")]
    async fn list(&self) -> impl Stream<Item = FormEvent> + Send + 'static {
        async_stream::stream! {
            yield FormEvent::List { count: 42 };
        }
    }

    #[plexus_macros::method(description = "Get form by slug")]
    async fn get(&self, slug: String) -> impl Stream<Item = FormEvent> + Send + 'static {
        async_stream::stream! {
            yield FormEvent::Error { message: format!("not found: {}", slug) };
        }
    }
}

fn main() {
    let activation = FormsStub;
    let schema = Activation::plugin_schema(&activation);
    let json = serde_json::to_string_pretty(&schema).unwrap();
    println!("Existing activation schema:\n{}", json);

    let v: serde_json::Value = serde_json::from_str(&json).unwrap();

    // No request field expected
    assert!(
        v.get("request").is_none() || v["request"].is_null(),
        "existing activation should have no 'request' field"
    );

    // Methods still present
    assert!(v["methods"].is_array());
    let methods = v["methods"].as_array().unwrap();
    assert!(!methods.is_empty(), "should have methods");

    println!("\nS-09: OK — existing activation without request = Type compiles and schemas correctly");
    println!("      No 'request' field in output (correct)");
}
