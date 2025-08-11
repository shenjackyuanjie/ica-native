use rust_socketio::asynchronous::{Client, ClientBuilder};
use rust_socketio::{Event, Payload, TransportType};
use rust_socketio::{async_any_callback, async_callback};

use crate::StopGetter;

pub async fn main(stop_alrm: StopGetter) -> anyhow::Result<()> {

    stop_alrm.await.ok();

    Ok(())
}
