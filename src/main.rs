//! Thin binary shim: all logic lives in the library's [`agent_habilis_mock::run`].

#[tokio::main]
async fn main() {
    if let Err(error) = agent_habilis_mock::run().await {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}
