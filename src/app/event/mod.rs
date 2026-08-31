use crate::ica::BridgeEvent;

use super::media::MediaEvent;

mod announcement;
mod reducer;

#[derive(Debug)]
pub enum AppEvent {
    Bridge(BridgeEvent),
    Media(MediaEvent),
}
