//! CoT Event structures and affiliation parsing

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;

/// CoT Event represents a Cursor on Target message
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    /// CoT version (typically "2.0")
    pub version: String,
    /// Unique identifier for this event
    pub uid: String,
    /// CoT type (e.g., "a-f-G" for atom-friendly-ground)
    #[serde(rename = "type")]
    pub event_type: String,
    /// Event timestamp
    pub time: DateTime<Utc>,
    /// Event start time
    pub start: DateTime<Utc>,
    /// Event stale time (when the event becomes invalid)
    pub stale: DateTime<Utc>,
    /// How the event was generated (e.g., "h-e" for human-entered)
    pub how: String,
    /// Geographic location and accuracy
    pub point: Point,
    /// Optional structured detail section
    pub detail: Option<Detail>,
}

/// Geographic point with accuracy metrics
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Point {
    /// Latitude in decimal degrees (-90 to 90)
    pub lat: f64,
    /// Longitude in decimal degrees (-180 to 180)
    pub lon: f64,
    /// Height above ellipsoid in meters
    pub hae: f64,
    /// Circular error in meters (95% confidence)
    pub ce: f64,
    /// Linear error in meters (95% confidence)
    pub le: f64,
}

/// Detail section with structured fields
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Detail {
    /// Unparsed XML content
    pub xml_detail: Option<String>,
    /// Contact information
    pub contact: Option<Contact>,
    /// Group information
    pub group: Option<Group>,
    /// Precision location source
    pub precision_location: Option<PrecisionLocation>,
    /// Status information
    pub status: Option<Status>,
    /// TAK version information
    pub takv: Option<Takv>,
    /// Track information
    pub track: Option<Track>,
    /// Shape/geometry information
    pub shape: Option<Shape>,
    /// Link information (for routes and relationships)
    pub link: Vec<Link>,
    /// Color in ARGB format (e.g., -65536 for red)
    pub color: Option<i32>,
    /// Fill color in ARGB format
    pub fill_color: Option<i32>,
    /// Stroke color in ARGB format
    pub stroke_color: Option<i32>,
    /// Stroke weight in pixels
    pub stroke_weight: Option<f64>,
    /// Whether to show labels
    pub labels_on: Option<bool>,
}

/// Contact information
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Contact {
    /// Optional endpoint for communication
    pub endpoint: Option<String>,
    /// Callsign for display
    pub callsign: String,
}

/// Group information
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Group {
    /// Group name
    pub name: String,
    /// Group role
    pub role: String,
}

/// Track information for moving entities
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Track {
    /// Speed in meters per second
    pub speed: f64,
    /// Course/heading in degrees (0-360)
    pub course: f64,
}

/// Status information
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    /// Battery level (0-100)
    pub battery: u32,
}

/// TAK version and device information
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Takv {
    /// Device identifier
    pub device: String,
    /// Platform (e.g., "ATAK", "WinTAK", "iTAK")
    pub platform: String,
    /// Operating system
    pub os: String,
    /// Version string
    pub version: String,
}

/// Precision location source information
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrecisionLocation {
    /// Geopoint source (e.g., "GPS", "USER")
    pub geopointsrc: String,
    /// Altitude source (e.g., "GPS", "DTED")
    pub altsrc: String,
}

/// Shape/Geometry for TAK objects
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Shape {
    /// Ellipse/Circle shape
    Ellipse {
        /// Major axis in meters
        major: f64,
        /// Minor axis in meters
        minor: f64,
        /// Rotation angle in degrees
        angle: f64,
    },
    /// Polyline (can be open or closed for polygons)
    Polyline {
        /// Vertices of the polyline
        vertices: Vec<Point>,
        /// Whether the polyline is closed (polygon)
        closed: bool,
    },
}

/// Link between CoT events (used for routes, hierarchies, etc.)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Link {
    /// UID of the linked event
    pub uid: String,
    /// Type of the linked event
    #[serde(rename = "type")]
    pub link_type: Option<String>,
    /// Relationship type (e.g., "p-p" for point-to-point, "c" for contains)
    pub relation: String,
}

/// MIL-STD-2525 affiliation parsed from CoT type field
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Affiliation {
    /// Pending (p)
    Pending,
    /// Unknown (u)
    Unknown,
    /// Assumed Friend (a)
    AssumedFriend,
    /// Friend (f)
    Friend,
    /// Neutral (n)
    Neutral,
    /// Suspect (s)
    Suspect,
    /// Hostile (h)
    Hostile,
    /// Joker (j)
    Joker,
    /// Faker (k)
    Faker,
    /// None specified (o)
    None,
}

impl Affiliation {
    /// Parse affiliation from CoT type field
    /// CoT type format: "a-f-G" where second character is affiliation
    pub fn from_cot_type(cot_type: &str) -> Option<Self> {
        let parts: Vec<&str> = cot_type.split('-').collect();
        if parts.len() < 2 {
            return None;
        }

        match parts[1].chars().next() {
            Some('p') => Some(Affiliation::Pending),
            Some('u') => Some(Affiliation::Unknown),
            Some('a') => Some(Affiliation::AssumedFriend),
            Some('f') => Some(Affiliation::Friend),
            Some('n') => Some(Affiliation::Neutral),
            Some('s') => Some(Affiliation::Suspect),
            Some('h') => Some(Affiliation::Hostile),
            Some('j') => Some(Affiliation::Joker),
            Some('k') => Some(Affiliation::Faker),
            Some('o') => Some(Affiliation::None),
            _ => None,
        }
    }
}

