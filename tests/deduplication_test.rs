//! Integration tests for message deduplication

mod common;

use common::{generate_cot_message, generate_unique_uid, init_test_tracing, TestEnvironment};
use omnitak_pool::{AggregatorConfig, DistributorConfig, InboundMessage, PoolConfig};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[tokio::test]
async fn test_dedup_same_uid_twice() {
    init_test_tracing();

    let env = TestEnvironment::new().await;

    // Add a connection
    let _conn_id = env.add_connection("conn-1", 5).await.unwrap();

    // Send message with same UID twice
    let uid = generate_unique_uid();
    let cot1 = generate_cot_message(&uid);
    let cot2 = generate_cot_message(&uid);

    let msg1 = InboundMessage {
        data: cot1,
        source: "source-1".to_string(),
        timestamp: Instant::now(),
    };

    let msg2 = InboundMessage {
        data: cot2,
        source: "source-2".to_string(),
        timestamp: Instant::now(),
    };

    // Send both messages
    env.aggregator.sender().send_async(msg1).await.unwrap();
    env.aggregator.sender().send_async(msg2).await.unwrap();

    // Wait for processing
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Check metrics - should have 1 unique and 1 duplicate
    let metrics = env.aggregator.metrics();
    let unique = metrics.unique_messages.load(std::sync::atomic::Ordering::Relaxed);
    let dupes = metrics.duplicate_messages.load(std::sync::atomic::Ordering::Relaxed);

    assert_eq!(unique, 1, "Expected 1 unique message");
    assert_eq!(dupes, 1, "Expected 1 duplicate message");

    env.shutdown().await;
}

#[tokio::test]
async fn test_dedup_different_uids() {
    init_test_tracing();

    let env = TestEnvironment::new().await;
    let _conn_id = env.add_connection("conn-1", 5).await.unwrap();

    // Send messages with different UIDs
    for _ in 0..10 {
        let uid = generate_unique_uid();
        let cot = generate_cot_message(&uid);

        let msg = InboundMessage {
            data: cot,
            source: "source-1".to_string(),
            timestamp: Instant::now(),
        };

        env.aggregator.sender().send_async(msg).await.unwrap();
    }

    // Wait for processing
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Check metrics - all should be unique
    let metrics = env.aggregator.metrics();
    let unique = metrics.unique_messages.load(std::sync::atomic::Ordering::Relaxed);
    let dupes = metrics.duplicate_messages.load(std::sync::atomic::Ordering::Relaxed);

    assert_eq!(unique, 10, "Expected 10 unique messages");
    assert_eq!(dupes, 0, "Expected 0 duplicate messages");

    env.shutdown().await;
}

#[tokio::test]
async fn test_dedup_window_expiry() {
    init_test_tracing();

    // Create env with short dedup window
    let env = TestEnvironment::with_config(
        PoolConfig::default(),
        DistributorConfig::default(),
        AggregatorConfig {
            dedup_window: Duration::from_millis(500),
            cleanup_interval: Duration::from_millis(100),
            ..Default::default()
        },
    )
    .await;

    let _conn_id = env.add_connection("conn-1", 5).await.unwrap();

    let uid = generate_unique_uid();
    let cot1 = generate_cot_message(&uid);

    // Send first message
    let msg1 = InboundMessage {
        data: cot1.clone(),
        source: "source-1".to_string(),
        timestamp: Instant::now(),
    };

    env.aggregator.sender().send_async(msg1).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Wait for dedup window to expire
    tokio::time::sleep(Duration::from_millis(600)).await;

    // Send same UID again - should be treated as new
    let msg2 = InboundMessage {
        data: cot1,
        source: "source-2".to_string(),
        timestamp: Instant::now(),
    };

    env.aggregator.sender().send_async(msg2).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Check metrics - both should be unique
    let metrics = env.aggregator.metrics();
    let unique = metrics.unique_messages.load(std::sync::atomic::Ordering::Relaxed);
    let dupes = metrics.duplicate_messages.load(std::sync::atomic::Ordering::Relaxed);

    assert_eq!(
        unique, 2,
        "Expected 2 unique messages after window expiry"
    );
    assert_eq!(dupes, 0, "Expected 0 duplicates after window expiry");

    env.shutdown().await;
}

