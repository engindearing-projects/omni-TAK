//! Map panel for visualizing TAK positions with altitude.

use crate::ui::offline_maps::{render_overlays, OfflineMapManager};
use crate::{AppState, MessageLog};
use eframe::egui;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use walkers::sources::{Mapbox, MapboxStyle, OpenStreetMap};
use walkers::{HttpTiles, Map, MapMemory, Plugin, Projector, Tiles};

/// Available map tile providers
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum TileProvider {
    OpenStreetMap,
    MapboxStreets,
    MapboxOutdoors,
    MapboxSatellite,
    MapboxSatelliteStreets,
    MapboxLight,
    MapboxDark,
}

impl TileProvider {
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::OpenStreetMap => "OpenStreetMap",
            Self::MapboxStreets => "Mapbox Streets",
            Self::MapboxOutdoors => "Mapbox Outdoors",
            Self::MapboxSatellite => "Mapbox Satellite",
            Self::MapboxSatelliteStreets => "Mapbox Satellite+Streets",
            Self::MapboxLight => "Mapbox Light",
            Self::MapboxDark => "Mapbox Dark",
        }
    }

    pub fn requires_api_key(&self) -> bool {
        !matches!(self, Self::OpenStreetMap)
    }
}

/// Drawing tool modes
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum DrawingTool {
    Select,
    Marker,
    Line,
    Circle,
    Polygon,
    Measure,
    RangeRing,
}

impl DrawingTool {
    pub fn icon(&self) -> &'static str {
        match self {
            Self::Select => "🖱️",
            Self::Marker => "📍",
            Self::Line => "📏",
            Self::Circle => "⭕",
            Self::Polygon => "🔷",
            Self::Measure => "📐",
            Self::RangeRing => "🎯",
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Select => "Select",
            Self::Marker => "Marker",
            Self::Line => "Line",
            Self::Circle => "Circle",
            Self::Polygon => "Polygon",
            Self::Measure => "Measure",
            Self::RangeRing => "Range Ring",
        }
    }
}

/// A drawn shape on the map
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum DrawnShape {
    Marker {
        lat: f64,
        lon: f64,
        label: String,
        color: [u8; 3],
    },
    Line {
        points: Vec<(f64, f64)>,
        color: [u8; 3],
        width: f32,
    },
    Circle {
        center_lat: f64,
        center_lon: f64,
        radius_m: f64,
        color: [u8; 3],
        filled: bool,
    },
    Polygon {
        points: Vec<(f64, f64)>,
        color: [u8; 3],
        filled: bool,
    },
    RangeRing {
        center_lat: f64,
        center_lon: f64,
        rings: Vec<f64>, // radii in meters
        color: [u8; 3],
    },
}

/// Track history entry for Blue Force Tracking
#[derive(Clone, Debug)]
pub struct TrackPoint {
    pub lat: f64,
    pub lon: f64,
    pub altitude: Option<f64>,
    pub timestamp: Instant,
    pub speed: Option<f64>,
    pub heading: Option<f64>,
}

/// Blue Force Track with history
#[derive(Clone, Debug)]
pub struct BlueForceTack {
    pub uid: String,
    pub callsign: String,
    pub affiliation: String,
    pub history: VecDeque<TrackPoint>,
    pub max_history: usize,
}

impl BlueForceTack {
    pub fn new(uid: String, callsign: String, affiliation: String) -> Self {
        Self {
            uid,
            callsign,
            affiliation,
            history: VecDeque::with_capacity(100),
            max_history: 100,
        }
    }

    pub fn add_point(&mut self, point: TrackPoint) {
        if self.history.len() >= self.max_history {
            self.history.pop_front();
        }
        self.history.push_back(point);
    }

    pub fn latest(&self) -> Option<&TrackPoint> {
        self.history.back()
    }

    pub fn calculate_speed_heading(&self) -> (Option<f64>, Option<f64>) {
        if self.history.len() < 2 {
            return (None, None);
        }

        let latest = self.history.back().unwrap();
        let prev = self.history.get(self.history.len() - 2).unwrap();

        let dt = latest
            .timestamp
            .duration_since(prev.timestamp)
            .as_secs_f64();
        if dt < 0.1 {
            return (None, None);
        }

        // Haversine distance calculation
        let lat1 = prev.lat.to_radians();
        let lat2 = latest.lat.to_radians();
        let dlat = (latest.lat - prev.lat).to_radians();
        let dlon = (latest.lon - prev.lon).to_radians();

        let a = (dlat / 2.0).sin().powi(2) + lat1.cos() * lat2.cos() * (dlon / 2.0).sin().powi(2);
        let c = 2.0 * a.sqrt().atan2((1.0 - a).sqrt());
        let distance = 6371000.0 * c; // Earth radius in meters

        let speed = distance / dt; // m/s

        // Calculate heading (bearing)
        let y = dlon.sin() * lat2.cos();
        let x = lat1.cos() * lat2.sin() - lat1.sin() * lat2.cos() * dlon.cos();
        let heading = y.atan2(x).to_degrees();
        let heading = (heading + 360.0) % 360.0;

        (Some(speed), Some(heading))
    }
}

/// Map panel state (persisted)
#[derive(Serialize, Deserialize)]
pub struct MapPanelState {
    /// Follow latest position
    pub follow_mode: bool,

    /// Show altitude labels
    pub show_altitude: bool,

    /// Altitude color coding enabled
    pub altitude_coloring: bool,

    /// Minimum altitude for color scale (meters)
    pub min_altitude: f64,

    /// Maximum altitude for color scale (meters)
    pub max_altitude: f64,

    /// Selected tile provider
    pub tile_provider: TileProvider,

    /// Mapbox API key (required for Mapbox providers)
    pub mapbox_api_key: String,

    /// High resolution tiles (Mapbox only, 1024x1024)
    pub high_resolution: bool,

    /// Current drawing tool
    pub drawing_tool: DrawingTool,

    /// Drawn shapes on the map
    pub shapes: Vec<DrawnShape>,

    /// Points being drawn (for multi-point shapes)
    #[serde(skip)]
    pub drawing_points: Vec<(f64, f64)>,

