mod announcement;
mod connection;
mod contacts;
mod forward;
mod group;
mod login;
mod message;
mod misc;
mod payload;
mod reducer;
mod room;
mod search;

use super::media::MediaEvent;
use crate::ica::BridgeEvent;

#[derive(Debug)]
pub enum AppEvent {
    AgentContext(crate::agent_context::server::GuiRequest),
    Bridge(BridgeEvent),
    Media(MediaEvent),
}
