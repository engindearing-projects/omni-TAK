//! Integration tests for message distribution

mod common;

use common::{
    generate_cot_message, generate_cot_with_properties, generate_unique_uid, init_test_tracing,
    TestEnvironment,
};
use omnitak_pool::{
    DistributionMessage, DistributionStrategy, DistributorConfig, FilterRule, PoolConfig,
    PoolMessage,
};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[tokio::test]
async fn test_distribution_basic_fanout() {
    init_test_tracing();

    let env = TestEnvironment::new().await;

    // Add multiple connections
    let conn_ids: Vec<String> = (1..=5)
        .map(|i| {
            let id = format!("conn-{}", i);
            env.pool
                .add_connection(
                    id.clone(),
                    format!("Connection {}", i),
                    format!("localhost:808{}", i),
                    5,
                )
        })
        .collect::<Vec<_>>()
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .await
        .unwrap();

    // Send a message through distributor
    let uid = generate_unique_uid();
    let cot = generate_cot_message(&uid);

    let msg = DistributionMessage {
        data: cot,
        source: None,
        timestamp: Instant::now(),
    };

    env.distributor.sender().send_async(msg).await.unwrap();

    // Wait for distribution
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Verify all connections received the message
    for conn_id in &conn_ids {
        let conn = env.pool.get_connection(conn_id).unwrap();
        let received = tokio::time::timeout(Duration::from_millis(100), conn.rx.recv_async())
            .await
            .unwrap()
            .unwrap();

        match received {
            PoolMessage::Cot(_) => {}
            _ => panic!("Expected CoT message"),
        }
    }

    env.shutdown().await;
}

#[tokio::test]
async fn test_distribution_source_filtering() {
    init_test_tracing();

    let env = TestEnvironment::new().await;

    // Add connections
    let source_id = env.add_connection("source-conn", 5).await.unwrap();
    let dest1_id = env.add_connection("dest-1", 5).await.unwrap();
    let dest2_id = env.add_connection("dest-2", 5).await.unwrap();

    // Send message with source connection specified
    let uid = generate_unique_uid();
    let cot = generate_cot_message(&uid);

    let msg = DistributionMessage {
        data: cot,
        source: Some(source_id.clone()),
        timestamp: Instant::now(),
    };

    env.distributor.sender().send_async(msg).await.unwrap();

    // Wait for distribution
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Source connection should NOT receive the message (loop prevention)
    let source_conn = env.pool.get_connection(&source_id).unwrap();
    let source_result =
        tokio::time::timeout(Duration::from_millis(100), source_conn.rx.recv_async()).await;

    assert!(
        source_result.is_err(),
        "Source connection should not receive its own message"
    );

    // Destination connections SHOULD receive the message
    for dest_id in [dest1_id, dest2_id] {
        let dest_conn = env.pool.get_connection(&dest_id).unwrap();
        let result =
            tokio::time::timeout(Duration::from_millis(100), dest_conn.rx.recv_async()).await;

        assert!(
            result.is_ok(),
            "Destination connection should receive message"
        );
    }

    env.shutdown().await;
}

#[tokio::test]
async fn test_distribution_filter_always_send() {
    init_test_tracing();

    let env = TestEnvironment::new().await;

    let conn_id = env.add_connection("filtered-conn", 5).await.unwrap();

    // Set AlwaysSend filter
    env.distributor
        .set_filters(conn_id.clone(), vec![FilterRule::AlwaysSend]);

    // Send message
    let uid = generate_unique_uid();
    let cot = generate_cot_message(&uid);

    let msg = DistributionMessage {
        data: cot,
        source: None,
        timestamp: Instant::now(),
    };

    env.distributor.sender().send_async(msg).await.unwrap();

    // Wait for distribution
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Connection should receive message
    let conn = env.pool.get_connection(&conn_id).unwrap();
    let result = tokio::time::timeout(Duration::from_millis(100), conn.rx.recv_async()).await;

    assert!(result.is_ok(), "AlwaysSend filter should pass message");

    env.shutdown().await;
}