#[tokio::test]
async fn test_dedup_multiple_sources_same_uid() {
    init_test_tracing();

    let env = TestEnvironment::new().await;
    let _conn_id = env.add_connection("conn-1", 5).await.unwrap();

    let uid = generate_unique_uid();

    // Send same UID from 5 different sources
    for i in 1..=5 {
        let cot = generate_cot_message(&uid);
        let msg = InboundMessage {
            data: cot,
            source: format!("source-{}", i),
            timestamp: Instant::now(),
        };

        env.aggregator.sender().send_async(msg).await.unwrap();
    }

    // Wait for processing
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Check metrics - 1 unique, 4 duplicates
    let metrics = env.aggregator.metrics();
    let unique = metrics.unique_messages.load(std::sync::atomic::Ordering::Relaxed);
    let dupes = metrics.duplicate_messages.load(std::sync::atomic::Ordering::Relaxed);

    assert_eq!(unique, 1, "Expected 1 unique message");
    assert_eq!(dupes, 4, "Expected 4 duplicate messages");

    env.shutdown().await;
}

#[tokio::test]
async fn test_dedup_high_volume() {
    init_test_tracing();

    let env = TestEnvironment::with_config(
        PoolConfig::default(),
        DistributorConfig::default(),
        AggregatorConfig {
            worker_count: 8,
            channel_capacity: 10000,
            ..Default::default()
        },
    )
    .await;

    let _conn_id = env.add_connection("conn-1", 5).await.unwrap();

    // Send 1000 messages: 500 unique, 500 duplicates
    let uids: Vec<String> = (0..500).map(|_| generate_unique_uid()).collect();

    for uid in &uids {
        // Send each UID twice
        for _ in 0..2 {
            let cot = generate_cot_message(uid);
            let msg = InboundMessage {
                data: cot,
                source: "high-volume-source".to_string(),
                timestamp: Instant::now(),
            };

            env.aggregator.sender().send_async(msg).await.unwrap();
        }
    }

    // Wait for processing
    tokio::time::sleep(Duration::from_millis(1000)).await;

    // Check metrics
    let metrics = env.aggregator.metrics();
    let unique = metrics.unique_messages.load(std::sync::atomic::Ordering::Relaxed);
    let dupes = metrics.duplicate_messages.load(std::sync::atomic::Ordering::Relaxed);

    assert!(
        unique >= 450,
        "Expected at least 450 unique messages, got {}",
        unique
    );
    assert!(
        dupes >= 450,
        "Expected at least 450 duplicate messages, got {}",
        dupes
    );

    env.shutdown().await;
}

#[tokio::test]
async fn test_dedup_no_uid_messages() {
    init_test_tracing();

    let env = TestEnvironment::new().await;
    let _conn_id = env.add_connection("conn-1", 5).await.unwrap();

    // Send messages without UIDs
    for i in 0..5 {
        let msg_data = format!("<event type=\"a-f-G\">Message {}</event>", i).into_bytes();

        let msg = InboundMessage {
            data: msg_data,
            source: "source-1".to_string(),
            timestamp: Instant::now(),
        };

        env.aggregator.sender().send_async(msg).await.unwrap();
    }

    // Wait for processing
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Check metrics - all should be forwarded without deduplication
    let metrics = env.aggregator.metrics();
    let no_uid = metrics.messages_no_uid.load(std::sync::atomic::Ordering::Relaxed);

    assert_eq!(no_uid, 5, "Expected 5 messages without UID");

    env.shutdown().await;
}

#[tokio::test]
async fn test_dedup_cache_cleanup() {
    init_test_tracing();

    let env = TestEnvironment::with_config(
        PoolConfig::default(),
        DistributorConfig::default(),
        AggregatorConfig {
            dedup_window: Duration::from_millis(200),
            cleanup_interval: Duration::from_millis(100),
            max_cache_entries: 100,
            ..Default::default()
        },
    )
    .await;

    let _conn_id = env.add_connection("conn-1", 5).await.unwrap();

    // Send 50 unique messages
    for _ in 0..50 {
        let uid = generate_unique_uid();
        let cot = generate_cot_message(&uid);
        let msg = InboundMessage {
            data: cot,
            source: "source-1".to_string(),
            timestamp: Instant::now(),
        };

        env.aggregator.sender().send_async(msg).await.unwrap();
    }

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Check initial cache size
    let (entries_before, _) = env.aggregator.cache_stats();
    assert!(
        entries_before >= 40,
        "Expected at least 40 cache entries, got {}",
        entries_before
    );

    // Wait for cleanup to run multiple times
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Cache should be cleaned
    let (entries_after, _) = env.aggregator.cache_stats();
    assert!(
        entries_after < entries_before,
        "Expected cache to be cleaned, before: {}, after: {}",
        entries_before,
        entries_after
    );

    env.shutdown().await;
}

