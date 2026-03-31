mod app;

use anyhow::Result;
use tracing_subscriber;

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    tracing::info!("Starting Nexus terminal");

    app::run()
}
