use futures::stream::Stream;

// First test: request = () (unit type = no extraction)
// This should work after we add `request` parsing to HubMethodsAttrs.
// For now: verify current behavior (panic or compile error expected for request = Type).

struct NoRequestActivation;

// Test 1: plain activation with no request — must still work (backward compat, S-09)
#[plexus_macros::activation(namespace = "s05_no_req", version = "1.0.0", crate_path = "plexus_core")]
impl NoRequestActivation {
    #[plexus_macros::method(description = "ping")]
    async fn ping(&self) -> impl Stream<Item = String> + Send + 'static {
        async_stream::stream! { yield "pong".to_string(); }
    }
}

fn main() {
    println!("S-05 part 1: activation with no request = compiles OK");

    // Test 2: request = SomeType — currently expected to fail at compile time.
    // If this compiles, the parser already accepts it (unlikely).
    // If it fails, we document what error we get and what needs changing in parse.rs.

    // Uncomment to test — will likely fail to compile:
    // struct MyReq { auth_token: String }
    // #[plexus_macros::activation(namespace = "s05_with_req", request = MyReq)]
    // impl WithReqActivation {
    //     #[plexus_macros::method]
    //     async fn ping(&self) -> impl Stream<Item = String> + Send + 'static {
    //         async_stream::stream! { yield "ok".to_string(); }
    //     }
    // }

    println!("S-05: OK — baseline activation without request compiles");
    println!("NOTE: request = Type parsing not yet implemented in HubMethodsAttrs");
    println!("      Need to add 'request' key to parse.rs HubMethodsAttrs::parse()");
}
