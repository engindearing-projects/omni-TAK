//! Configuration tests for pool, aggregator, and distributor

mod common;

use common::{init_test_tracing, TestEnvironment};
use omnitak_pool::{
    AggregatorConfig, DistributionStrategy, DistributorConfig, PoolConfig,
};
use std::time::Duration;

#[test]
fn test_pool_config_default() {
    let config = PoolConfig::default();

    assert_eq!(config.max_connections, 10_000);
    assert_eq!(config.channel_capacity, 1000);
    assert_eq!(config.health_check_interval, Duration::from_secs(30));
    assert_eq!(config.inactive_timeout, Duration::from_secs(300));
    assert!(config.auto_reconnect);
}

#[test]
fn test_pool_config_custom() {
    let config = PoolConfig {
        max_connections: 100,
        channel_capacity: 500,
        health_check_interval: Duration::from_secs(10),
        inactive_timeout: Duration::from_secs(60),
        auto_reconnect: false,
    };

    assert_eq!(config.max_connections, 100);
    assert_eq!(config.channel_capacity, 500);
    assert_eq!(config.health_check_interval, Duration::from_secs(10));
    assert_eq!(config.inactive_timeout, Duration::from_secs(60));
    assert!(!config.auto_reconnect);
}

#[test]
fn test_distributor_config_default() {
    let config = DistributorConfig::default();

    assert_eq!(config.channel_capacity, 10_000);
    assert_eq!(config.strategy, DistributionStrategy::DropOnFull);
    assert_eq!(config.max_workers, 16);
    assert_eq!(config.batch_size, 100);
    assert_eq!(config.flush_interval, Duration::from_millis(10));
}

#[test]
fn test_distributor_config_custom() {
    let config = DistributorConfig {
        channel_capacity: 5000,
        strategy: DistributionStrategy::BlockOnFull,
        max_workers: 8,
        batch_size: 50,
        flush_interval: Duration::from_millis(20),
    };

    assert_eq!(config.channel_capacity, 5000);
    assert_eq!(config.strategy, DistributionStrategy::BlockOnFull);
    assert_eq!(config.max_workers, 8);
    assert_eq!(config.batch_size, 50);
    assert_eq!(config.flush_interval, Duration::from_millis(20));
}

#[test]
fn test_distributor_strategy_timeout() {
    let strategy = DistributionStrategy::TryForTimeout(Duration::from_millis(100));

    match strategy {
        DistributionStrategy::TryForTimeout(timeout) => {
            assert_eq!(timeout, Duration::from_millis(100));
        }
        _ => panic!("Expected TryForTimeout strategy"),
    }
}

#[test]
fn test_aggregator_config_default() {
    let config = AggregatorConfig::default();

    assert_eq!(config.dedup_window, Duration::from_secs(60));
    assert_eq!(config.max_cache_entries, 100_000);
    assert_eq!(config.cleanup_interval, Duration::from_secs(10));
    assert_eq!(config.channel_capacity, 10_000);
    assert_eq!(config.worker_count, 4);
}

#[test]
fn test_aggregator_config_custom() {
    let config = AggregatorConfig {
        dedup_window: Duration::from_secs(30),
        max_cache_entries: 50_000,
        cleanup_interval: Duration::from_secs(5),
        channel_capacity: 5000,
        worker_count: 8,
    };

    assert_eq!(config.dedup_window, Duration::from_secs(30));
    assert_eq!(config.max_cache_entries, 50_000);
    assert_eq!(config.cleanup_interval, Duration::from_secs(5));
    assert_eq!(config.channel_capacity, 5000);
    assert_eq!(config.worker_count, 8);
}

#[tokio::test]
async fn test_pool_respects_max_connections() {
    init_test_tracing();

    let config = PoolConfig {
        max_connections: 5,
        ..Default::default()
    };

    let env = TestEnvironment::with_config(
        config,
        DistributorConfig::default(),
        AggregatorConfig::default(),
    )
    .await;

    // Add max connections
    for i in 1..=5 {
        env.add_connection(&format!("conn-{}", i), 5)
            .await
            .unwrap();
    }

    assert_eq!(env.pool.connection_count(), 5);

    // Try to exceed limit
    let result = env.add_connection("conn-6", 5).await;
    assert!(result.is_err(), "Should not exceed max_connections");

    env.shutdown().await;
}

