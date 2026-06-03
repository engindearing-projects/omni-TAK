//! Protobuf support for CoT messages with TAK Protocol Version 1

use crate::event::{Contact, Detail, Event, Group, Point, PrecisionLocation, Status, Takv, Track};
use chrono::{DateTime, Utc};
use prost::Message;
use std::io::Write;
use thiserror::Error;

// Include generated protobuf code
pub mod pb {
    include!(concat!(env!("OUT_DIR"), "/omnitak.cot.rs"));
}

/// TAK Protocol headers
const MESH_HEADER: &[u8] = &[0xBF, 0x01, 0xBF];

#[derive(Error, Debug)]
pub enum ProtoError {
    #[error("Protobuf encoding error: {0}")]
    EncodeError(#[from] prost::EncodeError),

    #[error("Protobuf decoding error: {0}")]
    DecodeError(#[from] prost::DecodeError),

    #[error("Invalid datetime format: {0}")]
    InvalidDateTime(String),

    #[error("Missing required field: {0}")]
    MissingField(String),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
}

/// Convert DateTime to milliseconds since epoch
fn datetime_to_millis(dt: &DateTime<Utc>) -> u64 {
    (dt.timestamp() * 1000 + dt.timestamp_subsec_millis() as i64) as u64
}

/// Convert Event to protobuf CotEvent format (TAK Protocol Version 1)
impl From<&Event> for pb::CotEvent {
    fn from(event: &Event) -> Self {
        pb::CotEvent {
            version: event.version.clone(),
            uid: event.uid.clone(),
            r#type: event.event_type.clone(),
            time: event.time.to_rfc3339(),
            start: event.start.to_rfc3339(),
            stale: event.stale.to_rfc3339(),
            how: event.how.clone(),
            // Inline point fields (TAK Protocol Version 1)
            lat: event.point.lat,
            lon: event.point.lon,
            hae: event.point.hae,
            ce: event.point.ce,
            le: event.point.le,
            // Point for backward compatibility
            point: Some(pb::Point {
                lat: event.point.lat,
                lon: event.point.lon,
                hae: event.point.hae,
                ce: event.point.ce,
                le: event.point.le,
            }),
            detail: event.detail.as_ref().map(|d| pb::Detail {
                xml_detail: d.xml_detail.clone().unwrap_or_default(),
                contact: d.contact.as_ref().map(|c| pb::Contact {
                    endpoint: c.endpoint.clone().unwrap_or_default(),
                    callsign: c.callsign.clone(),
                }),
                group: d.group.as_ref().map(|g| pb::Group {
                    name: g.name.clone(),
                    role: g.role.clone(),
                }),
                precision_location: d
                    .precision_location
                    .as_ref()
                    .map(|pl| pb::PrecisionLocation {
                        geopointsrc: pl.geopointsrc.clone(),
                        altsrc: pl.altsrc.clone(),
                    }),
                status: d.status.as_ref().map(|s| pb::Status { battery: s.battery }),
                takv: d.takv.as_ref().map(|t| pb::Takv {
                    device: t.device.clone(),
                    platform: t.platform.clone(),
                    os: t.os.clone(),
                    version: t.version.clone(),
                }),
                track: d.track.as_ref().map(|t| pb::Track {
                    speed: t.speed,
                    course: t.course,
                }),
            }),
            // TAK Protocol Version 1 fields
            send_time: datetime_to_millis(&event.time),
            start_time: datetime_to_millis(&event.start),
            stale_time: datetime_to_millis(&event.stale),
            // Optional fields
            access: String::new(),
            qos: String::new(),
            opex: String::new(),
        }
    }
}

/// Convert Event to TakMessage
impl From<&Event> for pb::TakMessage {
    fn from(event: &Event) -> Self {
        pb::TakMessage {
            tak_control: Some(pb::TakControl {
                min_proto_version: 1,
                max_proto_version: 1,
                contact_uid: event.uid.clone(),
            }),
            cot_event: Some(pb::CotEvent::from(event)),
        }
    }
}

/// Convert protobuf format to Event
impl TryFrom<pb::CotEvent> for Event {
    type Error = ProtoError;

    fn try_from(proto: pb::CotEvent) -> Result<Self, Self::Error> {
        let time = proto
            .time
            .parse::<DateTime<Utc>>()
            .map_err(|_| ProtoError::InvalidDateTime(proto.time.clone()))?;

        let start = proto
            .start
            .parse::<DateTime<Utc>>()
            .map_err(|_| ProtoError::InvalidDateTime(proto.start.clone()))?;

        let stale = proto
            .stale
            .parse::<DateTime<Utc>>()
            .map_err(|_| ProtoError::InvalidDateTime(proto.stale.clone()))?;

        // Use inline point fields from TAK Protocol Version 1
        let point = Point {
            lat: proto.lat,
            lon: proto.lon,
            hae: proto.hae,
            ce: if proto.ce == 0.0 { 9999999.0 } else { proto.ce },
            le: if proto.le == 0.0 { 9999999.0 } else { proto.le },
        };

        Ok(Event {
            version: proto.version,
            uid: proto.uid,
            event_type: proto.r#type,
            time,
            start,
            stale,
            how: proto.how,
            point,
            detail: proto.detail.map(|d| Detail {
                xml_detail: if d.xml_detail.is_empty() {
                    None
                } else {
                    Some(d.xml_detail)
                },
                contact: d.contact.map(|c| Contact {
                    endpoint: if c.endpoint.is_empty() {
                        None
                    } else {
                        Some(c.endpoint)
                    },
                    callsign: c.callsign,
                }),
                group: d.group.map(|g| Group {
                    name: g.name,
                    role: g.role,
                }),
                precision_location: d.precision_location.map(|pl| PrecisionLocation {
                    geopointsrc: pl.geopointsrc,
                    altsrc: pl.altsrc,
                }),
                status: d.status.map(|s| Status { battery: s.battery }),
                takv: d.takv.map(|t| Takv {
                    device: t.device,
                    platform: t.platform,
                    os: t.os,
                    version: t.version,
                }),
                track: d.track.map(|t| Track {
                    speed: t.speed,
                    course: t.course,
                }),
                shape: None,         // TODO: Parse shape from protobuf
                link: Vec::new(),    // TODO: Parse links from protobuf
                color: None,         // TODO: Parse color from protobuf
                fill_color: None,    // TODO: Parse fill_color from protobuf
                stroke_color: None,  // TODO: Parse stroke_color from protobuf
                stroke_weight: None, // TODO: Parse stroke_weight from protobuf
                labels_on: None,     // TODO: Parse labels_on from protobuf
            }),
        })
    }
}

/// Encode an Event to protobuf binary format
pub fn encode_event(event: &Event) -> Result<Vec<u8>, ProtoError> {
    let proto_event = pb::CotEvent::from(event);
    let mut buf = Vec::new();
    proto_event.encode(&mut buf)?;
    Ok(buf)
}

/// Decode an Event from protobuf binary format
pub fn decode_event(data: &[u8]) -> Result<Event, ProtoError> {
    let proto_event = pb::CotEvent::decode(data)?;
    Event::try_from(proto_event)
}

/// Encode an Event to TAK Protocol Version 1 Mesh format
/// Mesh format: [0xBF, 0x01, 0xBF] + protobuf
pub fn encode_mesh(event: &Event) -> Result<Vec<u8>, ProtoError> {
    let tak_message = pb::TakMessage::from(event);
    let mut buf = Vec::new();

    // Write mesh header
    buf.extend_from_slice(MESH_HEADER);

    // Write protobuf
    tak_message.encode(&mut buf)?;

    Ok(buf)
}

/// Encode an Event to TAK Protocol Version 1 Stream format
/// Stream format: varint length + protobuf
pub fn encode_stream(event: &Event) -> Result<Vec<u8>, ProtoError> {
    let tak_message = pb::TakMessage::from(event);
    let mut buf = Vec::new();

    // Encode the message to get its size
    let mut proto_buf = Vec::new();
    tak_message.encode(&mut proto_buf)?;

    // Write varint length prefix
    write_varint(&mut buf, proto_buf.len())?;

    // Write protobuf data
    buf.extend_from_slice(&proto_buf);

    Ok(buf)
}

/// Write a varint to a buffer
fn write_varint<W: Write>(writer: &mut W, mut value: usize) -> Result<(), ProtoError> {
    loop {
        let mut byte = (value & 0x7F) as u8;
        value >>= 7;

        if value != 0 {
            byte |= 0x80;
        }

        writer.write_all(&[byte])?;

        if value == 0 {
            break;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn create_test_event() -> Event {
        Event {
            version: "2.0".to_string(),
            uid: "test-123".to_string(),
            event_type: "a-f-G".to_string(),
            time: Utc.with_ymd_and_hms(2024, 1, 15, 10, 30, 0).unwrap(),
            start: Utc.with_ymd_and_hms(2024, 1, 15, 10, 30, 0).unwrap(),
            stale: Utc.with_ymd_and_hms(2024, 1, 15, 10, 35, 0).unwrap(),
            how: "h-e".to_string(),
            point: Point {
                lat: 37.7749,
                lon: -122.4194,
                hae: 100.0,
                ce: 10.0,
                le: 5.0,
            },
            detail: Some(Detail {
                xml_detail: None,
                contact: Some(Contact {
                    endpoint: None,
                    callsign: "Alpha-1".to_string(),
                }),
                group: None,
                precision_location: None,
                status: None,
                takv: None,
                track: None,
                ..Default::default()
            }),
        }
    }

    #[test]
    fn test_encode_decode() {
        let event = create_test_event();
        let encoded = encode_event(&event).expect("Failed to encode");
        let decoded = decode_event(&encoded).expect("Failed to decode");

        assert_eq!(event.uid, decoded.uid);
        assert_eq!(event.version, decoded.version);
        assert_eq!(event.event_type, decoded.event_type);
        assert_eq!(event.point.lat, decoded.point.lat);
        assert_eq!(event.point.lon, decoded.point.lon);
    }

    #[test]
    fn test_proto_conversion() {
        let event = create_test_event();
        let proto = pb::CotEvent::from(&event);

        assert_eq!(proto.uid, "test-123");
        assert_eq!(proto.version, "2.0");
        assert_eq!(proto.r#type, "a-f-G");
        assert!(proto.point.is_some());
    }
}
