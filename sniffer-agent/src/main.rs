use sniffer_common::logger;

use tracing::info;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _guard = logger::init("magnet-sniffer-agent", None, None, None);
    info!("Starting Sniffer Agent...");

    Ok(())
}