    /// Show track history trails
    pub show_trails: bool,

    /// Trail length (number of points)
    pub trail_length: usize,

    /// Show speed/heading indicators
    pub show_vectors: bool,

    /// Selected track UID
    #[serde(skip)]
    pub selected_track: Option<String>,

    /// Mouse position in geo coordinates
    #[serde(skip)]
    pub mouse_geo_pos: Option<(f64, f64)>,

    /// Last measurement result
    #[serde(skip)]
    pub measurement_result: Option<String>,

    /// HTTP tile downloader (not serialized)
    #[serde(skip)]
    pub tiles: Option<HttpTiles>,

    /// Map memory/state (not serialized)
    #[serde(skip)]
    pub map_memory: Option<MapMemory>,

    /// Current provider (to detect changes)
    #[serde(skip)]
    current_provider: Option<TileProvider>,

    /// Blue Force Tracks (not serialized - rebuilt from messages)
    #[serde(skip)]
    pub tracks: HashMap<String, BlueForceTack>,

    /// Offline map manager (not serialized)
    #[serde(skip)]
    pub offline_manager: OfflineMapManager,

    /// File picker promise for loading layers
    #[serde(skip)]
    pub layer_picker_promise: Option<poll_promise::Promise<Option<PathBuf>>>,
}

impl Default for MapPanelState {
    fn default() -> Self {
        Self {
            follow_mode: true,
            show_altitude: true,
            altitude_coloring: true,
            min_altitude: 0.0,
            max_altitude: 10000.0,
            tile_provider: TileProvider::OpenStreetMap,
            mapbox_api_key: String::new(),
            high_resolution: false,
            drawing_tool: DrawingTool::Select,
            shapes: vec![],
            drawing_points: vec![],
            show_trails: true,
            trail_length: 50,
            show_vectors: true,
            selected_track: None,
            mouse_geo_pos: None,
            measurement_result: None,
            tiles: None,
            map_memory: None,
            current_provider: None,
            tracks: HashMap::new(),
            offline_manager: OfflineMapManager::new(),
            layer_picker_promise: None,
        }
    }
}

impl MapPanelState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Create HttpTiles for the selected provider
    fn create_tiles(&self, ctx: egui::Context) -> Option<HttpTiles> {
        match self.tile_provider {
            TileProvider::OpenStreetMap => Some(HttpTiles::new(OpenStreetMap, ctx)),
            _ if self.mapbox_api_key.is_empty() => None,
            TileProvider::MapboxStreets => Some(HttpTiles::new(
                Mapbox {
                    style: MapboxStyle::Streets,
                    high_resolution: self.high_resolution,
                    access_token: self.mapbox_api_key.clone(),
                },
                ctx,
            )),
            TileProvider::MapboxOutdoors => Some(HttpTiles::new(
                Mapbox {
                    style: MapboxStyle::Outdoors,
                    high_resolution: self.high_resolution,
                    access_token: self.mapbox_api_key.clone(),
                },
                ctx,
            )),
            TileProvider::MapboxSatellite => Some(HttpTiles::new(
                Mapbox {
                    style: MapboxStyle::Satellite,
                    high_resolution: self.high_resolution,
                    access_token: self.mapbox_api_key.clone(),
                },
                ctx,
            )),
            TileProvider::MapboxSatelliteStreets => Some(HttpTiles::new(
                Mapbox {
                    style: MapboxStyle::SatelliteStreets,
                    high_resolution: self.high_resolution,
                    access_token: self.mapbox_api_key.clone(),
                },
                ctx,
            )),
            TileProvider::MapboxLight => Some(HttpTiles::new(
                Mapbox {
                    style: MapboxStyle::Light,
                    high_resolution: self.high_resolution,
                    access_token: self.mapbox_api_key.clone(),
                },
                ctx,
            )),
            TileProvider::MapboxDark => Some(HttpTiles::new(
                Mapbox {
                    style: MapboxStyle::Dark,
                    high_resolution: self.high_resolution,
                    access_token: self.mapbox_api_key.clone(),
                },
                ctx,
            )),
        }
    }
}

/// Converts altitude to color (low = blue, mid = green, high = red)
fn altitude_to_color(altitude: f64, min_alt: f64, max_alt: f64) -> egui::Color32 {
    let normalized = ((altitude - min_alt) / (max_alt - min_alt)).clamp(0.0, 1.0);

    if normalized < 0.5 {
        // Blue to green (0.0 - 0.5)
        let t = normalized * 2.0;
        egui::Color32::from_rgb(0, (255.0 * t) as u8, (255.0 * (1.0 - t)) as u8)
    } else {
        // Green to red (0.5 - 1.0)
        let t = (normalized - 0.5) * 2.0;
        egui::Color32::from_rgb((255.0 * t) as u8, (255.0 * (1.0 - t)) as u8, 0)
    }
}

/// Plugin for rendering tactical markers on the map
pub struct TacticalMarkersPlugin {
    positions: Vec<MessageLog>,
    show_altitude: bool,
    altitude_coloring: bool,
    min_altitude: f64,
    max_altitude: f64,
}

impl TacticalMarkersPlugin {
    pub fn new(
        positions: Vec<MessageLog>,
        show_altitude: bool,
        altitude_coloring: bool,
        min_altitude: f64,
        max_altitude: f64,
    ) -> Self {
        Self {
            positions,
            show_altitude,
            altitude_coloring,
            min_altitude,
            max_altitude,
        }
    }
}

