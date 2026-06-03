//! Performance benchmarks for filter operations
//!
//! Run with: cargo bench --package omnitak-filter

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use omnitak_filter::affiliation::CotType;
use omnitak_filter::fast_path::{fast_extract_affiliation, fast_in_bbox, fast_is_friendly};
use omnitak_filter::router::{Route, RouteTable, RouteTableBuilder};
use omnitak_filter::rules::{
    AffiliationFilter, CotMessage, FilterRule, GeoBoundingBoxFilter, TeamFilter,
};
use std::sync::Arc;

fn create_test_message() -> CotMessage<'static> {
    CotMessage {
        cot_type: "a-f-G-E-V-C",
        uid: "BENCH-001",
        callsign: Some("ALPHA-1"),
        group: Some("Blue Force"),
        team: Some("Alpha"),
        lat: 40.7128,
        lon: -74.0060,
        hae: Some(100.0),
    }
}

fn bench_affiliation_parsing(c: &mut Criterion) {
    let mut group = c.benchmark_group("affiliation_parsing");

    let cot_types = vec![
        "a-f-G-E-V-C",         // Short
        "a-h-A-M-F-Q-H",       // Medium
        "a-f-G-E-V-C-U-I-M-N", // Long
    ];

    for cot_type in cot_types {
        group.bench_with_input(
            BenchmarkId::new("normal_parse", cot_type),
            &cot_type,
            |b, &cot_type| {
                b.iter(|| {
                    let cot = CotType::parse(black_box(cot_type));
                    black_box(cot.affiliation)
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("fast_extract", cot_type),
            &cot_type,
            |b, &cot_type| {
                b.iter(|| black_box(fast_extract_affiliation(black_box(cot_type))));
            },
        );
    }

    group.finish();
}

fn bench_affiliation_checks(c: &mut Criterion) {
    let mut group = c.benchmark_group("affiliation_checks");

    let cot_type = "a-f-G-E-V-C";

    group.bench_function("is_friendly", |b| {
        b.iter(|| black_box(fast_is_friendly(black_box(cot_type))));
    });

    group.bench_function("normal_is_friendly", |b| {
        b.iter(|| {
            let cot = CotType::parse(black_box(cot_type));
            black_box(cot.is_friendly())
        });
    });

    group.finish();
}

fn bench_filter_evaluation(c: &mut Criterion) {
    let mut group = c.benchmark_group("filter_evaluation");

    let msg = create_test_message();

    // Affiliation filter
    let aff_filter = AffiliationFilter::friendly_only();
    group.bench_function("affiliation_filter", |b| {
        b.iter(|| black_box(aff_filter.evaluate(black_box(&msg))));
    });

    // Geo bounding box filter
    let geo_filter = GeoBoundingBoxFilter::new(40.0, 41.0, -75.0, -73.0);
    group.bench_function("geo_bbox_filter", |b| {
        b.iter(|| black_box(geo_filter.evaluate(black_box(&msg))));
    });

    // Team filter
    let team_filter = TeamFilter::new(vec!["Alpha".to_string(), "Bravo".to_string()]);
    group.bench_function("team_filter", |b| {
        b.iter(|| black_box(team_filter.evaluate(black_box(&msg))));
    });

    group.finish();
}

fn bench_geo_bbox(c: &mut Criterion) {
    let mut group = c.benchmark_group("geo_bbox");

    let bbox = [40.0, 45.0, -75.0, -70.0];
    let lat = 42.0;
    let lon = -72.0;

    group.bench_function("fast_in_bbox", |b| {
        b.iter(|| {
            black_box(fast_in_bbox(
                black_box(lat),
                black_box(lon),
                black_box(&bbox),
            ))
        });
    });

    group.bench_function("normal_bbox_check", |b| {
        b.iter(|| {
            let lat = black_box(lat);
            let lon = black_box(lon);
            let bbox = black_box(&bbox);
            black_box(lat >= bbox[0] && lat <= bbox[1] && lon >= bbox[2] && lon <= bbox[3])
        });
    });

    group.finish();
}

fn bench_routing(c: &mut Criterion) {
    let mut group = c.benchmark_group("routing");

    let msg = create_test_message();

    // Create route table with multiple routes
    let table = RouteTableBuilder::multicast()
        .add_route(Route::new(
            "friendly".to_string(),
            "Friendly units".to_string(),
            Arc::new(AffiliationFilter::friendly_only()),
            vec!["dest1".to_string()],
            100,
        ))
        .add_route(Route::new(
            "nyc".to_string(),
            "NYC area".to_string(),
            Arc::new(GeoBoundingBoxFilter::new(40.0, 41.0, -75.0, -73.0)),
            vec!["dest2".to_string()],
            90,
        ))
        .add_route(Route::new(
            "team_alpha".to_string(),
            "Team Alpha".to_string(),
            Arc::new(TeamFilter::new(vec!["Alpha".to_string()])),
            vec!["dest3".to_string()],
            80,
        ))
        .build();

    group.bench_function("route_message_3_routes", |b| {
        b.iter(|| black_box(table.route(black_box(&msg))));
    });

    // Create table with more routes
    let large_table = RouteTable::multicast();
    for i in 0..10 {
        large_table.add_route(Route::new(
            format!("route_{}", i),
            format!("Route {}", i),
            Arc::new(AffiliationFilter::friendly_only()),
            vec![format!("dest_{}", i)],
            100 - i,
        ));
    }

    group.bench_function("route_message_10_routes", |b| {
        b.iter(|| black_box(large_table.route(black_box(&msg))));
    });

    group.finish();
}

fn bench_routing_strategies(c: &mut Criterion) {
    let mut group = c.benchmark_group("routing_strategies");

    let msg = create_test_message();

    // Multicast (evaluate all routes)
    let multicast_table = RouteTableBuilder::multicast()
        .add_route(Route::new(
            "route1".to_string(),
            "Route 1".to_string(),
            Arc::new(AffiliationFilter::friendly_only()),
            vec!["dest1".to_string()],
            100,
        ))
        .add_route(Route::new(
            "route2".to_string(),
            "Route 2".to_string(),
            Arc::new(GeoBoundingBoxFilter::new(40.0, 41.0, -75.0, -73.0)),
            vec!["dest2".to_string()],
            90,
        ))
        .build();

    group.bench_function("multicast", |b| {
        b.iter(|| black_box(multicast_table.route(black_box(&msg))));
    });

    // Unicast (first match only)
    let unicast_table = RouteTableBuilder::unicast()
        .add_route(Route::new(
            "route1".to_string(),
            "Route 1".to_string(),
            Arc::new(AffiliationFilter::friendly_only()),
            vec!["dest1".to_string()],
            100,
        ))
        .add_route(Route::new(
            "route2".to_string(),
            "Route 2".to_string(),
            Arc::new(GeoBoundingBoxFilter::new(40.0, 41.0, -75.0, -73.0)),
            vec!["dest2".to_string()],
            90,
        ))
        .build();

    group.bench_function("unicast", |b| {
        b.iter(|| black_box(unicast_table.route(black_box(&msg))));
    });

    group.finish();
}

fn bench_parallel_filtering(c: &mut Criterion) {
    let mut group = c.benchmark_group("parallel_filtering");

    let messages: Vec<_> = (0..100)
        .map(|i| CotMessage {
            cot_type: if i % 2 == 0 {
                "a-f-G-E-V-C"
            } else {
                "a-h-G-E-V-C"
            },
            uid: "TEST",
            callsign: Some("ALPHA"),
            group: Some("Blue"),
            team: Some("Alpha"),
            lat: 40.0 + (i as f64 * 0.01),
            lon: -74.0 + (i as f64 * 0.01),
            hae: Some(100.0),
        })
        .collect();

    let filter = AffiliationFilter::friendly_only();

    group.bench_function("sequential_100_msgs", |b| {
        b.iter(|| {
            for msg in &messages {
                black_box(filter.evaluate(black_box(msg)));
            }
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_affiliation_parsing,
    bench_affiliation_checks,
    bench_filter_evaluation,
    bench_geo_bbox,
    bench_routing,
    bench_routing_strategies,
    bench_parallel_filtering,
);
criterion_main!(benches);
