use rust_socketio::asynchronous::{Client, ClientBuilder};
use rust_socketio::{Event, Payload, TransportType};
use rust_socketio::{async_any_callback, async_callback};

use crate::StopGetter;
use crate::cfg::IcaBridge;

pub mod events;
pub mod types;

/// icalingua 客户端的兼容版本号
pub const ICA_PROTOCOL_VERSION: &str = "2.12.28";

#[derive(Debug, Clone)]
pub struct IcaClient {}

pub async fn main(stop_alrm: StopGetter, bridge_cfg: &IcaBridge) -> anyhow::Result<()> {
    if !bridge_cfg.enable {
        return Ok(());
    }

    let start_connect_time = std::time::Instant::now();

    let client = match ClientBuilder::new(bridge_cfg.url.clone())
        .transport_type(TransportType::Websocket)
        .on_any(async_any_callback!(events::any_event))
        .connect()
        .await
    {
        Ok(client) => client,
        Err(e) => return Err(e.into()),
    };
    stop_alrm.await.ok();
    match client.disconnect().await {
        Ok(_) => {
            println!("Disconnected")
        }
        Err(e) => {
            println!("Failed to disconnect: {}", e);
        }
    }

    Ok(())
}
