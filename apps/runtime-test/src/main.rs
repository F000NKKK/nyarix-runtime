//! Manual smoke-test binary for exercising the Runtime during development.

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    tracing::info!("nyarix runtime-test placeholder binary");
}