#[tokio::test]
async fn test_dedup_lru_eviction() {
    init_test_tracing();

    let env = TestEnvironment::with_config(
        PoolConfig::default(),
        DistributorConfig::default(),
        AggregatorConfig {
            dedup_window: Duration::from_secs(300),
            max_cache_entries: 10,
            cleanup_interval: Duration::from_millis(100),
            ..Default::default()
        },
    )
    .await;

    let _conn_id = env.add_connection("conn-1", 5).await.unwrap();

    // Send 20 unique messages (more than cache size)
    for _ in 0..20 {
        let uid = generate_unique_uid();
        let cot = generate_cot_message(&uid);
        let msg = InboundMessage {
            data: cot,
            source: "source-1".to_string(),
            timestamp: Instant::now(),
        };

        env.aggregator.sender().send_async(msg).await.unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    tokio::time::sleep(Duration::from_millis(200)).await;

    // Cache should not exceed max size
    let (entries, _) = env.aggregator.cache_stats();
    assert!(
        entries <= 10,
        "Cache size {} exceeded max size 10",
        entries
    );

    env.shutdown().await;
}

#[tokio::test]
async fn test_dedup_ratio_calculation() {
    init_test_tracing();

    let env = TestEnvironment::new().await;
    let _conn_id = env.add_connection("conn-1", 5).await.unwrap();

    // Send 10 unique messages
    let uids: Vec<String> = (0..10).map(|_| generate_unique_uid()).collect();

    for uid in &uids {
        let cot = generate_cot_message(uid);
        let msg = InboundMessage {
            data: cot,
            source: "source-1".to_string(),
            timestamp: Instant::now(),
        };

        env.aggregator.sender().send_async(msg).await.unwrap();
    }

    // Send 5 duplicates
    for uid in uids.iter().take(5) {
        let cot = generate_cot_message(uid);
        let msg = InboundMessage {
            data: cot,
            source: "source-2".to_string(),
            timestamp: Instant::now(),
        };

        env.aggregator.sender().send_async(msg).await.unwrap();
    }

    // Wait for processing
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Check dedup ratio
    let metrics = env.aggregator.metrics();
    let dedup_ratio = metrics.get_dedup_ratio();

    // 5 duplicates out of 15 total = 0.333...
    assert!(
        (dedup_ratio - 0.33).abs() < 0.05,
        "Expected dedup ratio around 0.33, got {}",
        dedup_ratio
    );

    env.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_dedup_concurrent_messages() {
    init_test_tracing();

    let env = Arc::new(
        TestEnvironment::with_config(
            PoolConfig::default(),
            DistributorConfig::default(),
            AggregatorConfig {
                worker_count: 8,
                channel_capacity: 20000,
                ..Default::default()
            },
        )
        .await,
    );

    let _conn_id = env.add_connection("conn-1", 5).await.unwrap();

    // Create 100 UIDs
    let uids: Vec<String> = (0..100).map(|_| generate_unique_uid()).collect();
    let uids = Arc::new(uids);

    // Spawn 10 tasks, each sending the same 100 UIDs
    let mut handles = vec![];

    for task_id in 0..10 {
        let env_clone = Arc::clone(&env);
        let uids_clone = Arc::clone(&uids);

        let handle = tokio::spawn(async move {
            for uid in uids_clone.iter() {
                let cot = generate_cot_message(uid);
                let msg = InboundMessage {
                    data: cot,
                    source: format!("source-{}", task_id),
                    timestamp: Instant::now(),
                };

                env_clone
                    .aggregator
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

    // Check metrics - 100 unique, 900 duplicates
    let metrics = env.aggregator.metrics();
    let unique = metrics.unique_messages.load(std::sync::atomic::Ordering::Relaxed);
    let dupes = metrics.duplicate_messages.load(std::sync::atomic::Ordering::Relaxed);

    assert!(
        unique >= 95,
        "Expected at least 95 unique messages, got {}",
        unique
    );
    assert!(
        dupes >= 850,
        "Expected at least 850 duplicate messages, got {}",
        dupes
    );

    let env = Arc::try_unwrap(env).unwrap();
    env.shutdown().await;
}