impl Plugin for TacticalMarkersPlugin {
    fn run(
        self: Box<Self>,
        ui: &mut egui::Ui,
        _response: &egui::Response,
        projector: &Projector,
        _map_memory: &MapMemory,
    ) {
        let painter = ui.painter();

        for msg in &self.positions {
            if let (Some(lat), Some(lon)) = (msg.lat, msg.lon) {
                // Project geographic coordinates to screen coordinates
                let geo_pos = walkers::lat_lon(lat, lon);
                let screen_vec = projector.project(geo_pos);
                let screen_pos = egui::pos2(screen_vec.x, screen_vec.y);

                // Check if position is within the visible area
                if !ui.clip_rect().contains(screen_pos) {
                    continue;
                }

                let radius = 8.0;

                // Determine marker color
                let color = if self.altitude_coloring {
                    if let Some(alt) = msg.altitude {
                        altitude_to_color(alt, self.min_altitude, self.max_altitude)
                    } else {
                        egui::Color32::GRAY
                    }
                } else {
                    // Color by affiliation
                    match msg.affiliation.as_deref() {
                        Some("f") | Some("friend") => egui::Color32::BLUE,
                        Some("h") | Some("hostile") => egui::Color32::RED,
                        Some("n") | Some("neutral") => egui::Color32::GREEN,
                        Some("u") | Some("unknown") => egui::Color32::YELLOW,
                        _ => egui::Color32::GRAY,
                    }
                };

                // Draw marker circle with shadow for depth
                painter.circle_filled(
                    screen_pos + egui::vec2(2.0, 2.0),
                    radius,
                    egui::Color32::from_black_alpha(100),
                );
                painter.circle_filled(screen_pos, radius, color);
                painter.circle_stroke(
                    screen_pos,
                    radius,
                    egui::Stroke::new(2.0, egui::Color32::WHITE),
                );

                // Draw callsign label with background
                if let Some(callsign) = &msg.callsign {
                    let label_pos = screen_pos + egui::vec2(12.0, -8.0);
                    let galley = painter.layout_no_wrap(
                        callsign.clone(),
                        egui::FontId::proportional(12.0),
                        egui::Color32::WHITE,
                    );
                    let text_rect = galley.rect.translate(label_pos.to_vec2());

                    // Background for readability
                    painter.rect_filled(
                        text_rect.expand(2.0),
                        2.0,
                        egui::Color32::from_black_alpha(180),
                    );
                    painter.galley(label_pos, galley, egui::Color32::WHITE);
                }

                // Draw altitude label if enabled
                if self.show_altitude {
                    if let Some(alt) = msg.altitude {
                        let alt_text = format!("{:.0}m", alt);
                        let alt_pos = screen_pos + egui::vec2(12.0, 6.0);
                        let galley = painter.layout_no_wrap(
                            alt_text,
                            egui::FontId::proportional(10.0),
                            egui::Color32::LIGHT_GRAY,
                        );
                        let text_rect = galley.rect.translate(alt_pos.to_vec2());

                        painter.rect_filled(
                            text_rect.expand(1.0),
                            1.0,
                            egui::Color32::from_black_alpha(150),
                        );
                        painter.galley(alt_pos, galley, egui::Color32::LIGHT_GRAY);
                    }
                }
            }
        }
    }
}

/// Plugin for rendering Blue Force Tracking with trails and vectors
pub struct BlueForceTrackingPlugin {
    tracks: Vec<BlueForceTack>,
    show_trails: bool,
    trail_length: usize,
    show_vectors: bool,
    selected_track: Option<String>,
}

impl BlueForceTrackingPlugin {
    pub fn new(
        tracks: Vec<BlueForceTack>,
        show_trails: bool,
        trail_length: usize,
        show_vectors: bool,
        selected_track: Option<String>,
    ) -> Self {
        Self {
            tracks,
            show_trails,
            trail_length,
            show_vectors,
            selected_track,
        }
    }
}

impl Plugin for BlueForceTrackingPlugin {
    fn run(
        self: Box<Self>,
        ui: &mut egui::Ui,
        _response: &egui::Response,
        projector: &Projector,
        _map_memory: &MapMemory,
    ) {
        let painter = ui.painter();

        for track in &self.tracks {
            if track.history.is_empty() {
                continue;
            }

            let is_selected = self.selected_track.as_ref() == Some(&track.uid);
            let base_color = match track.affiliation.as_str() {
                "f" | "friend" => egui::Color32::from_rgb(50, 150, 255),
                "h" | "hostile" => egui::Color32::from_rgb(255, 50, 50),
                "n" | "neutral" => egui::Color32::from_rgb(50, 255, 50),
                _ => egui::Color32::from_rgb(255, 200, 50),
            };

            // Draw track history trail
            if self.show_trails && track.history.len() > 1 {
                let trail_points: Vec<egui::Pos2> = track
                    .history
                    .iter()
                    .rev()
                    .take(self.trail_length)
                    .rev()
                    .map(|pt| {
                        let geo = walkers::lat_lon(pt.lat, pt.lon);
                        let screen = projector.project(geo);
                        egui::pos2(screen.x, screen.y)
                    })
                    .collect();

                // Draw trail with fading opacity
                for i in 1..trail_points.len() {
                    let alpha = ((i as f32 / trail_points.len() as f32) * 180.0) as u8;
                    let trail_color = egui::Color32::from_rgba_unmultiplied(
                        base_color.r(),
                        base_color.g(),
                        base_color.b(),
                        alpha,
                    );
                    painter.line_segment(
                        [trail_points[i - 1], trail_points[i]],
                        egui::Stroke::new(2.0, trail_color),
                    );
                }
            }

            // Draw current position
            if let Some(latest) = track.latest() {
                let geo_pos = walkers::lat_lon(latest.lat, latest.lon);
                let screen_vec = projector.project(geo_pos);
                let screen_pos = egui::pos2(screen_vec.x, screen_vec.y);

                if !ui.clip_rect().contains(screen_pos) {
                    continue;
                }

                let radius = if is_selected { 12.0 } else { 8.0 };

                // Draw marker
                painter.circle_filled(
                    screen_pos + egui::vec2(2.0, 2.0),
                    radius,
                    egui::Color32::from_black_alpha(100),
                );
                painter.circle_filled(screen_pos, radius, base_color);
                painter.circle_stroke(
                    screen_pos,
                    radius,
                    egui::Stroke::new(if is_selected { 3.0 } else { 2.0 }, egui::Color32::WHITE),
                );

                // Draw speed/heading vector
                if self.show_vectors {
                    let (speed, heading) = track.calculate_speed_heading();
                    if let (Some(spd), Some(hdg)) = (speed, heading) {
                        if spd > 0.5 {
                            // Draw heading arrow
                            let arrow_len = (spd * 3.0).min(50.0).max(15.0) as f32;
                            let hdg_rad = hdg.to_radians();
                            let arrow_end = screen_pos
                                + egui::vec2(
                                    arrow_len * hdg_rad.sin() as f32,
                                    -arrow_len * hdg_rad.cos() as f32,
                                );

                            painter.arrow(
                                screen_pos,
                                arrow_end - screen_pos,
                                egui::Stroke::new(2.0, egui::Color32::YELLOW),
                            );

                            // Speed label
                            let speed_text = format!("{:.1} m/s", spd);
                            let speed_pos = arrow_end + egui::vec2(5.0, 0.0);
                            painter.text(
                                speed_pos,
                                egui::Align2::LEFT_CENTER,
                                speed_text,
                                egui::FontId::proportional(9.0),
                                egui::Color32::YELLOW,
                            );
                        }
                    }
                }

                // Draw callsign
                let label_pos = screen_pos + egui::vec2(radius + 4.0, -radius);
                let galley = painter.layout_no_wrap(
                    track.callsign.clone(),
                    egui::FontId::proportional(if is_selected { 14.0 } else { 12.0 }),
                    egui::Color32::WHITE,
                );
                let text_rect = galley.rect.translate(label_pos.to_vec2());
                painter.rect_filled(
                    text_rect.expand(2.0),
                    2.0,
                    egui::Color32::from_black_alpha(200),
                );
                painter.galley(label_pos, galley, egui::Color32::WHITE);
            }
        }
    }
}