#[tokio::test]
async fn test_distribution_filter_never_send() {
    init_test_tracing();

    let env = TestEnvironment::new().await;

    let conn_id = env.add_connection("blocked-conn", 5).await.unwrap();

    // Set NeverSend filter
    env.distributor
        .set_filters(conn_id.clone(), vec![FilterRule::NeverSend]);

    // Send message
    let uid = generate_unique_uid();
    let cot = generate_cot_message(&uid);

    let msg = DistributionMessage {
        data: cot,
        source: None,
        timestamp: Instant::now(),
    };

    env.distributor.sender().send_async(msg).await.unwrap();

    // Wait for distribution
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Connection should NOT receive message
    let conn = env.pool.get_connection(&conn_id).unwrap();
    let result = tokio::time::timeout(Duration::from_millis(100), conn.rx.recv_async()).await;

    assert!(
        result.is_err(),
        "NeverSend filter should block message"
    );

    env.shutdown().await;
}

#[tokio::test]
async fn test_distribution_filter_by_type() {
    init_test_tracing();

    let env = TestEnvironment::new().await;

    let conn_id = env.add_connection("type-filtered", 5).await.unwrap();

    // Set filter for specific type
    env.distributor.set_filters(
        conn_id.clone(),
        vec![FilterRule::ByType(vec!["a-f-G".to_string()])],
    );

    // Send matching message
    let uid1 = generate_unique_uid();
    let cot1 = generate_cot_with_properties(&uid1, 37.7749, -122.4194, "a-f-G", "TEST1");

    let msg1 = DistributionMessage {
        data: cot1,
        source: None,
        timestamp: Instant::now(),
    };

    env.distributor.sender().send_async(msg1).await.unwrap();

    // Wait for distribution
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Should receive message
    let conn = env.pool.get_connection(&conn_id).unwrap();
    let result = tokio::time::timeout(Duration::from_millis(100), conn.rx.recv_async()).await;

    assert!(result.is_ok(), "Type filter should pass matching message");

    // Send non-matching message
    let uid2 = generate_unique_uid();
    let cot2 = generate_cot_with_properties(&uid2, 37.7749, -122.4194, "a-h-G", "TEST2");

    let msg2 = DistributionMessage {
        data: cot2,
        source: None,
        timestamp: Instant::now(),
    };

    env.distributor.sender().send_async(msg2).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Should NOT receive message
    let result = tokio::time::timeout(Duration::from_millis(100), conn.rx.recv_async()).await;

    assert!(
        result.is_err(),
        "Type filter should block non-matching message"
    );

    env.shutdown().await;
}

#[tokio::test]
async fn test_distribution_filter_by_callsign() {
    init_test_tracing();

    let env = TestEnvironment::new().await;

    let conn_id = env.add_connection("callsign-filtered", 5).await.unwrap();

    // Set filter for specific callsign pattern
    env.distributor.set_filters(
        conn_id.clone(),
        vec![FilterRule::ByCallsign("ALPHA".to_string())],
    );

    // Send matching message
    let uid1 = generate_unique_uid();
    let cot1 = generate_cot_with_properties(&uid1, 37.7749, -122.4194, "a-f-G", "ALPHA-1");

    let msg1 = DistributionMessage {
        data: cot1,
        source: None,
        timestamp: Instant::now(),
    };

    env.distributor.sender().send_async(msg1).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Should receive message
    let conn = env.pool.get_connection(&conn_id).unwrap();
    let result = tokio::time::timeout(Duration::from_millis(100), conn.rx.recv_async()).await;

    assert!(
        result.is_ok(),
        "Callsign filter should pass matching message"
    );

    // Send non-matching message
    let uid2 = generate_unique_uid();
    let cot2 = generate_cot_with_properties(&uid2, 37.7749, -122.4194, "a-f-G", "BRAVO-1");

    let msg2 = DistributionMessage {
        data: cot2,
        source: None,
        timestamp: Instant::now(),
    };

    env.distributor.sender().send_async(msg2).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Should NOT receive message
    let result = tokio::time::timeout(Duration::from_millis(100), conn.rx.recv_async()).await;

    assert!(
        result.is_err(),
        "Callsign filter should block non-matching message"
    );

    env.shutdown().await;
}

