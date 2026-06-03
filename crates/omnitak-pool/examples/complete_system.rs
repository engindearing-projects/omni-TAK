//! Complete Connection Pool System Example
//!
//! This example demonstrates a full-featured connection pool setup with:
//! - Connection management
//! - Message distribution
//! - Deduplication
//! - Health monitoring
//! - Metrics collection
//! - Concurrency control
//!
//! Run with: cargo run --example complete_system

use omnitak_pool::{
    AggregatorConfig, ConcurrencyConfig, ConcurrencyLimiter, ConnectionPool, DistributionStrategy,
    DistributorConfig, FilterRule, HealthConfig, HealthMonitor, InboundMessage, MessageAggregator,
    MessageDistributor, MetricsConfig, MetricsRegistry, PoolConfig,
};
use std::sync::Arc;
use std::time::Duration;
use tokio::signal;
use tracing::{info, Level};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt().with_max_level(Level::INFO).init();

    info!("Starting OmniTAK Connection Pool System");

    // 1. Initialize metrics registry
    let metrics = Arc::new(MetricsRegistry::new(MetricsConfig {
        enabled: true,
        bind_address: "0.0.0.0:9090".parse().unwrap(),
        ..Default::default()
    }));

    metrics.init()?;
    metrics.start_server().await?;
    info!("Metrics server started on http://0.0.0.0:9090/metrics");

    // 2. Create connection pool
    let pool = Arc::new(ConnectionPool::new(PoolConfig {
        max_connections: 10_000,
        channel_capacity: 1000,
        health_check_interval: Duration::from_secs(30),
        inactive_timeout: Duration::from_secs(300),
        auto_reconnect: true,
    }));

    info!("Connection pool created with max {} connections", 10_000);

    // 3. Create concurrency limiter
    let limiter = Arc::new(ConcurrencyLimiter::new(ConcurrencyConfig {
        max_concurrent: 10_000,
        max_queue_size: 1_000,
        enable_rate_limit: true,
        rate_limit_ops_per_sec: 10_000,
        ..Default::default()
    }));

    limiter.start().await;
    info!("Concurrency limiter started");

    // 4. Create message distributor
    let distributor = Arc::new(MessageDistributor::new(
        Arc::clone(&pool),
        DistributorConfig {
            channel_capacity: 10_000,
            strategy: DistributionStrategy::DropOnFull,
            max_workers: 16,
            batch_size: 100,
            flush_interval: Duration::from_millis(10),
        },
    ));

    distributor.start().await;
    info!("Message distributor started with 16 workers");

    // 5. Create message aggregator
    let aggregator = Arc::new(MessageAggregator::new(
        Arc::clone(&distributor),
        AggregatorConfig {
            dedup_window: Duration::from_secs(60),
            max_cache_entries: 100_000,
            cleanup_interval: Duration::from_secs(10),
            channel_capacity: 10_000,
            worker_count: 4,
        },
    ));

    aggregator.start().await;
    info!("Message aggregator started with deduplication");

    // 6. Create health monitor
    let health_monitor = Arc::new(HealthMonitor::with_config(HealthConfig {
        check_interval: Duration::from_secs(30),
        check_timeout: Duration::from_secs(5),
        degraded_threshold: Duration::from_secs(60),
        unhealthy_threshold: Duration::from_secs(300),
        circuit_failure_threshold: 5,
        circuit_reset_timeout: Duration::from_secs(60),
        circuit_success_threshold: 2,
        auto_reconnect: true,
    }));

    health_monitor.start(Arc::clone(&pool));
    info!("Health monitor started");

    // 7. Add example connections
    info!("Adding connections to pool...");

    pool.add_connection(
        "primary-server".to_string(),
        "Primary TAK Server".to_string(),
        "192.168.1.100:8087".to_string(),
        10, // high priority
    )
    .await?;

    pool.add_connection(
        "secondary-server".to_string(),
        "Secondary TAK Server".to_string(),
        "192.168.1.101:8087".to_string(),
        5, // normal priority
    )
    .await?;

    pool.add_connection(
        "backup-server".to_string(),
        "Backup TAK Server".to_string(),
        "192.168.1.102:8087".to_string(),
        3, // low priority
    )
    .await?;

    info!("Added {} connections", pool.connection_count());

    // 8. Configure filters
    info!("Configuring message filters...");

    // Primary server gets friendly ground units
    distributor.add_filter(
        "primary-server".to_string(),
        FilterRule::ByType(vec!["a-f-G".to_string()]),
    );

    // Secondary server gets all friendly units
    distributor.add_filter(
        "secondary-server".to_string(),
        FilterRule::ByType(vec!["a-f".to_string()]),
    );

    // Backup server gets everything
    distributor.add_filter("backup-server".to_string(), FilterRule::AlwaysSend);

    // 9. Start statistics reporter
    let pool_clone = Arc::clone(&pool);
    let metrics_clone = Arc::clone(&metrics);
    let limiter_clone = Arc::clone(&limiter);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(10));

        loop {
            interval.tick().await;

            // Update gauge metrics
            metrics_clone.update_gauges(&pool_clone);

            // Print statistics
            let stats = pool_clone.stats();
            let snapshot = metrics_clone.snapshot();

            info!(
                "Pool Stats - Connections: {}/{}, Messages: {}/{}, Errors: {}, Throughput: {:.2}/s, Dedup: {:.2}%",
                stats.active_connections,
                stats.total_connections,
                stats.total_messages_received,
                stats.total_messages_sent,
                stats.total_errors,
                snapshot.distributor_throughput,
                snapshot.aggregator_dedup_ratio * 100.0
            );

            // Print limiter stats
            let limiter_stats = limiter_clone.stats();
            info!(
                "Concurrency - Active: {}, Queued: {}, Accepted: {}, Rejected: {}",
                limiter_stats.active_connections,
                limiter_stats.queued_requests,
                limiter_stats.total_accepted,
                limiter_stats.total_rejected
            );
        }
    });

    // 10. Simulate message traffic
    let aggregator_clone = Arc::clone(&aggregator);
    tokio::spawn(async move {
        let mut counter = 0u64;
        let mut interval = tokio::time::interval(Duration::from_millis(100));

        loop {
            interval.tick().await;

            // Generate test messages
            for i in 0..10 {
                counter += 1;
                let uid = format!("test-{}-{}", counter, i);

                let msg = InboundMessage {
                    data: format!(
                        r#"<event uid="{}" type="a-f-G" time="2024-01-01T00:00:00Z"><point lat="35.0" lon="-120.0" /></event>"#,
                        uid
                    )
                    .into_bytes(),
                    source: "test-generator".to_string(),
                    timestamp: std::time::Instant::now(),
                };

                if let Err(e) = aggregator_clone.sender().try_send(msg) {
                    tracing::warn!("Failed to submit message: {}", e);
                }
            }

            // Also send some duplicates
            if counter.is_multiple_of(10) {
                let duplicate_msg = InboundMessage {
                    data: format!(
                        r#"<event uid="duplicate-{}" type="a-f-G" time="2024-01-01T00:00:00Z"></event>"#,
                        counter / 10
                    )
                    .into_bytes(),
                    source: "test-generator-2".to_string(),
                    timestamp: std::time::Instant::now(),
                };

                let _ = aggregator_clone.sender().try_send(duplicate_msg);
            }
        }
    });

    info!("System is running. Press Ctrl+C to shutdown.");
    info!("View metrics at: http://localhost:9090/metrics");

    // 11. Wait for shutdown signal
    match signal::ctrl_c().await {
        Ok(()) => {
            info!("Shutdown signal received");
        }
        Err(err) => {
            tracing::error!("Unable to listen for shutdown signal: {}", err);
        }
    }

    // 12. Graceful shutdown
    info!("Initiating graceful shutdown...");

    info!("Stopping health monitor...");
    health_monitor.stop().await;

    info!("Stopping aggregator...");
    aggregator.stop().await;

    info!("Stopping distributor...");
    distributor.stop().await;

    info!("Stopping concurrency limiter...");
    limiter.stop().await;

    info!("Shutting down connection pool...");
    pool.shutdown().await?;

    info!("Shutdown complete");

    // Print final statistics
    let final_stats = pool.stats();
    info!("Final Statistics:");
    info!("  Total Connections: {}", final_stats.total_connections);
    info!("  Messages Sent: {}", final_stats.total_messages_sent);
    info!(
        "  Messages Received: {}",
        final_stats.total_messages_received
    );
    info!("  Total Errors: {}", final_stats.total_errors);

    let final_snapshot = metrics.snapshot();
    info!(
        "  Average Throughput: {:.2} msg/s",
        final_snapshot.distributor_throughput
    );
    info!(
        "  Deduplication Ratio: {:.2}%",
        final_snapshot.aggregator_dedup_ratio * 100.0
    );

    Ok(())
}