/// Plugin for rendering drawn shapes
pub struct DrawnShapesPlugin {
    shapes: Vec<DrawnShape>,
    drawing_points: Vec<(f64, f64)>,
    current_tool: DrawingTool,
}

impl DrawnShapesPlugin {
    pub fn new(
        shapes: Vec<DrawnShape>,
        drawing_points: Vec<(f64, f64)>,
        current_tool: DrawingTool,
    ) -> Self {
        Self {
            shapes,
            drawing_points,
            current_tool,
        }
    }
}

impl Plugin for DrawnShapesPlugin {
    fn run(
        self: Box<Self>,
        ui: &mut egui::Ui,
        _response: &egui::Response,
        projector: &Projector,
        _map_memory: &MapMemory,
    ) {
        let painter = ui.painter();

        // Draw completed shapes
        for shape in &self.shapes {
            match shape {
                DrawnShape::Marker {
                    lat,
                    lon,
                    label,
                    color,
                } => {
                    let geo = walkers::lat_lon(*lat, *lon);
                    let screen = projector.project(geo);
                    let pos = egui::pos2(screen.x, screen.y);
                    let c = egui::Color32::from_rgb(color[0], color[1], color[2]);

                    painter.circle_filled(pos, 6.0, c);
                    painter.circle_stroke(pos, 6.0, egui::Stroke::new(2.0, egui::Color32::WHITE));

                    if !label.is_empty() {
                        painter.text(
                            pos + egui::vec2(10.0, 0.0),
                            egui::Align2::LEFT_CENTER,
                            label,
                            egui::FontId::proportional(11.0),
                            egui::Color32::WHITE,
                        );
                    }
                }
                DrawnShape::Line {
                    points,
                    color,
                    width,
                } => {
                    if points.len() >= 2 {
                        let screen_points: Vec<egui::Pos2> = points
                            .iter()
                            .map(|(lat, lon)| {
                                let geo = walkers::lat_lon(*lat, *lon);
                                let s = projector.project(geo);
                                egui::pos2(s.x, s.y)
                            })
                            .collect();

                        let c = egui::Color32::from_rgb(color[0], color[1], color[2]);
                        for i in 1..screen_points.len() {
                            painter.line_segment(
                                [screen_points[i - 1], screen_points[i]],
                                egui::Stroke::new(*width, c),
                            );
                        }
                    }
                }
                DrawnShape::Circle {
                    center_lat,
                    center_lon,
                    radius_m,
                    color,
                    filled,
                } => {
                    let center_geo = walkers::lat_lon(*center_lat, *center_lon);
                    let center_screen = projector.project(center_geo);
                    let center_pos = egui::pos2(center_screen.x, center_screen.y);

                    // Approximate screen radius (rough calculation)
                    let edge_lat = center_lat + (radius_m / 111320.0);
                    let edge_geo = walkers::lat_lon(edge_lat, *center_lon);
                    let edge_screen = projector.project(edge_geo);
                    let screen_radius = ((edge_screen.y - center_screen.y).abs()).max(5.0);

                    let c = egui::Color32::from_rgb(color[0], color[1], color[2]);
                    if *filled {
                        painter.circle_filled(center_pos, screen_radius, c.linear_multiply(0.3));
                    }
                    painter.circle_stroke(center_pos, screen_radius, egui::Stroke::new(2.0, c));
                }
                DrawnShape::Polygon {
                    points,
                    color,
                    filled,
                } => {
                    if points.len() >= 3 {
                        let screen_points: Vec<egui::Pos2> = points
                            .iter()
                            .map(|(lat, lon)| {
                                let geo = walkers::lat_lon(*lat, *lon);
                                let s = projector.project(geo);
                                egui::pos2(s.x, s.y)
                            })
                            .collect();

                        let c = egui::Color32::from_rgb(color[0], color[1], color[2]);
                        if *filled {
                            painter.add(egui::Shape::convex_polygon(
                                screen_points.clone(),
                                c.linear_multiply(0.3),
                                egui::Stroke::new(2.0, c),
                            ));
                        } else {
                            for i in 0..screen_points.len() {
                                let next = (i + 1) % screen_points.len();
                                painter.line_segment(
                                    [screen_points[i], screen_points[next]],
                                    egui::Stroke::new(2.0, c),
                                );
                            }
                        }
                    }
                }
                DrawnShape::RangeRing {
                    center_lat,
                    center_lon,
                    rings,
                    color,
                } => {
                    let center_geo = walkers::lat_lon(*center_lat, *center_lon);
                    let center_screen = projector.project(center_geo);
                    let center_pos = egui::pos2(center_screen.x, center_screen.y);
                    let c = egui::Color32::from_rgb(color[0], color[1], color[2]);

                    for radius_m in rings {
                        let edge_lat = center_lat + (radius_m / 111320.0);
                        let edge_geo = walkers::lat_lon(edge_lat, *center_lon);
                        let edge_screen = projector.project(edge_geo);
                        let screen_radius = ((edge_screen.y - center_screen.y).abs()).max(5.0);

                        painter.circle_stroke(center_pos, screen_radius, egui::Stroke::new(1.5, c));

                        // Label the ring
                        let label = if *radius_m >= 1000.0 {
                            format!("{:.1}km", radius_m / 1000.0)
                        } else {
                            format!("{:.0}m", radius_m)
                        };
                        painter.text(
                            center_pos + egui::vec2(screen_radius, 0.0),
                            egui::Align2::LEFT_CENTER,
                            label,
                            egui::FontId::proportional(10.0),
                            c,
                        );
                    }
                }
            }
        }

        // Draw in-progress shape
        if !self.drawing_points.is_empty() {
            let screen_points: Vec<egui::Pos2> = self
                .drawing_points
                .iter()
                .map(|(lat, lon)| {
                    let geo = walkers::lat_lon(*lat, *lon);
                    let s = projector.project(geo);
                    egui::pos2(s.x, s.y)
                })
                .collect();

            let preview_color = egui::Color32::from_rgb(255, 165, 0); // Orange

            match self.current_tool {
                DrawingTool::Line | DrawingTool::Measure => {
                    for i in 1..screen_points.len() {
                        painter.line_segment(
                            [screen_points[i - 1], screen_points[i]],
                            egui::Stroke::new(2.0, preview_color),
                        );
                    }
                    for pt in &screen_points {
                        painter.circle_filled(*pt, 4.0, preview_color);
                    }
                }
                DrawingTool::Polygon => {
                    for i in 1..screen_points.len() {
                        painter.line_segment(
                            [screen_points[i - 1], screen_points[i]],
                            egui::Stroke::new(2.0, preview_color),
                        );
                    }
                    if screen_points.len() > 2 {
                        painter.line_segment(
                            [*screen_points.last().unwrap(), screen_points[0]],
                            egui::Stroke::new(1.0, preview_color.linear_multiply(0.5)),
                        );
                    }
                    for pt in &screen_points {
                        painter.circle_filled(*pt, 4.0, preview_color);
                    }
                }
                _ => {}
            }
        }
    }
}