#[tokio::test]
async fn test_distribution_drop_on_full() {
    init_test_tracing();

    let env = TestEnvironment::with_config(
        PoolConfig {
            channel_capacity: 2, // Small capacity to test backpressure
            ..Default::default()
        },
        DistributorConfig {
            strategy: DistributionStrategy::DropOnFull,
            ..Default::default()
        },
        Default::default(),
    )
    .await;

    let conn_id = env.add_connection("small-buffer", 5).await.unwrap();

    // Fill the channel by sending multiple messages quickly
    for _ in 0..10 {
        let uid = generate_unique_uid();
        let cot = generate_cot_message(&uid);

        let msg = DistributionMessage {
            data: cot,
            source: None,
            timestamp: Instant::now(),
        };

        env.distributor.sender().send_async(msg).await.unwrap();
    }

    // Wait for distribution attempts
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Check that some messages were dropped
    let metrics = env.distributor.metrics();
    let dropped = metrics.messages_dropped.load(std::sync::atomic::Ordering::Relaxed);

    assert!(dropped > 0, "Expected some messages to be dropped");

    env.shutdown().await;
}

#[tokio::test]
async fn test_distribution_multiple_filters() {
    init_test_tracing();

    let env = TestEnvironment::new().await;

    let conn_id = env.add_connection("multi-filter", 5).await.unwrap();

    // Set multiple filters (OR logic - any filter matches)
    env.distributor.set_filters(
        conn_id.clone(),
        vec![
            FilterRule::ByType(vec!["a-f-G".to_string()]),
            FilterRule::ByCallsign("SPECIAL".to_string()),
        ],
    );

    // Send message matching first filter
    let uid1 = generate_unique_uid();
    let cot1 = generate_cot_with_properties(&uid1, 37.7749, -122.4194, "a-f-G", "NORMAL");

    let msg1 = DistributionMessage {
        data: cot1,
        source: None,
        timestamp: Instant::now(),
    };

    env.distributor.sender().send_async(msg1).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    let conn = env.pool.get_connection(&conn_id).unwrap();
    let result = tokio::time::timeout(Duration::from_millis(100), conn.rx.recv_async()).await;
    assert!(result.is_ok(), "Should receive message matching first filter");

    // Send message matching second filter
    let uid2 = generate_unique_uid();
    let cot2 = generate_cot_with_properties(&uid2, 37.7749, -122.4194, "a-h-G", "SPECIAL-1");

    let msg2 = DistributionMessage {
        data: cot2,
        source: None,
        timestamp: Instant::now(),
    };

    env.distributor.sender().send_async(msg2).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = tokio::time::timeout(Duration::from_millis(100), conn.rx.recv_async()).await;
    assert!(result.is_ok(), "Should receive message matching second filter");

    env.shutdown().await;
}

#[tokio::test]
async fn test_distribution_batch_processing() {
    init_test_tracing();

    let env = TestEnvironment::with_config(
        PoolConfig::default(),
        DistributorConfig {
            batch_size: 10,
            flush_interval: Duration::from_millis(50),
            ..Default::default()
        },
        Default::default(),
    )
    .await;

    let conn_id = env.add_connection("batch-test", 5).await.unwrap();

    // Send batch of messages
    for _ in 0..25 {
        let uid = generate_unique_uid();
        let cot = generate_cot_message(&uid);

        let msg = DistributionMessage {
            data: cot,
            source: None,
            timestamp: Instant::now(),
        };

        env.distributor.sender().send_async(msg).await.unwrap();
    }

    // Wait for processing
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Check metrics - should have processed multiple batches
    let metrics = env.distributor.metrics();
    let batches = metrics.batches_processed.load(std::sync::atomic::Ordering::Relaxed);

    assert!(batches >= 2, "Expected at least 2 batches, got {}", batches);

    env.shutdown().await;
}

