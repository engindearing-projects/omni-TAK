//! High-performance parser for CoT messages supporting XML, Mesh, and Stream protocols

use crate::event::{Contact, Detail, Event, Group, Point, PrecisionLocation, Status, Takv, Track};
use crate::proto::pb;
use chrono::{DateTime, Utc};
use prost::Message;
use quick_xml::events::Event as XmlEvent;
use quick_xml::Reader;
use std::io::{Cursor, Read};
use thiserror::Error;

/// TAK Protocol headers
const MESH_HEADER: &[u8] = &[0xBF, 0x01, 0xBF];
const STREAM_HEADER_START: u8 = 0x0A;
const STREAM_HEADER_END: u8 = 0x0D;
const XML_HEADER: &[u8] = b"<?xml";

/// Protocol detection result
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    /// TAK Protocol Version 1 - Mesh mode (0xBF 0x01 0xBF header)
    Mesh,
    /// TAK Protocol Version 1 - Stream mode (length-delimited protobuf)
    Stream,
    /// XML CoT message
    Xml,
}

#[derive(Error, Debug)]
pub enum ParseError {
    #[error("XML parsing error: {0}")]
    XmlError(#[from] quick_xml::Error),

    #[error("Missing required field: {0}")]
    MissingField(String),

    #[error("Invalid datetime format: {0}")]
    InvalidDateTime(String),

    #[error("Invalid number format: {0}")]
    InvalidNumber(String),

    #[error("Invalid event structure: {0}")]
    InvalidStructure(String),

    #[error("Protobuf decoding error: {0}")]
    ProtobufError(#[from] prost::DecodeError),

    #[error("Unknown protocol: unable to detect message format")]
    UnknownProtocol,

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Invalid varint encoding")]
    InvalidVarint,
}

/// Detect the protocol of a CoT message
pub fn detect_protocol(data: &[u8]) -> Result<Protocol, ParseError> {
    if data.is_empty() {
        return Err(ParseError::UnknownProtocol);
    }

    // Check for Mesh header (0xBF 0x01 0xBF) - most specific
    if data.len() >= 3 && data[0..3] == *MESH_HEADER {
        return Ok(Protocol::Mesh);
    }

    // Check for XML header
    if data.len() >= 5 && data[0..5] == *XML_HEADER {
        return Ok(Protocol::Xml);
    }

    // Check for <event tag (XML without declaration)
    if data.len() >= 6 && &data[0..6] == b"<event" {
        return Ok(Protocol::Xml);
    }

    // If it's not mesh or XML, assume it's stream format (length-delimited protobuf)
    // Stream format starts with a varint length prefix which can be any byte value
    // We'll optimistically try to parse it as stream
    Ok(Protocol::Stream)
}

/// Parse a CoT message automatically detecting the protocol
pub fn parse_any(data: &[u8]) -> Result<Event, ParseError> {
    let protocol = detect_protocol(data)?;
    match protocol {
        Protocol::Mesh => parse_mesh(data),
        Protocol::Stream => parse_stream(data),
        Protocol::Xml => parse_cot_bytes(data),
    }
}

/// Parse a CoT message from XML string
pub fn parse_cot(xml: &str) -> Result<Event, ParseError> {
    parse_cot_bytes(xml.as_bytes())
}

/// Parse TAK Protocol Version 1 Mesh mode message
pub fn parse_mesh(data: &[u8]) -> Result<Event, ParseError> {
    if data.len() < 3 || data[0..3] != *MESH_HEADER {
        return Err(ParseError::InvalidStructure(
            "Invalid mesh header".to_string(),
        ));
    }

    // Skip the 3-byte header and decode the protobuf
    let protobuf_data = &data[3..];
    let tak_message = pb::TakMessage::decode(protobuf_data)?;

    // Convert TakMessage to Event
    tak_message_to_event(tak_message)
}

/// Parse TAK Protocol Version 1 Stream mode message (length-delimited)
pub fn parse_stream(data: &[u8]) -> Result<Event, ParseError> {
    let mut cursor = Cursor::new(data);

    // Read the varint length prefix
    let msg_len = read_varint(&mut cursor)?;

    // Read the protobuf message
    let mut protobuf_data = vec![0u8; msg_len];
    cursor.read_exact(&mut protobuf_data)?;

    let tak_message = pb::TakMessage::decode(protobuf_data.as_slice())?;

    // Convert TakMessage to Event
    tak_message_to_event(tak_message)
}

/// Read a varint from a cursor (length-delimited protobuf)
fn read_varint<R: Read>(reader: &mut R) -> Result<usize, ParseError> {
    let mut result: u64 = 0;
    let mut shift = 0;

    for _ in 0..10 {
        // Max 10 bytes for 64-bit varint
        let mut buf = [0u8; 1];
        reader.read_exact(&mut buf)?;
        let byte = buf[0];

        result |= ((byte & 0x7F) as u64) << shift;

        if byte & 0x80 == 0 {
            return Ok(result as usize);
        }

        shift += 7;
    }

    Err(ParseError::InvalidVarint)
}

/// Convert a TakMessage protobuf to an Event struct
fn tak_message_to_event(tak_message: pb::TakMessage) -> Result<Event, ParseError> {
    let cot_event = tak_message
        .cot_event
        .ok_or_else(|| ParseError::MissingField("cotEvent".to_string()))?;

    // Convert timestamps: prioritize millisecond timestamps, fall back to string timestamps
    let time = if cot_event.send_time > 0 {
        millis_to_datetime(cot_event.send_time)
    } else if !cot_event.time.is_empty() {
        parse_datetime(&cot_event.time)?
    } else {
        return Err(ParseError::MissingField("time/sendTime".to_string()));
    };

    let start = if cot_event.start_time > 0 {
        millis_to_datetime(cot_event.start_time)
    } else if !cot_event.start.is_empty() {
        parse_datetime(&cot_event.start)?
    } else {
        return Err(ParseError::MissingField("start/startTime".to_string()));
    };

    let stale = if cot_event.stale_time > 0 {
        millis_to_datetime(cot_event.stale_time)
    } else if !cot_event.stale.is_empty() {
        parse_datetime(&cot_event.stale)?
    } else {
        return Err(ParseError::MissingField("stale/staleTime".to_string()));
    };

    // Extract point data from CotEvent (inline fields in TAK Protocol Version 1)
    let point = Point {
        lat: cot_event.lat,
        lon: cot_event.lon,
        hae: cot_event.hae,
        ce: if cot_event.ce == 0.0 {
            9999999.0
        } else {
            cot_event.ce
        },
        le: if cot_event.le == 0.0 {
            9999999.0
        } else {
            cot_event.le
        },
    };

    Ok(Event {
        version: if cot_event.version.is_empty() {
            "2.0".to_string()
        } else {
            cot_event.version
        },
        uid: cot_event.uid,
        event_type: cot_event.r#type,
        time,
        start,
        stale,
        how: cot_event.how,
        point,
        detail: cot_event.detail.map(|pb_detail| Detail {
            xml_detail: if pb_detail.xml_detail.is_empty() {
                None
            } else {
                Some(pb_detail.xml_detail)
            },
            contact: pb_detail.contact.map(|c| Contact {
                endpoint: if c.endpoint.is_empty() {
                    None
                } else {
                    Some(c.endpoint)
                },
                callsign: c.callsign,
            }),
            group: pb_detail.group.map(|g| Group {
                name: g.name,
                role: g.role,
            }),
            precision_location: pb_detail.precision_location.map(|pl| PrecisionLocation {
                geopointsrc: pl.geopointsrc,
                altsrc: pl.altsrc,
            }),
            status: pb_detail.status.map(|s| Status { battery: s.battery }),
            takv: pb_detail.takv.map(|t| Takv {
                device: t.device,
                platform: t.platform,
                os: t.os,
                version: t.version,
            }),
            track: pb_detail.track.map(|t| Track {
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

/// Convert milliseconds since epoch to DateTime<Utc>
fn millis_to_datetime(millis: u64) -> DateTime<Utc> {
    let secs = (millis / 1000) as i64;
    let nanos = ((millis % 1000) * 1_000_000) as u32;
    DateTime::from_timestamp(secs, nanos).unwrap_or(DateTime::UNIX_EPOCH)
}

/// Parse a CoT message from XML bytes (zero-copy where possible)
pub fn parse_cot_bytes(xml: &[u8]) -> Result<Event, ParseError> {
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(true);

    let mut buf = Vec::new();

    // Event fields
    let mut version = None;
    let mut uid = None;
    let mut event_type = None;
    let mut time = None;
    let mut start = None;
    let mut stale = None;
    let mut how = None;
    let mut point = None;
    let mut detail = None;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(XmlEvent::Start(e)) | Ok(XmlEvent::Empty(e)) => {
                match e.name().as_ref() {
                    b"event" => {
                        // Parse event attributes
                        for attr in e.attributes() {
                            let attr = attr.map_err(|e| {
                                ParseError::XmlError(quick_xml::Error::InvalidAttr(e))
                            })?;
                            match attr.key.as_ref() {
                                b"version" => {
                                    version = Some(
                                        String::from_utf8_lossy(attr.value.as_ref()).into_owned(),
                                    );
                                }
                                b"uid" => {
                                    uid = Some(
                                        String::from_utf8_lossy(attr.value.as_ref()).into_owned(),
                                    );
                                }
                                b"type" => {
                                    event_type = Some(
                                        String::from_utf8_lossy(attr.value.as_ref()).into_owned(),
                                    );
                                }
                                b"time" => {
                                    let time_str = String::from_utf8_lossy(attr.value.as_ref());
                                    time = Some(parse_datetime(&time_str)?);
                                }
                                b"start" => {
                                    let start_str = String::from_utf8_lossy(attr.value.as_ref());
                                    start = Some(parse_datetime(&start_str)?);
                                }
                                b"stale" => {
                                    let stale_str = String::from_utf8_lossy(attr.value.as_ref());
                                    stale = Some(parse_datetime(&stale_str)?);
                                }
                                b"how" => {
                                    how = Some(
                                        String::from_utf8_lossy(attr.value.as_ref()).into_owned(),
                                    );
                                }
                                _ => {}
                            }
                        }
                    }
                    b"point" => {
                        // Parse point attributes
                        let mut lat = None;
                        let mut lon = None;
                        let mut hae = None;
                        let mut ce = None;
                        let mut le = None;

                        for attr in e.attributes() {
                            let attr = attr.map_err(|e| {
                                ParseError::XmlError(quick_xml::Error::InvalidAttr(e))
                            })?;
                            match attr.key.as_ref() {
                                b"lat" => {
                                    let lat_str = String::from_utf8_lossy(attr.value.as_ref());
                                    lat = Some(parse_f64(&lat_str)?);
                                }
                                b"lon" => {
                                    let lon_str = String::from_utf8_lossy(attr.value.as_ref());
                                    lon = Some(parse_f64(&lon_str)?);
                                }
                                b"hae" => {
                                    let hae_str = String::from_utf8_lossy(attr.value.as_ref());
                                    hae = Some(parse_f64(&hae_str)?);
                                }
                                b"ce" => {
                                    let ce_str = String::from_utf8_lossy(attr.value.as_ref());
                                    ce = Some(parse_f64(&ce_str)?);
                                }
                                b"le" => {
                                    let le_str = String::from_utf8_lossy(attr.value.as_ref());
                                    le = Some(parse_f64(&le_str)?);
                                }
                                _ => {}
                            }
                        }

                        point = Some(Point {
                            lat: lat.ok_or_else(|| ParseError::MissingField("lat".into()))?,
                            lon: lon.ok_or_else(|| ParseError::MissingField("lon".into()))?,
                            hae: hae.unwrap_or(0.0),
                            ce: ce.unwrap_or(9999999.0),
                            le: le.unwrap_or(9999999.0),
                        });
                    }
                    b"detail" => {
                        // Parse detail section (simplified as HashMap for now)
                        detail = Some(parse_detail(&mut reader, &mut buf)?);
                    }
                    _ => {}
                }
            }
            Ok(XmlEvent::Eof) => break,
            Err(e) => return Err(ParseError::XmlError(e)),
            _ => {}
        }
        buf.clear();
    }

    // Build Event struct
    Ok(Event {
        version: version.ok_or_else(|| ParseError::MissingField("version".into()))?,
        uid: uid.ok_or_else(|| ParseError::MissingField("uid".into()))?,
        event_type: event_type.ok_or_else(|| ParseError::MissingField("type".into()))?,
        time: time.ok_or_else(|| ParseError::MissingField("time".into()))?,
        start: start.ok_or_else(|| ParseError::MissingField("start".into()))?,
        stale: stale.ok_or_else(|| ParseError::MissingField("stale".into()))?,
        how: how.ok_or_else(|| ParseError::MissingField("how".into()))?,
        point: point.ok_or_else(|| ParseError::MissingField("point".into()))?,
        detail,
    })
}

fn parse_datetime(s: &str) -> Result<DateTime<Utc>, ParseError> {
    s.parse::<DateTime<Utc>>()
        .map_err(|_| ParseError::InvalidDateTime(s.to_string()))
}

fn parse_f64(s: &str) -> Result<f64, ParseError> {
    s.parse::<f64>()
        .map_err(|_| ParseError::InvalidNumber(s.to_string()))
}

fn parse_detail(reader: &mut Reader<&[u8]>, buf: &mut Vec<u8>) -> Result<Detail, ParseError> {
    let mut detail = Detail::default();
    let mut xml_fragments = Vec::new();
    let mut depth = 1;

    loop {
        match reader.read_event_into(buf) {
            Ok(XmlEvent::Start(e)) => {
                depth += 1;
                let tag_name = String::from_utf8_lossy(e.name().as_ref()).into_owned();

                // For Start events with unknown tags, save to xml_detail
                if !matches!(
                    tag_name.as_str(),
                    "contact" | "__group" | "track" | "status" | "takv" | "precisionlocation"
                ) {
                    xml_fragments.push(tag_name);
                }
            }
            Ok(XmlEvent::Empty(e)) => {
                let tag_name = String::from_utf8_lossy(e.name().as_ref()).into_owned();

                match tag_name.as_str() {
                    "contact" => {
                        detail.contact = Some(parse_contact(&e)?);
                    }
                    "__group" => {
                        detail.group = Some(parse_group(&e)?);
                    }
                    "track" => {
                        detail.track = Some(parse_track(&e)?);
                    }
                    "status" => {
                        detail.status = Some(parse_status(&e)?);
                    }
                    "takv" => {
                        detail.takv = Some(parse_takv(&e)?);
                    }
                    "precisionlocation" => {
                        detail.precision_location = Some(parse_precision_location(&e)?);
                    }
                    _ => {
                        // Unknown element - save to xml_detail
                        let element_str = std::str::from_utf8(buf).unwrap_or("").to_string();
                        xml_fragments.push(element_str);
                    }
                }
            }
            Ok(XmlEvent::End(_)) => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            Ok(XmlEvent::Eof) => break,
            Err(e) => return Err(ParseError::XmlError(e)),
            _ => {}
        }
        buf.clear();
    }

    // Combine xml fragments if any
    if !xml_fragments.is_empty() {
        detail.xml_detail = Some(xml_fragments.join("\n"));
    }

    Ok(detail)
}

fn parse_contact(element: &quick_xml::events::BytesStart) -> Result<Contact, ParseError> {
    let mut endpoint = None;
    let mut callsign = None;

    for attr in element.attributes() {
        let attr = attr.map_err(|e| ParseError::XmlError(quick_xml::Error::InvalidAttr(e)))?;
        match attr.key.as_ref() {
            b"endpoint" => {
                endpoint = Some(String::from_utf8_lossy(attr.value.as_ref()).into_owned());
            }
            b"callsign" => {
                callsign = Some(String::from_utf8_lossy(attr.value.as_ref()).into_owned());
            }
            _ => {}
        }
    }

    Ok(Contact {
        endpoint,
        callsign: callsign.ok_or_else(|| ParseError::MissingField("callsign".into()))?,
    })
}

fn parse_group(element: &quick_xml::events::BytesStart) -> Result<Group, ParseError> {
    let mut name = None;
    let mut role = None;

    for attr in element.attributes() {
        let attr = attr.map_err(|e| ParseError::XmlError(quick_xml::Error::InvalidAttr(e)))?;
        match attr.key.as_ref() {
            b"name" => {
                name = Some(String::from_utf8_lossy(attr.value.as_ref()).into_owned());
            }
            b"role" => {
                role = Some(String::from_utf8_lossy(attr.value.as_ref()).into_owned());
            }
            _ => {}
        }
    }

    Ok(Group {
        name: name.ok_or_else(|| ParseError::MissingField("group name".into()))?,
        role: role.ok_or_else(|| ParseError::MissingField("group role".into()))?,
    })
}

fn parse_track(element: &quick_xml::events::BytesStart) -> Result<Track, ParseError> {
    let mut speed = None;
    let mut course = None;

    for attr in element.attributes() {
        let attr = attr.map_err(|e| ParseError::XmlError(quick_xml::Error::InvalidAttr(e)))?;
        match attr.key.as_ref() {
            b"speed" => {
                let speed_str = String::from_utf8_lossy(attr.value.as_ref());
                speed = Some(parse_f64(&speed_str)?);
            }
            b"course" => {
                let course_str = String::from_utf8_lossy(attr.value.as_ref());
                course = Some(parse_f64(&course_str)?);
            }
            _ => {}
        }
    }

    Ok(Track {
        speed: speed.ok_or_else(|| ParseError::MissingField("track speed".into()))?,
        course: course.ok_or_else(|| ParseError::MissingField("track course".into()))?,
    })
}

fn parse_status(element: &quick_xml::events::BytesStart) -> Result<Status, ParseError> {
    let mut battery = None;

    for attr in element.attributes() {
        let attr = attr.map_err(|e| ParseError::XmlError(quick_xml::Error::InvalidAttr(e)))?;
        if attr.key.as_ref() == b"battery" {
            let battery_str = String::from_utf8_lossy(attr.value.as_ref());
            battery = Some(
                battery_str
                    .parse::<u32>()
                    .map_err(|_| ParseError::InvalidNumber(battery_str.to_string()))?,
            );
        }
    }

    Ok(Status {
        battery: battery.ok_or_else(|| ParseError::MissingField("battery".into()))?,
    })
}

fn parse_takv(element: &quick_xml::events::BytesStart) -> Result<Takv, ParseError> {
    let mut device = None;
    let mut platform = None;
    let mut os = None;
    let mut version = None;

    for attr in element.attributes() {
        let attr = attr.map_err(|e| ParseError::XmlError(quick_xml::Error::InvalidAttr(e)))?;
        match attr.key.as_ref() {
            b"device" => {
                device = Some(String::from_utf8_lossy(attr.value.as_ref()).into_owned());
            }
            b"platform" => {
                platform = Some(String::from_utf8_lossy(attr.value.as_ref()).into_owned());
            }
            b"os" => {
                os = Some(String::from_utf8_lossy(attr.value.as_ref()).into_owned());
            }
            b"version" => {
                version = Some(String::from_utf8_lossy(attr.value.as_ref()).into_owned());
            }
            _ => {}
        }
    }

    Ok(Takv {
        device: device.ok_or_else(|| ParseError::MissingField("takv device".into()))?,
        platform: platform.ok_or_else(|| ParseError::MissingField("takv platform".into()))?,
        os: os.ok_or_else(|| ParseError::MissingField("takv os".into()))?,
        version: version.ok_or_else(|| ParseError::MissingField("takv version".into()))?,
    })
}

fn parse_precision_location(
    element: &quick_xml::events::BytesStart,
) -> Result<PrecisionLocation, ParseError> {
    let mut geopointsrc = None;
    let mut altsrc = None;

    for attr in element.attributes() {
        let attr = attr.map_err(|e| ParseError::XmlError(quick_xml::Error::InvalidAttr(e)))?;
        match attr.key.as_ref() {
            b"geopointsrc" => {
                geopointsrc = Some(String::from_utf8_lossy(attr.value.as_ref()).into_owned());
            }
            b"altsrc" => {
                altsrc = Some(String::from_utf8_lossy(attr.value.as_ref()).into_owned());
            }
            _ => {}
        }
    }

    Ok(PrecisionLocation {
        geopointsrc: geopointsrc.ok_or_else(|| ParseError::MissingField("geopointsrc".into()))?,
        altsrc: altsrc.ok_or_else(|| ParseError::MissingField("altsrc".into()))?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE_COT: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<event version="2.0" uid="ANDROID-12345678" type="a-f-G" time="2024-01-15T10:30:00Z" start="2024-01-15T10:30:00Z" stale="2024-01-15T10:35:00Z" how="h-e">
    <point lat="37.7749" lon="-122.4194" hae="100.0" ce="10.0" le="5.0"/>
    <detail>
        <contact callsign="Alpha-1"/>
        <remarks>Test CoT message</remarks>
    </detail>
</event>"#;

    #[test]
    fn test_parse_cot() {
        let event = parse_cot(EXAMPLE_COT).expect("Failed to parse CoT");

        assert_eq!(event.version, "2.0");
        assert_eq!(event.uid, "ANDROID-12345678");
        assert_eq!(event.event_type, "a-f-G");
        assert_eq!(event.how, "h-e");

        assert_eq!(event.point.lat, 37.7749);
        assert_eq!(event.point.lon, -122.4194);
        assert_eq!(event.point.hae, 100.0);
        assert_eq!(event.point.ce, 10.0);
        assert_eq!(event.point.le, 5.0);

        assert!(event.detail.is_some());
    }

    #[test]
    fn test_parse_minimal_cot() {
        let minimal_cot = r#"<event version="2.0" uid="test-1" type="a-f-G" time="2024-01-15T10:30:00Z" start="2024-01-15T10:30:00Z" stale="2024-01-15T10:35:00Z" how="m-g">
    <point lat="0.0" lon="0.0" hae="0.0"/>
</event>"#;

        let event = parse_cot(minimal_cot).expect("Failed to parse minimal CoT");
        assert_eq!(event.version, "2.0");
        assert_eq!(event.uid, "test-1");
    }

    #[test]
    fn test_parse_malformed_cot() {
        let malformed = r#"<event version="2.0">invalid</event>"#;
        assert!(parse_cot(malformed).is_err());
    }

    #[test]
    fn test_parse_cot_bytes() {
        let event = parse_cot_bytes(EXAMPLE_COT.as_bytes()).expect("Failed to parse CoT bytes");
        assert_eq!(event.uid, "ANDROID-12345678");
    }

    #[test]
    fn test_protocol_detection() {
        // Test XML detection
        let xml_data = b"<?xml version=\"1.0\"?><event/>";
        assert_eq!(detect_protocol(xml_data).unwrap(), Protocol::Xml);

        // Test XML without declaration
        let xml_no_decl = b"<event version=\"2.0\"/>";
        assert_eq!(detect_protocol(xml_no_decl).unwrap(), Protocol::Xml);

        // Test Mesh detection
        let mesh_data = [0xBF, 0x01, 0xBF, 0x00];
        assert_eq!(detect_protocol(&mesh_data).unwrap(), Protocol::Mesh);

        // Test Stream detection (any non-XML, non-Mesh data is assumed to be Stream)
        let stream_data = [0x0A, 0x00];
        assert_eq!(detect_protocol(&stream_data).unwrap(), Protocol::Stream);

        // Any other binary data defaults to Stream
        let other_binary = [0xFF, 0xFF];
        assert_eq!(detect_protocol(&other_binary).unwrap(), Protocol::Stream);

        // Empty data returns error
        let empty: &[u8] = &[];
        assert!(detect_protocol(empty).is_err());
    }

    #[test]
    fn test_mesh_roundtrip() {
        use crate::proto::encode_mesh;

        // Create a test event
        let event = parse_cot(EXAMPLE_COT).expect("Failed to parse CoT");

        // Encode to mesh format
        let mesh_data = encode_mesh(&event).expect("Failed to encode mesh");

        // Verify header
        assert_eq!(&mesh_data[0..3], MESH_HEADER);

        // Parse back
        let decoded_event = parse_mesh(&mesh_data).expect("Failed to parse mesh");

        assert_eq!(decoded_event.uid, event.uid);
        assert_eq!(decoded_event.event_type, event.event_type);
        assert_eq!(decoded_event.point.lat, event.point.lat);
        assert_eq!(decoded_event.point.lon, event.point.lon);
    }

    #[test]
    fn test_stream_roundtrip() {
        use crate::proto::encode_stream;

        // Create a test event
        let event = parse_cot(EXAMPLE_COT).expect("Failed to parse CoT");

        // Encode to stream format
        let stream_data = encode_stream(&event).expect("Failed to encode stream");

        // Parse back
        let decoded_event = parse_stream(&stream_data).expect("Failed to parse stream");

        assert_eq!(decoded_event.uid, event.uid);
        assert_eq!(decoded_event.event_type, event.event_type);
        assert_eq!(decoded_event.point.lat, event.point.lat);
        assert_eq!(decoded_event.point.lon, event.point.lon);
    }

    #[test]
    fn test_parse_any() {
        // Test with XML
        let xml_event =
            parse_any(EXAMPLE_COT.as_bytes()).expect("Failed to parse XML via parse_any");
        assert_eq!(xml_event.uid, "ANDROID-12345678");

        // Test with Mesh
        use crate::proto::encode_mesh;
        let event = parse_cot(EXAMPLE_COT).expect("Failed to parse CoT");
        let mesh_data = encode_mesh(&event).expect("Failed to encode mesh");
        let mesh_event = parse_any(&mesh_data).expect("Failed to parse mesh via parse_any");
        assert_eq!(mesh_event.uid, event.uid);

        // Test with Stream
        use crate::proto::encode_stream;
        let stream_data = encode_stream(&event).expect("Failed to encode stream");
        let stream_event = parse_any(&stream_data).expect("Failed to parse stream via parse_any");
        assert_eq!(stream_event.uid, event.uid);
    }

    #[test]
    fn test_timestamp_conversion() {
        let event = parse_cot(EXAMPLE_COT).expect("Failed to parse CoT");

        // Test timestamp conversion
        let time_millis = event.time_millis();
        assert!(time_millis > 0);

        let start_millis = event.start_millis();
        assert!(start_millis > 0);

        let stale_millis = event.stale_millis();
        assert!(stale_millis > 0);

        // Verify stale is after start
        assert!(stale_millis > start_millis);
    }
}
