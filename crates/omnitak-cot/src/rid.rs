//! Native Remote ID to CoT decode.
//!
//! The ESP32 drone detector described in `claude-interface/COT_HANDLING_NOTES.md`
//! scans Wi-Fi (beacon/NAN) and Bluetooth for Remote ID broadcasts in both
//! ODID (OpenDroneID / ASTM F3411) and French (ASD-STAN EN 4709-002) formats and
//! is expected to produce `a-u-A` drone CoT. This module decodes those raw
//! broadcast frames in Rust and emits an [`Event`] of exactly that type, instead
//! of hand-handling partial XML from an external detector.
//!
//! The decode uses the [`remoteid`] crate to parse the broadcast and merge its
//! messages into one per-aircraft record, and the [`uas_cot`] crate to turn that
//! record into a CoT event, which is then parsed back into this crate's [`Event`]
//! so it flows through the same pipeline as any other CoT message.

use crate::event::Event;
use crate::parser::{parse_cot, ParseError};

/// Error decoding a Remote ID broadcast into a CoT event.
#[derive(Debug, thiserror::Error)]
pub enum RidError {
    /// The payload is not a valid Remote ID frame.
    #[error("Remote ID decode failed: {0}")]
    Decode(#[from] remoteid::Error),
    /// The frame carries no usable aircraft ID or position, so no track can be
    /// placed on the map.
    #[error("Remote ID record has no id or position to place a track")]
    Incomplete,
    /// The generated CoT could not be parsed back into an event.
    #[error("CoT parse error: {0}")]
    Cot(#[from] ParseError),
}

/// Decodes one raw Remote ID broadcast frame (ODID or French format, as carried
/// by a Wi-Fi beacon/NAN action frame or a Bluetooth advertisement) into a CoT
/// `a-u-A` (unknown air) [`Event`].
///
/// `now_us` is the current time in microseconds since the Unix epoch, used for
/// the event timestamps.
///
/// ```
/// use omnitak_cot::rid::event_from_remoteid;
/// use remoteid::{BasicId, IdType, Location, Message, MessagePackBuilder, UaType};
///
/// let id = BasicId::new(IdType::SerialNumber, UaType::HelicopterOrMultirotor, "1596F35074529ABCDEFG").unwrap();
/// let mut loc = Location::default();
/// loc.latitude = 45.5470818;
/// loc.longitude = -122.9668346;
/// loc.altitude_geo = Some(237.0);
///
/// let mut pack = MessagePackBuilder::new();
/// pack.push(&Message::BasicId(id)).unwrap();
/// pack.push(&Message::Location(loc)).unwrap();
///
/// let event = event_from_remoteid(pack.as_bytes(), 1_700_000_000_000_000).unwrap();
/// assert!(event.event_type.starts_with("a-u-A"));
/// assert_eq!(event.uid, "rid-1596F35074529ABCDEFG");
/// ```
pub fn event_from_remoteid(payload: &[u8], now_us: u64) -> Result<Event, RidError> {
    let frame = remoteid::Frame::decode(payload)?;
    let mut uas = remoteid::UasData::default();
    uas.update_frame(&frame);

    let cot = uas_cot::rid::aircraft(&uas, None, now_us, &uas_cot::rid::Options::default())
        .ok_or(RidError::Incomplete)?;

    Ok(parse_cot(&cot.to_xml())?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use remoteid::{BasicId, IdType, Location, Message, MessagePackBuilder, UaType};

    #[test]
    fn decodes_wifi_remoteid_pack_to_drone_cot() {
        let id = BasicId::new(
            IdType::SerialNumber,
            UaType::HelicopterOrMultirotor,
            "1596F35074529ABCDEFG",
        )
        .unwrap();
        let mut loc = Location::default();
        loc.latitude = 45.5470818;
        loc.longitude = -122.9668346;
        loc.altitude_geo = Some(237.0);

        let mut pack = MessagePackBuilder::new();
        pack.push(&Message::BasicId(id)).unwrap();
        pack.push(&Message::Location(loc)).unwrap();

        let event = event_from_remoteid(pack.as_bytes(), 1_700_000_000_000_000).unwrap();

        assert!(
            event.event_type.starts_with("a-u-A"),
            "expected unknown-air CoT type, got {}",
            event.event_type
        );
        assert_eq!(event.uid, "rid-1596F35074529ABCDEFG");
        assert_eq!(event.point.hae, 237.0);
        assert!((event.point.lat - 45.5470818).abs() < 1e-5);
        assert!((event.point.lon - -122.9668346).abs() < 1e-5);
    }

    #[test]
    fn rejects_non_remoteid_payload() {
        let err = event_from_remoteid(&[0u8; 4], 0).unwrap_err();
        assert!(matches!(err, RidError::Decode(_)));
    }
}