#[tokio::test]
async fn test_distribution_latency_tracking() {
    init_test_tracing();

    let env = TestEnvironment::new().await;
    let conn_id = env.add_connection("latency-test", 5).await.unwrap();

    // Send messages
    for _ in 0..10 {
        let uid = generate_unique_uid();
        let cot = generate_cot_message(&uid);

        let msg = DistributionMessage {
            data: cot,
            source: None,
            timestamp: Instant::now(),
        };

        env.distributor.sender().send_async(msg).await.unwrap();
    }

    // Wait for processing
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Verify messages were sent
    let metrics = env.distributor.metrics();
    let sent = metrics.messages_sent.load(std::sync::atomic::Ordering::Relaxed);

    assert!(sent >= 10, "Expected at least 10 messages sent");

    env.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_distribution_concurrent_workers() {
    init_test_tracing();

    let env = Arc::new(
        TestEnvironment::with_config(
            PoolConfig::default(),
            DistributorConfig {
                max_workers: 8,
                channel_capacity: 10000,
                ..Default::default()
            },
            Default::default(),
        )
        .await,
    );

    // Add multiple connections
    for i in 1..=10 {
        env.add_connection(&format!("conn-{}", i), 5)
            .await
            .unwrap();
    }

    // Send many messages concurrently
    let mut handles = vec![];

    for _ in 0..10 {
        let env_clone = Arc::clone(&env);
        let handle = tokio::spawn(async move {
            for _ in 0..100 {
                let uid = generate_unique_uid();
                let cot = generate_cot_message(&uid);

                let msg = DistributionMessage {
                    data: cot,
                    source: None,
                    timestamp: Instant::now(),
                };

                env_clone
                    .distributor
                    .sender()
                    .send_async(msg)
                    .await
                    .unwrap();
            }
        });
        handles.push(handle);
    }

    // Wait for all tasks
    for handle in handles {
        handle.await.unwrap();
    }

    // Wait for processing
    tokio::time::sleep(Duration::from_secs(1)).await;

    // Check metrics
    let metrics = env.distributor.metrics();
    let received = metrics.messages_received.load(std::sync::atomic::Ordering::Relaxed);
    let sent = metrics.messages_sent.load(std::sync::atomic::Ordering::Relaxed);

    assert!(
        received >= 900,
        "Expected at least 900 messages received, got {}",
        received
    );
    assert!(
        sent >= 8000,
        "Expected at least 8000 messages sent (100 * 10 connections), got {}",
        sent
    );

    let env = Arc::try_unwrap(env).unwrap();
    env.shutdown().await;
}

#[tokio::test]
async fn test_distribution_custom_filter() {
    init_test_tracing();

    let env = TestEnvironment::new().await;
    let conn_id = env.add_connection("custom-filter", 5).await.unwrap();

    // Set custom filter that only passes messages containing "IMPORTANT"
    let custom_filter = FilterRule::Custom(Arc::new(|data: &[u8]| {
        String::from_utf8_lossy(data).contains("IMPORTANT")
    }));

    env.distributor.set_filters(conn_id.clone(), vec![custom_filter]);

    // Send important message
    let msg_data = b"<event uid=\"uid-1\" type=\"a-f-G\"><detail><remarks>IMPORTANT</remarks></detail></event>".to_vec();
    let msg1 = DistributionMessage {
        data: msg_data,
        source: None,
        timestamp: Instant::now(),
    };

    env.distributor.sender().send_async(msg1).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    let conn = env.pool.get_connection(&conn_id).unwrap();
    let result = tokio::time::timeout(Duration::from_millis(100), conn.rx.recv_async()).await;
    assert!(result.is_ok(), "Custom filter should pass important message");

    // Send normal message
    let msg_data2 = b"<event uid=\"uid-2\" type=\"a-f-G\"><detail><remarks>Normal</remarks></detail></event>".to_vec();
    let msg2 = DistributionMessage {
        data: msg_data2,
        source: None,
        timestamp: Instant::now(),
    };

    env.distributor.sender().send_async(msg2).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = tokio::time::timeout(Duration::from_millis(100), conn.rx.recv_async()).await;
    assert!(result.is_err(), "Custom filter should block normal message");

    env.shutdown().await;
}
