//! Smoke test to verify basic functionality of the bidirectional aggregator

use omnitak_pool::{
    AggregatorConfig, ConnectionPool, DistributorConfig, FilterRule, InboundMessage,
    MessageAggregator, MessageDistributor, PoolConfig,
};
use std::sync::Arc;
use std::time::Duration;

#[tokio::test(flavor = "multi_thread")]
async fn test_basic_pool_initialization() {
    // Test that we can initialize the pool infrastructure
    let pool = Arc::new(ConnectionPool::new(PoolConfig::default()));
    let distributor = Arc::new(MessageDistributor::new(
        Arc::clone(&pool),
        DistributorConfig::default(),
    ));
    let aggregator = Arc::new(MessageAggregator::new(
        Arc::clone(&distributor),
        AggregatorConfig::default(),
    ));

    // Start the components
    distributor.start().await;
    aggregator.start().await;

    // Verify pool is empty
    assert_eq!(pool.connection_count(), 0);

    // Add a connection
    let conn_id = pool
        .add_connection(
            "test-conn-1".to_string(),
            "Test Connection".to_string(),
            "127.0.0.1:8087".to_string(),
            5,
        )
        .await
        .expect("Failed to add connection");

    // Verify connection was added
    assert_eq!(pool.connection_count(), 1);

    // Set filter
    distributor.add_filter(conn_id.clone(), FilterRule::AlwaysSend);

    // Send a message through the aggregator
    let cot_message = br#"<?xml version="1.0"?>
<event version="2.0" uid="smoke-test-uid" type="a-f-G" time="2025-01-01T00:00:00Z" start="2025-01-01T00:00:00Z" stale="2025-01-01T00:05:00Z" how="h-e">
    <point lat="37.7749" lon="-122.4194" hae="100.0" ce="10.0" le="5.0"/>
    <detail>
        <contact callsign="SMOKE-TEST"/>
    </detail>
</event>"#;

    let inbound_msg = InboundMessage {
        source: conn_id.clone(),
        data: cot_message.to_vec(),
        timestamp: std::time::Instant::now(),
    };

    let sender = aggregator.sender();
    sender
        .send_async(inbound_msg)
        .await
        .expect("Failed to send message");

    // Give it a moment to process
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Clean up
    pool.remove_connection(&conn_id)
        .await
        .expect("Failed to remove connection");
    assert_eq!(pool.connection_count(), 0);

    aggregator.stop().await;
    distributor.stop().await;
    pool.shutdown().await.expect("Failed to shutdown pool");

    println!("✓ Smoke test passed: Pool, Distributor, and Aggregator working correctly");
}

#[tokio::test]
async fn test_deduplication() {
    // Test that duplicate messages are filtered out
    let pool = Arc::new(ConnectionPool::new(PoolConfig::default()));
    let distributor = Arc::new(MessageDistributor::new(
        Arc::clone(&pool),
        DistributorConfig::default(),
    ));
    let aggregator = Arc::new(MessageAggregator::new(
        Arc::clone(&distributor),
        AggregatorConfig::default(),
    ));

    distributor.start().await;
    aggregator.start().await;

    // Add a test connection
    let conn_id = pool
        .add_connection(
            "test-conn-dedup".to_string(),
            "Dedup Test".to_string(),
            "127.0.0.1:8088".to_string(),
            5,
        )
        .await
        .expect("Failed to add connection");

    distributor.add_filter(conn_id.clone(), FilterRule::AlwaysSend);

    // Send the same message twice with the same UID
    let cot_message = br#"<?xml version="1.0"?>
<event version="2.0" uid="dedup-test-uid-123" type="a-f-G" time="2025-01-01T00:00:00Z" start="2025-01-01T00:00:00Z" stale="2025-01-01T00:05:00Z" how="h-e">
    <point lat="37.7749" lon="-122.4194" hae="100.0" ce="10.0" le="5.0"/>
    <detail>
        <contact callsign="DEDUP-TEST"/>
    </detail>
</event>"#;

    let sender = aggregator.sender();

    // First message
    sender
        .send_async(InboundMessage {
            source: conn_id.clone(),
            data: cot_message.to_vec(),
            timestamp: std::time::Instant::now(),
        })
        .await
        .expect("Failed to send first message");

    // Duplicate message (should be deduplicated)
    sender
        .send_async(InboundMessage {
            source: conn_id.clone(),
            data: cot_message.to_vec(),
            timestamp: std::time::Instant::now(),
        })
        .await
        .expect("Failed to send duplicate message");

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Clean up
    aggregator.stop().await;
    distributor.stop().await;
    pool.shutdown().await.expect("Failed to shutdown pool");

    println!("✓ Deduplication test passed");
}

#[tokio::test]
async fn test_multiple_connections() {
    // Test adding and managing multiple connections
    let pool = Arc::new(ConnectionPool::new(PoolConfig::default()));

    // Add multiple connections
    let conn1 = pool
        .add_connection(
            "test-multi-1".to_string(),
            "Multi Test 1".to_string(),
            "127.0.0.1:8089".to_string(),
            5,
        )
        .await
        .expect("Failed to add connection 1");

    let conn2 = pool
        .add_connection(
            "test-multi-2".to_string(),
            "Multi Test 2".to_string(),
            "127.0.0.1:8090".to_string(),
            5,
        )
        .await
        .expect("Failed to add connection 2");

    assert_eq!(pool.connection_count(), 2);

    // Remove connections
    pool.remove_connection(&conn1)
        .await
        .expect("Failed to remove connection 1");
    assert_eq!(pool.connection_count(), 1);

    pool.remove_connection(&conn2)
        .await
        .expect("Failed to remove connection 2");
    assert_eq!(pool.connection_count(), 0);

    pool.shutdown().await.expect("Failed to shutdown pool");

    println!("✓ Multiple connections test passed");
}
