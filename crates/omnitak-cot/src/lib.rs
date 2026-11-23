//! High-performance CoT (Cursor on Target) message parser for military-grade TAK aggregators
//!
//! This crate provides efficient parsing and serialization of Cursor on Target (CoT) messages,
//! which are XML-based messages used in military tactical awareness systems.
//!
//! # Features
//!
//! - Zero-copy XML parsing using quick-xml
//! - Protobuf support for binary serialization
//! - MIL-STD-2525 affiliation parsing
//! - Comprehensive validation
//! - High performance (<1μs per message for typical payloads)
//!
//! # Example
//!
//! ```rust
//! use omnitak_cot::parser::parse_cot;
//!
//! let cot_xml = r#"<?xml version="1.0"?>
//! <event version="2.0" uid="test-1" type="a-f-G"
//!        time="2024-01-15T10:30:00Z"
//!        start="2024-01-15T10:30:00Z"
//!        stale="2024-01-15T10:35:00Z" how="h-e">
//!     <point lat="37.7749" lon="-122.4194" hae="100.0" ce="10.0" le="5.0"/>
//! </event>"#;
//!
//! let event = parse_cot(cot_xml).expect("Failed to parse CoT");
//! assert_eq!(event.uid, "test-1");
//! assert_eq!(event.point.lat, 37.7749);
//! ```

pub mod event;
pub mod parser;
pub mod proto;
#[cfg(feature = "remote-id")]
pub mod rid;
pub mod serializer;
pub mod validate;

pub use event::{
    Affiliation, Contact, Detail, Event, Group, Point, PrecisionLocation, Status, Takv, Track,
};
pub use parser::{parse_cot, parse_cot_bytes, ParseError};
pub use proto::{decode_event, encode_event, ProtoError};
pub use serializer::serialize_event;
pub use validate::{validate_event, validate_event_strict, validate_point, ValidationError};