impl fmt::Display for Affiliation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Affiliation::Pending => write!(f, "Pending"),
            Affiliation::Unknown => write!(f, "Unknown"),
            Affiliation::AssumedFriend => write!(f, "Assumed Friend"),
            Affiliation::Friend => write!(f, "Friend"),
            Affiliation::Neutral => write!(f, "Neutral"),
            Affiliation::Suspect => write!(f, "Suspect"),
            Affiliation::Hostile => write!(f, "Hostile"),
            Affiliation::Joker => write!(f, "Joker"),
            Affiliation::Faker => write!(f, "Faker"),
            Affiliation::None => write!(f, "None"),
        }
    }
}

impl Detail {
    /// Create a new empty Detail
    pub fn new() -> Self {
        Self::default()
    }

    /// Check if the detail is empty
    pub fn is_empty(&self) -> bool {
        self.xml_detail.is_none()
            && self.contact.is_none()
            && self.group.is_none()
            && self.precision_location.is_none()
            && self.status.is_none()
            && self.takv.is_none()
            && self.track.is_none()
            && self.shape.is_none()
            && self.link.is_empty()
            && self.color.is_none()
            && self.fill_color.is_none()
            && self.stroke_color.is_none()
            && self.stroke_weight.is_none()
            && self.labels_on.is_none()
    }
}

impl Event {
    /// Get the affiliation from the event type
    pub fn affiliation(&self) -> Option<Affiliation> {
        Affiliation::from_cot_type(&self.event_type)
    }

    /// Get the callsign from the contact detail, if present
    pub fn callsign(&self) -> Option<&str> {
        self.detail
            .as_ref()
            .and_then(|d| d.contact.as_ref())
            .map(|c| c.callsign.as_str())
    }

    /// Get the group name from the group detail, if present
    pub fn group_name(&self) -> Option<&str> {
        self.detail
            .as_ref()
            .and_then(|d| d.group.as_ref())
            .map(|g| g.name.as_str())
    }

    /// Get the speed from the track detail, if present
    pub fn speed(&self) -> Option<f64> {
        self.detail
            .as_ref()
            .and_then(|d| d.track.as_ref())
            .map(|t| t.speed)
    }

    /// Get the course from the track detail, if present
    pub fn course(&self) -> Option<f64> {
        self.detail
            .as_ref()
            .and_then(|d| d.track.as_ref())
            .map(|t| t.course)
    }

    /// Convert event time to milliseconds since epoch (TAK Protocol Version 1 format)
    pub fn time_millis(&self) -> u64 {
        datetime_to_millis(&self.time)
    }

    /// Convert start time to milliseconds since epoch (TAK Protocol Version 1 format)
    pub fn start_millis(&self) -> u64 {
        datetime_to_millis(&self.start)
    }

    /// Convert stale time to milliseconds since epoch (TAK Protocol Version 1 format)
    pub fn stale_millis(&self) -> u64 {
        datetime_to_millis(&self.stale)
    }

    /// Create an Event with timestamps from milliseconds since epoch
    pub fn with_millis_timestamps(
        mut self,
        send_time: u64,
        start_time: u64,
        stale_time: u64,
    ) -> Self {
        self.time = millis_to_datetime(send_time);
        self.start = millis_to_datetime(start_time);
        self.stale = millis_to_datetime(stale_time);
        self
    }
}

/// Convert DateTime to milliseconds since epoch
fn datetime_to_millis(dt: &DateTime<Utc>) -> u64 {
    (dt.timestamp() * 1000 + dt.timestamp_subsec_millis() as i64) as u64
}

/// Convert milliseconds since epoch to DateTime<Utc>
fn millis_to_datetime(millis: u64) -> DateTime<Utc> {
    let secs = (millis / 1000) as i64;
    let nanos = ((millis % 1000) * 1_000_000) as u32;
    DateTime::from_timestamp(secs, nanos).unwrap_or(DateTime::UNIX_EPOCH)
}

impl Point {
    /// Create a new Point with default accuracy values
    pub fn new(lat: f64, lon: f64, hae: f64) -> Self {
        Self {
            lat,
            lon,
            hae,
            ce: 9999999.0,
            le: 9999999.0,
        }
    }

    /// Create a new Point with specified accuracy
    pub fn with_accuracy(lat: f64, lon: f64, hae: f64, ce: f64, le: f64) -> Self {
        Self {
            lat,
            lon,
            hae,
            ce,
            le,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_affiliation_parsing() {
        assert_eq!(
            Affiliation::from_cot_type("a-f-G"),
            Some(Affiliation::Friend)
        );
        assert_eq!(
            Affiliation::from_cot_type("a-h-G"),
            Some(Affiliation::Hostile)
        );
        assert_eq!(
            Affiliation::from_cot_type("a-n-G"),
            Some(Affiliation::Neutral)
        );
        assert_eq!(
            Affiliation::from_cot_type("a-u-G"),
            Some(Affiliation::Unknown)
        );
        assert_eq!(Affiliation::from_cot_type("invalid"), None);
    }

    #[test]
    fn test_point_creation() {
        let point = Point::new(37.7749, -122.4194, 100.0);
        assert_eq!(point.lat, 37.7749);
        assert_eq!(point.lon, -122.4194);
        assert_eq!(point.hae, 100.0);
        assert_eq!(point.ce, 9999999.0);
        assert_eq!(point.le, 9999999.0);
    }
}
