//! Basic filtering example
//!
//! Run with: cargo run --example basic_filtering

use omnitak_filter::affiliation::CotType;
use omnitak_filter::router::{Route, RouteTableBuilder};
use omnitak_filter::rules::{
    AffiliationFilter, CotMessage, FilterRule, GeoBoundingBoxFilter, TeamFilter,
};
use std::sync::Arc;

fn main() {
    println!("=== OmniTAK Filter Example ===\n");

    // Example 1: Parse CoT type
    println!("1. Parsing CoT Type:");
    let cot_types = vec![
        "a-f-G-E-V-C", // Friendly Ground Equipment
        "a-h-A-M-F",   // Hostile Air Military Fighter
        "a-n-G",       // Neutral Ground
    ];

    for cot_type in cot_types {
        let cot = CotType::parse(cot_type);
        println!(
            "  {} -> Affiliation: {:?}, Dimension: {:?}",
            cot_type, cot.affiliation, cot.dimension
        );
    }

    // Example 2: Create and test filters
    println!("\n2. Testing Filters:");

    let friendly_msg = CotMessage {
        cot_type: "a-f-G-E-V-C",
        uid: "FRIENDLY-001",
        callsign: Some("ALPHA-1"),
        group: Some("Blue Force"),
        team: Some("Alpha"),
        lat: 40.7128,
        lon: -74.0060,
        hae: Some(100.0),
    };

    let hostile_msg = CotMessage {
        cot_type: "a-h-A-M-F",
        uid: "HOSTILE-001",
        callsign: Some("BANDIT-1"),
        group: Some("Red Force"),
        team: Some("Unknown"),
        lat: 41.5,
        lon: -73.5,
        hae: Some(5000.0),
    };

    // Affiliation filter
    let aff_filter = AffiliationFilter::friendly_only();
    println!(
        "  Friendly filter on friendly msg: {:?}",
        aff_filter.evaluate(&friendly_msg)
    );
    println!(
        "  Friendly filter on hostile msg: {:?}",
        aff_filter.evaluate(&hostile_msg)
    );

    // Team filter
    let team_filter = TeamFilter::new(vec!["Alpha".to_string()]);
    println!(
        "  Team Alpha filter on friendly msg: {:?}",
        team_filter.evaluate(&friendly_msg)
    );
    println!(
        "  Team Alpha filter on hostile msg: {:?}",
        team_filter.evaluate(&hostile_msg)
    );

    // Geographic filter
    let geo_filter = GeoBoundingBoxFilter::new(40.0, 41.0, -75.0, -73.0);
    println!(
        "  NYC bbox filter on friendly msg (in bbox): {:?}",
        geo_filter.evaluate(&friendly_msg)
    );
    println!(
        "  NYC bbox filter on hostile msg (outside bbox): {:?}",
        geo_filter.evaluate(&hostile_msg)
    );

    // Example 3: Routing
    println!("\n3. Routing Messages:");

    let table = RouteTableBuilder::multicast()
        .add_route(Route::new(
            "friendly".to_string(),
            "Route friendly units".to_string(),
            Arc::new(AffiliationFilter::friendly_only()),
            vec!["blue-team-server".to_string()],
            100,
        ))
        .add_route(Route::new(
            "hostile".to_string(),
            "Route hostile units".to_string(),
            Arc::new(AffiliationFilter::hostile_only()),
            vec!["threat-tracker".to_string()],
            100,
        ))
        .add_route(Route::new(
            "nyc".to_string(),
            "Route NYC area".to_string(),
            Arc::new(GeoBoundingBoxFilter::new(40.0, 41.0, -75.0, -73.0)),
            vec!["nyc-regional-server".to_string()],
            50,
        ))
        .default_destination("default-server".to_string())
        .build();

    let result = table.route(&friendly_msg);
    println!("  Friendly message routed to: {:?}", result.destinations);
    println!("  Matched routes: {:?}", result.matched_routes);

    let result = table.route(&hostile_msg);
    println!("  Hostile message routed to: {:?}", result.destinations);
    println!("  Matched routes: {:?}", result.matched_routes);

    // Example 4: Performance metrics
    println!("\n4. Filter Statistics:");

    // Create a filter and evaluate multiple messages
    let filter = AffiliationFilter::friendly_only();
    let mut stats = omnitak_filter::rules::FilterStats::new();

    for i in 0..10 {
        let msg = CotMessage {
            cot_type: if i % 3 == 0 { "a-f-G" } else { "a-h-G" },
            uid: "TEST",
            callsign: None,
            group: None,
            team: None,
            lat: 0.0,
            lon: 0.0,
            hae: None,
        };
        let result = filter.evaluate(&msg);
        stats.record(result);
    }

    println!("  {}", stats);

    println!("\n=== Example Complete ===");
}