#[tokio::test]
async fn test_pool_channel_capacity_enforced() {
    init_test_tracing();

    let config = PoolConfig {
        channel_capacity: 5, // Very small capacity
        ..Default::default()
    };

    let env = TestEnvironment::with_config(
        config,
        DistributorConfig::default(),
        AggregatorConfig::default(),
    )
    .await;

    let conn_id = env.add_connection("small-channel", 5).await.unwrap();
    let connection = env.pool.get_connection(&conn_id).unwrap();

    // Fill the channel
    for i in 0..5 {
        connection
            .tx
            .send_async(omnitak_pool::PoolMessage::Ping)
            .await
            .expect(&format!("Should send message {}", i));
    }

    // Next send should timeout (channel full)
    let result = tokio::time::timeout(
        Duration::from_millis(100),
        connection.tx.send_async(omnitak_pool::PoolMessage::Ping),
    )
    .await;

    assert!(result.is_err(), "Channel should be full");

    env.shutdown().await;
}

#[tokio::test]
async fn test_aggregator_dedup_window_respected() {
    init_test_tracing();

    let config = AggregatorConfig {
        dedup_window: Duration::from_millis(100),
        cleanup_interval: Duration::from_millis(50),
        ..Default::default()
    };

    let env = TestEnvironment::with_config(
        PoolConfig::default(),
        DistributorConfig::default(),
        config,
    )
    .await;

    let _conn = env.add_connection("test", 5).await.unwrap();

    // Send message
    let uid = "test-uid";
    let cot = common::generate_cot_message(uid);

    let msg1 = omnitak_pool::InboundMessage {
        data: cot.clone(),
        source: "source-1".to_string(),
        timestamp: std::time::Instant::now(),
    };

    env.aggregator.sender().send_async(msg1).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Send same UID again - should be deduplicated
    let msg2 = omnitak_pool::InboundMessage {
        data: cot.clone(),
        source: "source-2".to_string(),
        timestamp: std::time::Instant::now(),
    };

    env.aggregator.sender().send_async(msg2).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let metrics = env.aggregator.metrics();
    let dupes = metrics
        .duplicate_messages
        .load(std::sync::atomic::Ordering::Relaxed);
    assert_eq!(dupes, 1, "Should have 1 duplicate within window");

    // Wait for window to expire
    tokio::time::sleep(Duration::from_millis(150)).await;

    // Send again - should NOT be deduplicated
    let msg3 = omnitak_pool::InboundMessage {
        data: cot,
        source: "source-3".to_string(),
        timestamp: std::time::Instant::now(),
    };

    env.aggregator.sender().send_async(msg3).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    let unique = metrics
        .unique_messages
        .load(std::sync::atomic::Ordering::Relaxed);
    assert_eq!(unique, 2, "Should have 2 unique messages after window expiry");

    env.shutdown().await;
}

#[tokio::test]
async fn test_aggregator_cache_size_limit() {
    init_test_tracing();

    let config = AggregatorConfig {
        max_cache_entries: 10,
        dedup_window: Duration::from_secs(300), // Long window
        cleanup_interval: Duration::from_millis(100),
        ..Default::default()
    };

    let env = TestEnvironment::with_config(
        PoolConfig::default(),
        DistributorConfig::default(),
        config,
    )
    .await;

    let _conn = env.add_connection("test", 5).await.unwrap();

    // Send 20 unique messages (more than cache size)
    for _ in 0..20 {
        let uid = common::generate_unique_uid();
        let cot = common::generate_cot_message(&uid);

        let msg = omnitak_pool::InboundMessage {
            data: cot,
            source: "source".to_string(),
            timestamp: std::time::Instant::now(),
        };

        env.aggregator.sender().send_async(msg).await.unwrap();
    }

    tokio::time::sleep(Duration::from_millis(300)).await;

    // Cache should not exceed max size
    let (cache_size, _) = env.aggregator.cache_stats();
    assert!(
        cache_size <= 10,
        "Cache size {} should not exceed limit 10",
        cache_size
    );

    env.shutdown().await;
}

#[tokio::test]
async fn test_distributor_batch_size() {
    init_test_tracing();

    let config = DistributorConfig {
        batch_size: 5,
        flush_interval: Duration::from_millis(50),
        ..Default::default()
    };

    let env = TestEnvironment::with_config(
        PoolConfig::default(),
        config,
        AggregatorConfig::default(),
    )
    .await;

    let _conn = env.add_connection("test", 5).await.unwrap();

    // Send 10 messages (2 batches)
    for _ in 0..10 {
        let uid = common::generate_unique_uid();
        let cot = common::generate_cot_message(&uid);

        let msg = omnitak_pool::DistributionMessage {
            data: cot,
            source: None,
            timestamp: std::time::Instant::now(),
        };

        env.distributor.sender().send_async(msg).await.unwrap();
    }

    tokio::time::sleep(Duration::from_millis(300)).await;

    // Should have processed at least 2 batches
    let metrics = env.distributor.metrics();
    let batches = metrics
        .batches_processed
        .load(std::sync::atomic::Ordering::Relaxed);

    assert!(batches >= 2, "Should have processed at least 2 batches");

    env.shutdown().await;
}