/// Plugin for rendering GeoJSON and KML overlays
pub struct OverlayLayersPlugin {
    geojson_layers: Vec<crate::ui::offline_maps::GeoJsonLayer>,
    kml_layers: Vec<crate::ui::offline_maps::KmlLayer>,
}

impl OverlayLayersPlugin {
    pub fn new(
        geojson_layers: Vec<crate::ui::offline_maps::GeoJsonLayer>,
        kml_layers: Vec<crate::ui::offline_maps::KmlLayer>,
    ) -> Self {
        Self {
            geojson_layers,
            kml_layers,
        }
    }
}

impl Plugin for OverlayLayersPlugin {
    fn run(
        self: Box<Self>,
        ui: &mut egui::Ui,
        _response: &egui::Response,
        projector: &Projector,
        _map_memory: &MapMemory,
    ) {
        render_overlays(ui, projector, &self.geojson_layers, &self.kml_layers);
    }
}

/// Renders a position marker on the map
#[allow(dead_code)]
fn draw_marker(
    painter: &egui::Painter,
    screen_pos: egui::Pos2,
    msg: &MessageLog,
    state: &MapPanelState,
) {
    let radius = 8.0;

    // Determine marker color
    let color = if state.altitude_coloring {
        if let Some(alt) = msg.altitude {
            altitude_to_color(alt, state.min_altitude, state.max_altitude)
        } else {
            egui::Color32::GRAY
        }
    } else {
        // Color by affiliation if available
        match msg.affiliation.as_deref() {
            Some("f") | Some("friend") => egui::Color32::BLUE,
            Some("h") | Some("hostile") => egui::Color32::RED,
            Some("n") | Some("neutral") => egui::Color32::GREEN,
            Some("u") | Some("unknown") => egui::Color32::YELLOW,
            _ => egui::Color32::GRAY,
        }
    };

    // Draw marker circle
    painter.circle_filled(screen_pos, radius, color);
    painter.circle_stroke(
        screen_pos,
        radius,
        egui::Stroke::new(2.0, egui::Color32::WHITE),
    );

    // Draw callsign label
    if let Some(callsign) = &msg.callsign {
        let label_pos = screen_pos + egui::vec2(10.0, -10.0);
        painter.text(
            label_pos,
            egui::Align2::LEFT_CENTER,
            callsign,
            egui::FontId::proportional(12.0),
            egui::Color32::WHITE,
        );
    }

    // Draw altitude label if enabled
    if state.show_altitude {
        if let Some(alt) = msg.altitude {
            let alt_text = format!("{:.0}m", alt);
            let alt_pos = screen_pos + egui::vec2(10.0, 5.0);
            painter.text(
                alt_pos,
                egui::Align2::LEFT_CENTER,
                &alt_text,
                egui::FontId::proportional(10.0),
                egui::Color32::LIGHT_GRAY,
            );
        }
    }
}

/// Calculate distance between two geographic points (Haversine)
fn haversine_distance(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let r = 6371000.0; // Earth radius in meters
    let dlat = (lat2 - lat1).to_radians();
    let dlon = (lon2 - lon1).to_radians();
    let a = (dlat / 2.0).sin().powi(2)
        + lat1.to_radians().cos() * lat2.to_radians().cos() * (dlon / 2.0).sin().powi(2);
    let c = 2.0 * a.sqrt().atan2((1.0 - a).sqrt());
    r * c
}

