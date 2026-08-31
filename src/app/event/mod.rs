use crate::ica::BridgeEvent;

use super::media::MediaEvent;

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

#[derive(Debug)]
pub enum AppEvent {
    Bridge(BridgeEvent),
    Media(MediaEvent),
}