#[tokio::test]
async fn test_distributor_strategy_drop_on_full() {
    init_test_tracing();

    let config = DistributorConfig {
        strategy: DistributionStrategy::DropOnFull,
        ..Default::default()
    };

    let pool_config = PoolConfig {
        channel_capacity: 2, // Small buffer
        ..Default::default()
    };

    let env = TestEnvironment::with_config(pool_config, config, AggregatorConfig::default()).await;

    let _conn = env.add_connection("small-buffer", 5).await.unwrap();

    // Flood with messages
    for _ in 0..20 {
        let uid = common::generate_unique_uid();
        let cot = common::generate_cot_message(&uid);

        let msg = omnitak_pool::DistributionMessage {
            data: cot,
            source: None,
            timestamp: std::time::Instant::now(),
        };

        env.distributor.sender().send_async(msg).await.unwrap();
    }

    tokio::time::sleep(Duration::from_millis(300)).await;

    // Some messages should be dropped
    let metrics = env.distributor.metrics();
    let dropped = metrics
        .messages_dropped
        .load(std::sync::atomic::Ordering::Relaxed);

    assert!(dropped > 0, "Some messages should be dropped with DropOnFull strategy");

    env.shutdown().await;
}

#[tokio::test]
async fn test_aggregator_worker_count() {
    init_test_tracing();

    let config = AggregatorConfig {
        worker_count: 8,
        channel_capacity: 10000,
        ..Default::default()
    };

    let env = TestEnvironment::with_config(
        PoolConfig::default(),
        DistributorConfig::default(),
        config,
    )
    .await;

    let _conn = env.add_connection("test", 5).await.unwrap();

    // Send many messages quickly
    for _ in 0..1000 {
        let uid = common::generate_unique_uid();
        let cot = common::generate_cot_message(&uid);

        let msg = omnitak_pool::InboundMessage {
            data: cot,
            source: "source".to_string(),
            timestamp: std::time::Instant::now(),
        };

        env.aggregator.sender().send_async(msg).await.unwrap();
    }

    tokio::time::sleep(Duration::from_millis(500)).await;

    // All workers should process messages efficiently
    let metrics = env.aggregator.metrics();
    let processed = metrics
        .messages_received
        .load(std::sync::atomic::Ordering::Relaxed);

    assert!(
        processed >= 900,
        "Should process most messages with multiple workers"
    );

    env.shutdown().await;
}

#[tokio::test]
async fn test_distributor_max_workers() {
    init_test_tracing();

    let config = DistributorConfig {
        max_workers: 16,
        channel_capacity: 20000,
        ..Default::default()
    };

    let env = TestEnvironment::with_config(
        PoolConfig::default(),
        config,
        AggregatorConfig::default(),
    )
    .await;

    // Add multiple connections
    for i in 1..=10 {
        env.add_connection(&format!("conn-{}", i), 5)
            .await
            .unwrap();
    }

    // Send many messages
    for _ in 0..1000 {
        let uid = common::generate_unique_uid();
        let cot = common::generate_cot_message(&uid);

        let msg = omnitak_pool::DistributionMessage {
            data: cot,
            source: None,
            timestamp: std::time::Instant::now(),
        };

        env.distributor.sender().send_async(msg).await.unwrap();
    }

    tokio::time::sleep(Duration::from_secs(1)).await;

    // Check distribution efficiency
    let metrics = env.distributor.metrics();
    let sent = metrics
        .messages_sent
        .load(std::sync::atomic::Ordering::Relaxed);

    assert!(
        sent >= 9000,
        "Should distribute efficiently with multiple workers"
    );

    env.shutdown().await;
}

#[test]
fn test_config_cloning() {
    let pool_config = PoolConfig {
        max_connections: 123,
        ..Default::default()
    };
    let cloned = pool_config.clone();
    assert_eq!(cloned.max_connections, 123);

    let dist_config = DistributorConfig {
        max_workers: 42,
        ..Default::default()
    };
    let cloned = dist_config.clone();
    assert_eq!(cloned.max_workers, 42);

    let agg_config = AggregatorConfig {
        worker_count: 16,
        ..Default::default()
    };
    let cloned = agg_config.clone();
    assert_eq!(cloned.worker_count, 16);
}
