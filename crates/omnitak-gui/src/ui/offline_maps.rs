//! Offline map support including MBTiles, tile caching, and overlay formats
//!
//! Provides:
//! - MBTiles (.mbtiles) offline tile database support
//! - Tile caching for online maps (SQLite-based)
//! - GeoJSON overlay loading
//! - KML/KMZ file parsing

use anyhow::{anyhow, Result};
use eframe::egui;
use rusqlite::{params, Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use walkers::{TileId, Tiles};

/// MBTiles database wrapper for offline map tiles
pub struct MBTilesSource {
    connection: Arc<Mutex<Connection>>,
    metadata: MBTilesMetadata,
}

/// MBTiles metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MBTilesMetadata {
    pub name: String,
    pub format: String,
    pub bounds: Option<[f64; 4]>, // [west, south, east, north]
    pub center: Option<[f64; 3]>, // [lon, lat, zoom]
    pub min_zoom: u8,
    pub max_zoom: u8,
    pub attribution: Option<String>,
    pub description: Option<String>,
}

impl MBTilesSource {
    /// Open an MBTiles file
    pub fn open(path: &Path) -> Result<Self> {
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;

        // Read metadata
        let metadata = Self::read_metadata(&connection)?;

        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
            metadata,
        })
    }

    fn read_metadata(conn: &Connection) -> Result<MBTilesMetadata> {
        let mut stmt = conn.prepare("SELECT name, value FROM metadata")?;
        let mut rows = stmt.query([])?;

        let mut metadata_map: HashMap<String, String> = HashMap::new();
        while let Some(row) = rows.next()? {
            let name: String = row.get(0)?;
            let value: String = row.get(1)?;
            metadata_map.insert(name, value);
        }

        let name = metadata_map
            .get("name")
            .cloned()
            .unwrap_or_else(|| "Unknown".to_string());
        let format = metadata_map
            .get("format")
            .cloned()
            .unwrap_or_else(|| "png".to_string());

        let bounds = metadata_map.get("bounds").and_then(|s| {
            let parts: Vec<f64> = s.split(',').filter_map(|p| p.trim().parse().ok()).collect();
            if parts.len() == 4 {
                Some([parts[0], parts[1], parts[2], parts[3]])
            } else {
                None
            }
        });

        let center = metadata_map.get("center").and_then(|s| {
            let parts: Vec<f64> = s.split(',').filter_map(|p| p.trim().parse().ok()).collect();
            if parts.len() == 3 {
                Some([parts[0], parts[1], parts[2]])
            } else {
                None
            }
        });

        let min_zoom = metadata_map
            .get("minzoom")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let max_zoom = metadata_map
            .get("maxzoom")
            .and_then(|s| s.parse().ok())
            .unwrap_or(18);

        let attribution = metadata_map.get("attribution").cloned();
        let description = metadata_map.get("description").cloned();

        Ok(MBTilesMetadata {
            name,
            format,
            bounds,
            center,
            min_zoom,
            max_zoom,
            attribution,
            description,
        })
    }

    /// Get a tile from the database
    pub fn get_tile(&self, zoom: u8, x: u32, y: u32) -> Result<Option<Vec<u8>>> {
        let conn = self
            .connection
            .lock()
            .map_err(|e| anyhow!("Lock error: {}", e))?;

        // MBTiles uses TMS (flipped Y) coordinate system
        let tms_y = (1 << zoom) - 1 - y;

        let mut stmt = conn.prepare(
            "SELECT tile_data FROM tiles WHERE zoom_level = ? AND tile_column = ? AND tile_row = ?",
        )?;

        let result: Result<Vec<u8>, _> = stmt.query_row(params![zoom, x, tms_y], |row| row.get(0));

        match result {
            Ok(data) => Ok(Some(data)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn metadata(&self) -> &MBTilesMetadata {
        &self.metadata
    }
}

/// Tile cache using SQLite for persistent storage
pub struct TileCache {
    connection: Arc<Mutex<Connection>>,
    max_size_mb: usize,
}

impl TileCache {
    /// Create or open a tile cache database
    pub fn open(path: &Path, max_size_mb: usize) -> Result<Self> {
        let connection = Connection::open(path)?;

        // Create tables if they don't exist
        connection.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS tiles (
                source TEXT NOT NULL,
                zoom INTEGER NOT NULL,
                x INTEGER NOT NULL,
                y INTEGER NOT NULL,
                data BLOB NOT NULL,
                timestamp INTEGER NOT NULL,
                PRIMARY KEY (source, zoom, x, y)
            );
            CREATE INDEX IF NOT EXISTS idx_timestamp ON tiles(timestamp);
            "#,
        )?;

        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
            max_size_mb,
        })
    }

    /// Get a cached tile
    pub fn get(&self, source: &str, zoom: u8, x: u32, y: u32) -> Result<Option<Vec<u8>>> {
        let conn = self
            .connection
            .lock()
            .map_err(|e| anyhow!("Lock error: {}", e))?;

        let mut stmt = conn
            .prepare("SELECT data FROM tiles WHERE source = ? AND zoom = ? AND x = ? AND y = ?")?;

        let result: Result<Vec<u8>, _> =
            stmt.query_row(params![source, zoom, x, y], |row| row.get(0));

        match result {
            Ok(data) => Ok(Some(data)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Store a tile in the cache
    pub fn put(&self, source: &str, zoom: u8, x: u32, y: u32, data: &[u8]) -> Result<()> {
        let conn = self
            .connection
            .lock()
            .map_err(|e| anyhow!("Lock error: {}", e))?;

        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        conn.execute(
            "INSERT OR REPLACE INTO tiles (source, zoom, x, y, data, timestamp) VALUES (?, ?, ?, ?, ?, ?)",
            params![source, zoom, x, y, data, timestamp],
        )?;

        // Check cache size and evict old tiles if necessary
        self.evict_if_needed(&conn)?;

        Ok(())
    }

    fn evict_if_needed(&self, conn: &Connection) -> Result<()> {
        let size: i64 = conn.query_row(
            "SELECT COALESCE(SUM(LENGTH(data)), 0) FROM tiles",
            [],
            |row| row.get(0),
        )?;

        let max_bytes = (self.max_size_mb * 1024 * 1024) as i64;

        if size > max_bytes {
            // Delete oldest 10% of tiles
            let to_delete = size / 10;
            conn.execute(
                "DELETE FROM tiles WHERE rowid IN (
                    SELECT rowid FROM tiles ORDER BY timestamp ASC LIMIT ?
                )",
                params![to_delete / 50000], // Approximate 50KB per tile
            )?;
        }

        Ok(())
    }

    /// Get cache statistics
    pub fn stats(&self) -> Result<CacheStats> {
        let conn = self
            .connection
            .lock()
            .map_err(|e| anyhow!("Lock error: {}", e))?;

        let count: i64 = conn.query_row("SELECT COUNT(*) FROM tiles", [], |row| row.get(0))?;
        let size: i64 = conn.query_row(
            "SELECT COALESCE(SUM(LENGTH(data)), 0) FROM tiles",
            [],
            |row| row.get(0),
        )?;

        Ok(CacheStats {
            tile_count: count as usize,
            size_bytes: size as usize,
        })
    }

    /// Clear the entire cache
    pub fn clear(&self) -> Result<()> {
        let conn = self
            .connection
            .lock()
            .map_err(|e| anyhow!("Lock error: {}", e))?;
        conn.execute("DELETE FROM tiles", [])?;
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct CacheStats {
    pub tile_count: usize,
    pub size_bytes: usize,
}

/// GeoJSON overlay layer
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeoJsonLayer {
    pub name: String,
    pub visible: bool,
    pub features: Vec<GeoFeature>,
    pub color: [u8; 3],
    pub opacity: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GeoFeature {
    Point {
        lat: f64,
        lon: f64,
        properties: HashMap<String, String>,
    },
    LineString {
        points: Vec<(f64, f64)>,
        properties: HashMap<String, String>,
    },
    Polygon {
        rings: Vec<Vec<(f64, f64)>>,
        properties: HashMap<String, String>,
    },
}

impl GeoJsonLayer {
    /// Load from a GeoJSON file
    pub fn from_file(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let geojson: geojson::GeoJson = content.parse()?;

        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Untitled")
            .to_string();

        let mut features = Vec::new();

        match geojson {
            geojson::GeoJson::FeatureCollection(fc) => {
                for feature in fc.features {
                    if let Some(geom) = feature.geometry {
                        let props = feature
                            .properties
                            .map(|p| {
                                p.into_iter()
                                    .filter_map(|(k, v)| v.as_str().map(|s| (k, s.to_string())))
                                    .collect()
                            })
                            .unwrap_or_default();

                        match geom.value {
                            geojson::Value::Point(coords) => {
                                if coords.len() >= 2 {
                                    features.push(GeoFeature::Point {
                                        lon: coords[0],
                                        lat: coords[1],
                                        properties: props,
                                    });
                                }
                            }
                            geojson::Value::LineString(coords) => {
                                let points: Vec<(f64, f64)> = coords
                                    .into_iter()
                                    .filter_map(|c| {
                                        if c.len() >= 2 {
                                            Some((c[1], c[0])) // lat, lon
                                        } else {
                                            None
                                        }
                                    })
                                    .collect();
                                features.push(GeoFeature::LineString {
                                    points,
                                    properties: props,
                                });
                            }
                            geojson::Value::Polygon(rings) => {
                                let parsed_rings: Vec<Vec<(f64, f64)>> = rings
                                    .into_iter()
                                    .map(|ring| {
                                        ring.into_iter()
                                            .filter_map(|c| {
                                                if c.len() >= 2 {
                                                    Some((c[1], c[0])) // lat, lon
                                                } else {
                                                    None
                                                }
                                            })
                                            .collect()
                                    })
                                    .collect();
                                features.push(GeoFeature::Polygon {
                                    rings: parsed_rings,
                                    properties: props,
                                });
                            }
                            _ => {} // Skip other geometry types for now
                        }
                    }
                }
            }
            _ => return Err(anyhow!("Expected a FeatureCollection")),
        }

        Ok(Self {
            name,
            visible: true,
            features,
            color: [255, 165, 0], // Orange
            opacity: 0.8,
        })
    }
}

/// KML overlay layer
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KmlLayer {
    pub name: String,
    pub visible: bool,
    pub placemarks: Vec<KmlPlacemark>,
    pub color: [u8; 3],
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KmlPlacemark {
    pub name: String,
    pub description: Option<String>,
    pub geometry: KmlGeometry,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum KmlGeometry {
    Point {
        lat: f64,
        lon: f64,
        alt: Option<f64>,
    },
    LineString {
        points: Vec<(f64, f64, Option<f64>)>,
    },
    Polygon {
        outer: Vec<(f64, f64, Option<f64>)>,
    },
}

impl KmlLayer {
    /// Load from a KML file
    pub fn from_file(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        Self::parse_kml(&content, path)
    }

    fn parse_kml(content: &str, path: &Path) -> Result<Self> {
        use quick_xml::events::Event;
        use quick_xml::Reader;

        let mut reader = Reader::from_str(content);
        reader.config_mut().trim_text(true);

        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Untitled")
            .to_string();

        let mut placemarks = Vec::new();
        let mut current_placemark: Option<KmlPlacemark> = None;
        let mut in_placemark = false;
        let mut current_tag = String::new();
        let mut in_coordinates = false;

        let mut buf = Vec::new();

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref e)) => {
                    let tag_name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                    current_tag = tag_name.clone();

                    if tag_name == "Placemark" {
                        in_placemark = true;
                        current_placemark = Some(KmlPlacemark {
                            name: String::new(),
                            description: None,
                            geometry: KmlGeometry::Point {
                                lat: 0.0,
                                lon: 0.0,
                                alt: None,
                            },
                        });
                    } else if tag_name == "coordinates" {
                        in_coordinates = true;
                    }
                }
                Ok(Event::End(ref e)) => {
                    let tag_name = String::from_utf8_lossy(e.name().as_ref()).to_string();

                    if tag_name == "Placemark" {
                        if let Some(pm) = current_placemark.take() {
                            placemarks.push(pm);
                        }
                        in_placemark = false;
                    } else if tag_name == "coordinates" {
                        in_coordinates = false;
                    }
                }
                Ok(Event::Text(e)) => {
                    if in_placemark {
                        let text = e.unescape().unwrap_or_default().to_string();

                        if let Some(ref mut pm) = current_placemark {
                            match current_tag.as_str() {
                                "name" => pm.name = text.clone(),
                                "description" => pm.description = Some(text.clone()),
                                _ => {}
                            }

                            if in_coordinates {
                                // Parse coordinates: lon,lat,alt lon,lat,alt ...
                                let coords: Vec<(f64, f64, Option<f64>)> = text
                                    .split_whitespace()
                                    .filter_map(|coord_str| {
                                        let parts: Vec<&str> = coord_str.split(',').collect();
                                        if parts.len() >= 2 {
                                            let lon = parts[0].parse().ok()?;
                                            let lat = parts[1].parse().ok()?;
                                            let alt = parts.get(2).and_then(|s| s.parse().ok());
                                            Some((lat, lon, alt))
                                        } else {
                                            None
                                        }
                                    })
                                    .collect();

                                if coords.len() == 1 {
                                    pm.geometry = KmlGeometry::Point {
                                        lat: coords[0].0,
                                        lon: coords[0].1,
                                        alt: coords[0].2,
                                    };
                                } else if coords.len() > 1 {
                                    // Could be LineString or Polygon
                                    pm.geometry = KmlGeometry::LineString { points: coords };
                                }
                            }
                        }
                    }
                }
                Ok(Event::Eof) => break,
                Err(e) => return Err(anyhow!("KML parse error: {}", e)),
                _ => {}
            }
            buf.clear();
        }

        Ok(Self {
            name,
            visible: true,
            placemarks,
            color: [0, 255, 255], // Cyan
        })
    }
}

/// Offline map manager
#[derive(Default)]
pub struct OfflineMapManager {
    pub mbtiles_sources: Vec<(PathBuf, MBTilesSource)>,
    pub tile_cache: Option<TileCache>,
    pub geojson_layers: Vec<GeoJsonLayer>,
    pub kml_layers: Vec<KmlLayer>,
    pub cache_enabled: bool,
    pub cache_path: PathBuf,
}

impl OfflineMapManager {
    pub fn new() -> Self {
        let cache_path = std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join("cache")
            .join("tiles.db");

        Self {
            mbtiles_sources: vec![],
            tile_cache: None,
            geojson_layers: vec![],
            kml_layers: vec![],
            cache_enabled: true,
            cache_path,
        }
    }

    /// Initialize the tile cache
    pub fn init_cache(&mut self, max_size_mb: usize) -> Result<()> {
        if let Some(parent) = self.cache_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        self.tile_cache = Some(TileCache::open(&self.cache_path, max_size_mb)?);
        Ok(())
    }

    /// Add an MBTiles file
    pub fn add_mbtiles(&mut self, path: PathBuf) -> Result<()> {
        let source = MBTilesSource::open(&path)?;
        self.mbtiles_sources.push((path, source));
        Ok(())
    }

    /// Add a GeoJSON layer
    pub fn add_geojson(&mut self, path: PathBuf) -> Result<()> {
        let layer = GeoJsonLayer::from_file(&path)?;
        self.geojson_layers.push(layer);
        Ok(())
    }

    /// Add a KML layer
    pub fn add_kml(&mut self, path: PathBuf) -> Result<()> {
        let layer = KmlLayer::from_file(&path)?;
        self.kml_layers.push(layer);
        Ok(())
    }

    /// Get cache statistics
    pub fn cache_stats(&self) -> Option<CacheStats> {
        self.tile_cache.as_ref().and_then(|c| c.stats().ok())
    }

    /// Clear the tile cache
    pub fn clear_cache(&self) -> Result<()> {
        if let Some(cache) = &self.tile_cache {
            cache.clear()?;
        }
        Ok(())
    }
}

/// Render GeoJSON and KML overlays on the map
pub fn render_overlays(
    ui: &mut egui::Ui,
    projector: &walkers::Projector,
    geojson_layers: &[GeoJsonLayer],
    kml_layers: &[KmlLayer],
) {
    let painter = ui.painter();

    // Render GeoJSON layers
    for layer in geojson_layers {
        if !layer.visible {
            continue;
        }

        let base_color = egui::Color32::from_rgb(layer.color[0], layer.color[1], layer.color[2]);

        for feature in &layer.features {
            match feature {
                GeoFeature::Point {
                    lat,
                    lon,
                    properties,
                } => {
                    let geo = walkers::lat_lon(*lat, *lon);
                    let screen = projector.project(geo);
                    let pos = egui::pos2(screen.x, screen.y);

                    painter.circle_filled(pos, 6.0, base_color);
                    painter.circle_stroke(pos, 6.0, egui::Stroke::new(1.5, egui::Color32::WHITE));

                    if let Some(name) = properties.get("name") {
                        painter.text(
                            pos + egui::vec2(10.0, 0.0),
                            egui::Align2::LEFT_CENTER,
                            name,
                            egui::FontId::proportional(10.0),
                            egui::Color32::WHITE,
                        );
                    }
                }
                GeoFeature::LineString { points, .. } => {
                    if points.len() >= 2 {
                        let screen_points: Vec<egui::Pos2> = points
                            .iter()
                            .map(|(lat, lon)| {
                                let geo = walkers::lat_lon(*lat, *lon);
                                let s = projector.project(geo);
                                egui::pos2(s.x, s.y)
                            })
                            .collect();

                        for i in 1..screen_points.len() {
                            painter.line_segment(
                                [screen_points[i - 1], screen_points[i]],
                                egui::Stroke::new(2.0, base_color),
                            );
                        }
                    }
                }
                GeoFeature::Polygon { rings, .. } => {
                    for ring in rings {
                        if ring.len() >= 3 {
                            let screen_points: Vec<egui::Pos2> = ring
                                .iter()
                                .map(|(lat, lon)| {
                                    let geo = walkers::lat_lon(*lat, *lon);
                                    let s = projector.project(geo);
                                    egui::pos2(s.x, s.y)
                                })
                                .collect();

                            painter.add(egui::Shape::convex_polygon(
                                screen_points,
                                base_color.linear_multiply(0.3),
                                egui::Stroke::new(2.0, base_color),
                            ));
                        }
                    }
                }
            }
        }
    }

    // Render KML layers
    for layer in kml_layers {
        if !layer.visible {
            continue;
        }

        let base_color = egui::Color32::from_rgb(layer.color[0], layer.color[1], layer.color[2]);

        for placemark in &layer.placemarks {
            match &placemark.geometry {
                KmlGeometry::Point { lat, lon, .. } => {
                    let geo = walkers::lat_lon(*lat, *lon);
                    let screen = projector.project(geo);
                    let pos = egui::pos2(screen.x, screen.y);

                    painter.circle_filled(pos, 6.0, base_color);
                    painter.circle_stroke(pos, 6.0, egui::Stroke::new(1.5, egui::Color32::WHITE));

                    if !placemark.name.is_empty() {
                        painter.text(
                            pos + egui::vec2(10.0, 0.0),
                            egui::Align2::LEFT_CENTER,
                            &placemark.name,
                            egui::FontId::proportional(10.0),
                            egui::Color32::WHITE,
                        );
                    }
                }
                KmlGeometry::LineString { points } => {
                    if points.len() >= 2 {
                        let screen_points: Vec<egui::Pos2> = points
                            .iter()
                            .map(|(lat, lon, _)| {
                                let geo = walkers::lat_lon(*lat, *lon);
                                let s = projector.project(geo);
                                egui::pos2(s.x, s.y)
                            })
                            .collect();

                        for i in 1..screen_points.len() {
                            painter.line_segment(
                                [screen_points[i - 1], screen_points[i]],
                                egui::Stroke::new(2.0, base_color),
                            );
                        }
                    }
                }
                KmlGeometry::Polygon { outer } => {
                    if outer.len() >= 3 {
                        let screen_points: Vec<egui::Pos2> = outer
                            .iter()
                            .map(|(lat, lon, _)| {
                                let geo = walkers::lat_lon(*lat, *lon);
                                let s = projector.project(geo);
                                egui::pos2(s.x, s.y)
                            })
                            .collect();

                        painter.add(egui::Shape::convex_polygon(
                            screen_points,
                            base_color.linear_multiply(0.3),
                            egui::Stroke::new(2.0, base_color),
                        ));
                    }
                }
            }
        }
    }
}
