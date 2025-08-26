use rust_socketio::asynchronous::{Client, ClientBuilder};
use rust_socketio::{Event, Payload, TransportType};
use rust_socketio::{async_any_callback, async_callback};

use crate::StopGetter;

pub mod events;

pub async fn main(stop_alrm: StopGetter) -> anyhow::Result<()> {
    let start_connect_time = std::time::Instant::now();

    let client = match ClientBuilder::new("test")
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