/// Shows the map panel
pub fn show(ui: &mut egui::Ui, app_state: &Arc<Mutex<AppState>>, map_state: &mut MapPanelState) {
    ui.heading("Tactical Map");

    // Drawing toolbar
    ui.horizontal(|ui| {
        ui.label("Tools:");
        let tools = [
            DrawingTool::Select,
            DrawingTool::Marker,
            DrawingTool::Line,
            DrawingTool::Circle,
            DrawingTool::Polygon,
            DrawingTool::Measure,
            DrawingTool::RangeRing,
        ];

        for tool in &tools {
            let selected = map_state.drawing_tool == *tool;
            if ui
                .selectable_label(selected, format!("{} {}", tool.icon(), tool.name()))
                .clicked()
            {
                map_state.drawing_tool = *tool;
                map_state.drawing_points.clear();
            }
        }

        ui.separator();

        if !map_state.shapes.is_empty() {
            if ui.button("🗑️ Clear Shapes").clicked() {
                map_state.shapes.clear();
            }
        }

        if !map_state.drawing_points.is_empty() {
            if ui.button("✓ Finish").clicked() {
                // Finalize current shape
                match map_state.drawing_tool {
                    DrawingTool::Line => {
                        if map_state.drawing_points.len() >= 2 {
                            map_state.shapes.push(DrawnShape::Line {
                                points: map_state.drawing_points.clone(),
                                color: [255, 100, 100],
                                width: 2.0,
                            });
                        }
                    }
                    DrawingTool::Polygon => {
                        if map_state.drawing_points.len() >= 3 {
                            map_state.shapes.push(DrawnShape::Polygon {
                                points: map_state.drawing_points.clone(),
                                color: [100, 255, 100],
                                filled: true,
                            });
                        }
                    }
                    DrawingTool::Measure => {
                        // Calculate total distance
                        let mut total_dist = 0.0;
                        for i in 1..map_state.drawing_points.len() {
                            let (lat1, lon1) = map_state.drawing_points[i - 1];
                            let (lat2, lon2) = map_state.drawing_points[i];
                            total_dist += haversine_distance(lat1, lon1, lat2, lon2);
                        }
                        map_state.measurement_result = Some(if total_dist >= 1000.0 {
                            format!("Distance: {:.2} km", total_dist / 1000.0)
                        } else {
                            format!("Distance: {:.1} m", total_dist)
                        });
                    }
                    _ => {}
                }
                map_state.drawing_points.clear();
            }
            if ui.button("✗ Cancel").clicked() {
                map_state.drawing_points.clear();
            }
        }
    });

    // Blue Force Tracking controls
    ui.horizontal(|ui| {
        ui.checkbox(&mut map_state.show_trails, "Show Trails");
        if map_state.show_trails {
            ui.add(
                egui::DragValue::new(&mut map_state.trail_length)
                    .prefix("Length: ")
                    .range(10..=200),
            );
        }
        ui.separator();
        ui.checkbox(&mut map_state.show_vectors, "Speed/Heading");
        ui.separator();
        ui.checkbox(&mut map_state.follow_mode, "Follow Latest");
        ui.separator();
        ui.checkbox(&mut map_state.show_altitude, "Show Altitude");
    });

    // Measurement result display
    if let Some(result) = &map_state.measurement_result {
        let result_text = result.clone();
        let mut clear_result = false;
        ui.horizontal(|ui| {
            ui.colored_label(egui::Color32::GREEN, format!("📏 {}", result_text));
            if ui.small_button("✗").clicked() {
                clear_result = true;
            }
        });
        if clear_result {
            map_state.measurement_result = None;
        }
    }

    ui.separator();

    // Tile provider selection (collapsible)
    egui::CollapsingHeader::new("Map Settings")
        .default_open(false)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label("Map Source:");
                egui::ComboBox::from_id_salt("tile_provider")
                    .selected_text(map_state.tile_provider.display_name())
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut map_state.tile_provider,
                            TileProvider::OpenStreetMap,
                            "OpenStreetMap (Free)",
                        );
                        ui.separator();
                        ui.label("Mapbox (API Key Required):");
                        ui.selectable_value(
                            &mut map_state.tile_provider,
                            TileProvider::MapboxStreets,
                            "Streets",
                        );
                        ui.selectable_value(
                            &mut map_state.tile_provider,
                            TileProvider::MapboxOutdoors,
                            "Outdoors",
                        );
                        ui.selectable_value(
                            &mut map_state.tile_provider,
                            TileProvider::MapboxSatellite,
                            "Satellite",
                        );
                        ui.selectable_value(
                            &mut map_state.tile_provider,
                            TileProvider::MapboxSatelliteStreets,
                            "Satellite + Streets",
                        );
                        ui.selectable_value(
                            &mut map_state.tile_provider,
                            TileProvider::MapboxLight,
                            "Light",
                        );
                        ui.selectable_value(
                            &mut map_state.tile_provider,
                            TileProvider::MapboxDark,
                            "Dark",
                        );
                    });

                if map_state.tile_provider.requires_api_key() {
                    ui.checkbox(&mut map_state.high_resolution, "Hi-Res");
                }
            });

            if map_state.tile_provider.requires_api_key() {
                ui.horizontal(|ui| {
                    ui.label("Mapbox API Key:");
                    ui.add(
                        egui::TextEdit::singleline(&mut map_state.mapbox_api_key)
                            .password(true)
                            .hint_text("Enter your Mapbox access token")
                            .desired_width(300.0),
                    );
                    if map_state.mapbox_api_key.is_empty() {
                        ui.colored_label(egui::Color32::YELLOW, "⚠ Required");
                    } else {
                        ui.colored_label(egui::Color32::GREEN, "✓");
                    }
                });
            }

            ui.horizontal(|ui| {
                ui.checkbox(&mut map_state.altitude_coloring, "Altitude Colors");
                if map_state.altitude_coloring {
                    ui.label("Range:");
                    ui.add(
                        egui::DragValue::new(&mut map_state.min_altitude)
                            .suffix("m")
                            .speed(10.0),
                    );
                    ui.label("-");
                    ui.add(
                        egui::DragValue::new(&mut map_state.max_altitude)
                            .suffix("m")
                            .speed(10.0),
                    );
                }
            });
        });

    // Handle layer file picker
    if let Some(promise) = &map_state.layer_picker_promise {
        if let Some(result) = promise.ready() {
            if let Some(path) = result {
                let ext = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("")
                    .to_lowercase();
                match ext.as_str() {
                    "geojson" | "json" => {
                        if let Err(e) = map_state.offline_manager.add_geojson(path.clone()) {
                            tracing::error!("Failed to load GeoJSON: {}", e);
                        }
                    }
                    "kml" => {
                        if let Err(e) = map_state.offline_manager.add_kml(path.clone()) {
                            tracing::error!("Failed to load KML: {}", e);
                        }
                    }
                    "mbtiles" => {
                        if let Err(e) = map_state.offline_manager.add_mbtiles(path.clone()) {
                            tracing::error!("Failed to load MBTiles: {}", e);
                        }
                    }
                    _ => {}
                }
            }
            map_state.layer_picker_promise = None;
        }
    }

    // Layers management panel
    egui::CollapsingHeader::new(format!(
        "Layers ({} GeoJSON, {} KML)",
        map_state.offline_manager.geojson_layers.len(),
        map_state.offline_manager.kml_layers.len()
    ))
    .default_open(false)
    .show(ui, |ui| {
        ui.horizontal(|ui| {
            if ui.button("📂 Load GeoJSON").clicked() && map_state.layer_picker_promise.is_none()
            {
                map_state.layer_picker_promise = Some(poll_promise::Promise::spawn_thread(
                    "geojson_picker",
                    || {
                        rfd::FileDialog::new()
                            .add_filter("GeoJSON", &["geojson", "json"])
                            .pick_file()
                    },
                ));
            }

            if ui.button("📂 Load KML").clicked() && map_state.layer_picker_promise.is_none() {
                map_state.layer_picker_promise =
                    Some(poll_promise::Promise::spawn_thread("kml_picker", || {
                        rfd::FileDialog::new()
                            .add_filter("KML", &["kml"])
                            .pick_file()
                    }));
            }

            if ui.button("📂 Load MBTiles").clicked() && map_state.layer_picker_promise.is_none()
            {
                map_state.layer_picker_promise = Some(poll_promise::Promise::spawn_thread(
                    "mbtiles_picker",
                    || {
                        rfd::FileDialog::new()
                            .add_filter("MBTiles", &["mbtiles"])
                            .pick_file()
                    },
                ));
            }
        });

        // List GeoJSON layers
        if !map_state.offline_manager.geojson_layers.is_empty() {
            ui.separator();
            ui.label("GeoJSON Layers:");
            let mut to_remove = None;
            for (i, layer) in map_state
                .offline_manager
                .geojson_layers
                .iter_mut()
                .enumerate()
            {
                ui.horizontal(|ui| {
                    ui.checkbox(&mut layer.visible, "");
                    ui.label(&layer.name);
                    ui.label(format!("({} features)", layer.features.len()));
                    if ui.small_button("🗑️").clicked() {
                        to_remove = Some(i);
                    }
                });
            }
            if let Some(idx) = to_remove {
                map_state.offline_manager.geojson_layers.remove(idx);
            }
        }

        // List KML layers
        if !map_state.offline_manager.kml_layers.is_empty() {
            ui.separator();
            ui.label("KML Layers:");
            let mut to_remove = None;
            for (i, layer) in map_state.offline_manager.kml_layers.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    ui.checkbox(&mut layer.visible, "");
                    ui.label(&layer.name);
                    ui.label(format!("({} placemarks)", layer.placemarks.len()));
                    if ui.small_button("🗑️").clicked() {
                        to_remove = Some(i);
                    }
                });
            }
            if let Some(idx) = to_remove {
                map_state.offline_manager.kml_layers.remove(idx);
            }
        }

        // List MBTiles sources
        if !map_state.offline_manager.mbtiles_sources.is_empty() {
            ui.separator();
            ui.label("Offline Map Sources:");
            for (path, source) in &map_state.offline_manager.mbtiles_sources {
                ui.horizontal(|ui| {
                    ui.label("📦");
                    ui.label(&source.metadata().name);
                    ui.label(format!(
                        "(zoom {}-{})",
                        source.metadata().min_zoom,
                        source.metadata().max_zoom
                    ));
                });
            }
        }

        // Cache statistics
        if let Some(stats) = map_state.offline_manager.cache_stats() {
            ui.separator();
            ui.label(format!(
                "Tile Cache: {} tiles, {:.1} MB",
                stats.tile_count,
                stats.size_bytes as f64 / 1_048_576.0
            ));
            if ui.small_button("Clear Cache").clicked() {
                let _ = map_state.offline_manager.clear_cache();
            }
        }
    });

    // Get messages with positions and update tracks
    let state = app_state.lock().unwrap();
    let positions: Vec<MessageLog> = state
        .message_log
        .iter()
        .filter(|msg| msg.lat.is_some() && msg.lon.is_some())
        .cloned()
        .collect();
    drop(state);

    // Update Blue Force Tracks from messages
    for msg in &positions {
        if let (Some(lat), Some(lon), Some(uid)) = (msg.lat, msg.lon, msg.uid.as_ref()) {
            let callsign = msg.callsign.clone().unwrap_or_else(|| uid.clone());
            let affiliation = msg.affiliation.clone().unwrap_or_else(|| "u".to_string());

            let track = map_state.tracks.entry(uid.clone()).or_insert_with(|| {
                BlueForceTack::new(uid.clone(), callsign.clone(), affiliation.clone())
            });

            // Update callsign and affiliation if they changed
            track.callsign = callsign;
            track.affiliation = affiliation;

            // Add track point
            let point = TrackPoint {
                lat,
                lon,
                altitude: msg.altitude,
                timestamp: Instant::now(),
                speed: None,
                heading: None,
            };
            track.add_point(point);
        }
    }

    // Determine map center
    let center_pos = if map_state.follow_mode {
        positions
            .last()
            .and_then(|msg| {
                if let (Some(lat), Some(lon)) = (msg.lat, msg.lon) {
                    Some(walkers::lat_lon(lat, lon))
                } else {
                    None
                }
            })
            .unwrap_or_else(|| walkers::lat_lon(37.7749, -122.4194))
    } else {
        walkers::lat_lon(37.7749, -122.4194)
    };

    // Check if provider changed and reinitialize tiles
    let provider_changed = map_state.current_provider != Some(map_state.tile_provider);
    if provider_changed || map_state.tiles.is_none() {
        map_state.tiles = map_state.create_tiles(ui.ctx().clone());
        map_state.current_provider = Some(map_state.tile_provider);
    }

    if map_state.map_memory.is_none() {
        map_state.map_memory = Some(MapMemory::default());
    }

    let has_tiles = map_state.tiles.is_some();
    if !has_tiles && map_state.tile_provider.requires_api_key() {
        ui.colored_label(
            egui::Color32::RED,
            "Please enter a valid Mapbox API key to view the map.",
        );
        return;
    }

    let tiles: Option<&mut dyn Tiles> = map_state.tiles.as_mut().map(|t| t as &mut dyn Tiles);
    let memory = map_state.map_memory.as_mut().unwrap();

    // Create plugins
    let markers_plugin = TacticalMarkersPlugin::new(
        positions.clone(),
        map_state.show_altitude,
        map_state.altitude_coloring,
        map_state.min_altitude,
        map_state.max_altitude,
    );

    let bft_plugin = BlueForceTrackingPlugin::new(
        map_state.tracks.values().cloned().collect(),
        map_state.show_trails,
        map_state.trail_length,
        map_state.show_vectors,
        map_state.selected_track.clone(),
    );

    let shapes_plugin = DrawnShapesPlugin::new(
        map_state.shapes.clone(),
        map_state.drawing_points.clone(),
        map_state.drawing_tool,
    );

    let overlay_plugin = OverlayLayersPlugin::new(
        map_state.offline_manager.geojson_layers.clone(),
        map_state.offline_manager.kml_layers.clone(),
    );

    // Map widget with all plugins
    let map_response = ui.add(
        Map::new(tiles, memory, center_pos)
            .with_plugin(overlay_plugin)
            .with_plugin(shapes_plugin)
            .with_plugin(bft_plugin)
            .with_plugin(markers_plugin),
    );

    // Handle map interactions for drawing
    if map_response.clicked() && map_state.drawing_tool != DrawingTool::Select {
        if let Some(pos) = map_response.interact_pointer_pos() {
            // Convert screen position to geo coordinates using proper projection
            let projector = Projector::new(map_response.rect, memory, center_pos);
            let screen_vec = egui::vec2(pos.x, pos.y);
            let geo_pos = projector.unproject(screen_vec);
            // geo_types::Point uses x() for longitude, y() for latitude
            let click_lon = geo_pos.x();
            let click_lat = geo_pos.y();

            match map_state.drawing_tool {
                DrawingTool::Marker => {
                    map_state.shapes.push(DrawnShape::Marker {
                        lat: click_lat,
                        lon: click_lon,
                        label: format!("Marker {}", map_state.shapes.len() + 1),
                        color: [255, 200, 50],
                    });
                }
                DrawingTool::Circle => {
                    if map_state.drawing_points.is_empty() {
                        map_state.drawing_points.push((click_lat, click_lon));
                    } else {
                        let (center_lat, center_lon) = map_state.drawing_points[0];
                        let radius =
                            haversine_distance(center_lat, center_lon, click_lat, click_lon);
                        map_state.shapes.push(DrawnShape::Circle {
                            center_lat,
                            center_lon,
                            radius_m: radius,
                            color: [100, 200, 255],
                            filled: false,
                        });
                        map_state.drawing_points.clear();
                    }
                }
                DrawingTool::RangeRing => {
                    map_state.shapes.push(DrawnShape::RangeRing {
                        center_lat: click_lat,
                        center_lon: click_lon,
                        rings: vec![500.0, 1000.0, 2000.0, 5000.0],
                        color: [255, 100, 100],
                    });
                }
                DrawingTool::Line | DrawingTool::Polygon | DrawingTool::Measure => {
                    map_state.drawing_points.push((click_lat, click_lon));
                }
                _ => {}
            }
        }
    }

    // Status bar with mouse position
    ui.horizontal(|ui| {
        ui.label(format!("📍 {} tracks", map_state.tracks.len()));
        ui.separator();
        ui.label(format!("🔷 {} shapes", map_state.shapes.len()));
        ui.separator();
        let zoom = memory.zoom();
        ui.label(format!("🔍 Zoom: {:.1}", zoom));

        if let Some((lat, lon)) = map_state.mouse_geo_pos {
            ui.separator();
            ui.label(format!("📌 {:.5}, {:.5}", lat, lon));
        }
    });

    // Blue Force Tracking panel
    egui::CollapsingHeader::new(format!("Blue Force Tracks ({})", map_state.tracks.len()))
        .default_open(true)
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .max_height(200.0)
                .show(ui, |ui| {
                    let mut tracks: Vec<_> = map_state.tracks.values().collect();
                    tracks.sort_by_key(|t| &t.callsign);

                    for track in tracks {
                        let is_selected = map_state.selected_track.as_ref() == Some(&track.uid);
                        let (speed, heading) = track.calculate_speed_heading();

                        ui.horizontal(|ui| {
                            let color = match track.affiliation.as_str() {
                                "f" | "friend" => egui::Color32::from_rgb(50, 150, 255),
                                "h" | "hostile" => egui::Color32::from_rgb(255, 50, 50),
                                "n" | "neutral" => egui::Color32::from_rgb(50, 255, 50),
                                _ => egui::Color32::from_rgb(255, 200, 50),
                            };
                            ui.colored_label(color, "●");

                            if ui.selectable_label(is_selected, &track.callsign).clicked() {
                                if is_selected {
                                    map_state.selected_track = None;
                                } else {
                                    map_state.selected_track = Some(track.uid.clone());
                                }
                            }

                            if let Some(latest) = track.latest() {
                                ui.label(format!("({:.4}, {:.4})", latest.lat, latest.lon));

                                if let Some(spd) = speed {
                                    ui.colored_label(
                                        egui::Color32::YELLOW,
                                        format!("{:.1}m/s", spd),
                                    );
                                }

                                if let Some(hdg) = heading {
                                    ui.label(format!("{}°", hdg as i32));
                                }
                            }
                        });
                    }
                });
        });
}
