//! End-to-end integration tests for complete message flow

mod common;

use common::{
    extract_uid_from_cot, generate_cot_message, generate_unique_uid, init_test_tracing,
    TestEnvironment,
};
use omnitak_pool::{InboundMessage, PoolMessage};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[tokio::test]
async fn test_e2e_single_message_flow() {
    init_test_tracing();

    let env = TestEnvironment::new().await;

    // Add two connections
    let conn_a = env.add_connection("conn-a", 5).await.unwrap();
    let conn_b = env.add_connection("conn-b", 5).await.unwrap();

    // Send message from connection A through aggregator
    let uid = generate_unique_uid();
    let cot = generate_cot_message(&uid);

    let inbound_msg = InboundMessage {
        data: cot.clone(),
        source: conn_a.clone(),
        timestamp: Instant::now(),
    };

    env.aggregator
        .sender()
        .send_async(inbound_msg)
        .await
        .unwrap();

    // Wait for processing through aggregator -> distributor -> pool
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Connection A should NOT receive the message (source filtering)
    let conn_a_ref = env.pool.get_connection(&conn_a).unwrap();
    let a_result =
        tokio::time::timeout(Duration::from_millis(100), conn_a_ref.rx.recv_async()).await;

    assert!(
        a_result.is_err(),
        "Connection A should not receive its own message"
    );

    // Connection B SHOULD receive the message
    let conn_b_ref = env.pool.get_connection(&conn_b).unwrap();
    let b_result =
        tokio::time::timeout(Duration::from_millis(100), conn_b_ref.rx.recv_async()).await;

    assert!(
        b_result.is_ok(),
        "Connection B should receive message from A"
    );

    match b_result.unwrap().unwrap() {
        PoolMessage::Cot(data) => {
            let received_uid = extract_uid_from_cot(&data);
            assert_eq!(received_uid, Some(uid));
        }
        _ => panic!("Expected CoT message"),
    }

    env.shutdown().await;
}

#[tokio::test]
async fn test_e2e_no_message_loops() {
    init_test_tracing();

    let env = TestEnvironment::new().await;

    let conn_id = env.add_connection("loop-test", 5).await.unwrap();

    // Send message with this connection as source
    let uid = generate_unique_uid();
    let cot = generate_cot_message(&uid);

    let msg = InboundMessage {
        data: cot,
        source: conn_id.clone(),
        timestamp: Instant::now(),
    };

    env.aggregator.sender().send_async(msg).await.unwrap();

    // Wait for processing
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Connection should NOT receive its own message
    let conn = env.pool.get_connection(&conn_id).unwrap();
    let result = tokio::time::timeout(Duration::from_millis(100), conn.rx.recv_async()).await;

    assert!(result.is_err(), "No message loop should occur");

    env.shutdown().await;
}

#[tokio::test]
async fn test_e2e_multiple_senders_receivers() {
    init_test_tracing();

    let env = Arc::new(TestEnvironment::new().await);

    // Add 5 connections
    let conn_ids: Vec<String> = (1..=5)
        .map(|i| format!("conn-{}", i))
        .collect::<Vec<_>>()
        .into_iter()
        .map(|id| env.pool.add_connection(id.clone(), format!("Connection {}", id), format!("localhost:{}", 9000 + id.chars().last().unwrap().to_digit(10).unwrap()), 5))
        .collect::<Vec<_>>()
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .await
        .unwrap();

    // Each connection sends a unique message
    for (i, sender_id) in conn_ids.iter().enumerate() {
        let uid = format!("uid-from-{}", i);
        let cot = generate_cot_message(&uid);

        let msg = InboundMessage {
            data: cot,
            source: sender_id.clone(),
            timestamp: Instant::now(),
        };

        env.aggregator.sender().send_async(msg).await.unwrap();
    }

    // Wait for all messages to be processed
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Each connection should receive 4 messages (all except its own)
    for (i, receiver_id) in conn_ids.iter().enumerate() {
        let conn = env.pool.get_connection(receiver_id).unwrap();
        let mut received_count = 0;
        let mut received_uids = Vec::new();

        // Try to receive messages
        for _ in 0..10 {
            match tokio::time::timeout(Duration::from_millis(50), conn.rx.recv_async()).await {
                Ok(Ok(PoolMessage::Cot(data))) => {
                    received_count += 1;
                    if let Some(uid) = extract_uid_from_cot(&data) {
                        received_uids.push(uid);
                    }
                }
                _ => break,
            }
        }

        assert_eq!(
            received_count, 4,
            "Connection {} should receive 4 messages, got {}",
            i, received_count
        );

        // Should not receive its own message
        let own_uid = format!("uid-from-{}", i);
        assert!(
            !received_uids.contains(&own_uid),
            "Connection {} received its own message",
            i
        );
    }

    let env = Arc::try_unwrap(env).unwrap();
    env.shutdown().await;
}

#[tokio::test]
async fn test_e2e_deduplication_across_flow() {
    init_test_tracing();

    let env = TestEnvironment::new().await;

    // Add connections
    let conn_a = env.add_connection("conn-a", 5).await.unwrap();
    let conn_b = env.add_connection("conn-b", 5).await.unwrap();

    // Send same message twice from different sources
    let uid = generate_unique_uid();
    let cot = generate_cot_message(&uid);

    let msg1 = InboundMessage {
        data: cot.clone(),
        source: conn_a.clone(),
        timestamp: Instant::now(),
    };

    let msg2 = InboundMessage {
        data: cot,
        source: conn_b.clone(),
        timestamp: Instant::now(),
    };

    env.aggregator.sender().send_async(msg1).await.unwrap();
    env.aggregator.sender().send_async(msg2).await.unwrap();

    // Wait for processing
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Check aggregator metrics - only 1 unique message
    let agg_metrics = env.aggregator.metrics();
    let unique = agg_metrics
        .unique_messages
        .load(std::sync::atomic::Ordering::Relaxed);
    let dupes = agg_metrics
        .duplicate_messages
        .load(std::sync::atomic::Ordering::Relaxed);

    assert_eq!(unique, 1, "Should have 1 unique message");
    assert_eq!(dupes, 1, "Should have 1 duplicate message");

    // Each connection should receive at most 1 message (from the other source)
    for conn_id in [conn_a, conn_b] {
        let conn = env.pool.get_connection(&conn_id).unwrap();
        let mut count = 0;

        for _ in 0..3 {
            if tokio::time::timeout(Duration::from_millis(50), conn.rx.recv_async())
                .await
                .is_ok()
            {
                count += 1;
            }
        }

        assert!(
            count <= 1,
            "Connection should receive at most 1 message, got {}",
            count
        );
    }

    env.shutdown().await;
}

#[tokio::test]
async fn test_e2e_high_throughput() {
    init_test_tracing();

    let env = Arc::new(TestEnvironment::new().await);

    // Add 3 connections
    for i in 1..=3 {
        env.add_connection(&format!("conn-{}", i), 5)
            .await
            .unwrap();
    }

    // Send 1000 unique messages rapidly
    let send_count = 1000;

    for _ in 0..send_count {
        let uid = generate_unique_uid();
        let cot = generate_cot_message(&uid);

        let msg = InboundMessage {
            data: cot,
            source: "external-source".to_string(),
            timestamp: Instant::now(),
        };

        env.aggregator.sender().send_async(msg).await.unwrap();
    }

    // Wait for processing
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Check metrics
    let agg_metrics = env.aggregator.metrics();
    let received = agg_metrics
        .messages_received
        .load(std::sync::atomic::Ordering::Relaxed);
    let unique = agg_metrics
        .unique_messages
        .load(std::sync::atomic::Ordering::Relaxed);

    let dist_metrics = env.distributor.metrics();
    let dist_sent = dist_metrics
        .messages_sent
        .load(std::sync::atomic::Ordering::Relaxed);

    assert!(
        received >= 950,
        "Should receive most messages, got {}",
        received
    );
    assert!(unique >= 950, "Should have most unique, got {}", unique);
    assert!(
        dist_sent >= 2800,
        "Should distribute to 3 connections (~3000), got {}",
        dist_sent
    );

    let env = Arc::try_unwrap(env).unwrap();
    env.shutdown().await;
}

#[tokio::test]
async fn test_e2e_connection_add_remove_during_flow() {
    init_test_tracing();

    let env = Arc::new(TestEnvironment::new().await);

    // Add initial connection
    let conn1 = env.add_connection("conn-1", 5).await.unwrap();

    // Send some messages
    for i in 0..10 {
        let uid = format!("uid-{}", i);
        let cot = generate_cot_message(&uid);

        let msg = InboundMessage {
            data: cot,
            source: "external".to_string(),
            timestamp: Instant::now(),
        };

        env.aggregator.sender().send_async(msg).await.unwrap();

        // Add another connection mid-stream
        if i == 5 {
            let _ = env.add_connection("conn-2", 5).await;
        }
    }

    // Wait for processing
    tokio::time::sleep(Duration::from_millis(500)).await;

    // conn-1 should have received all 10 messages
    let conn1_ref = env.pool.get_connection(&conn1).unwrap();
    let mut conn1_count = 0;

    for _ in 0..15 {
        if tokio::time::timeout(Duration::from_millis(50), conn1_ref.rx.recv_async())
            .await
            .is_ok()
        {
            conn1_count += 1;
        }
    }

    assert!(
        conn1_count >= 9,
        "Connection 1 should receive most messages, got {}",
        conn1_count
    );

    // conn-2 should have received messages sent after it was added
    if let Some(conn2_ref) = env.pool.get_connection(&"conn-2".to_string()) {
        let mut conn2_count = 0;

        for _ in 0..10 {
            if tokio::time::timeout(Duration::from_millis(50), conn2_ref.rx.recv_async())
                .await
                .is_ok()
            {
                conn2_count += 1;
            }
        }

        assert!(
            conn2_count >= 4 && conn2_count <= 5,
            "Connection 2 should receive messages sent after joining, got {}",
            conn2_count
        );
    }

    let env = Arc::try_unwrap(env).unwrap();
    env.shutdown().await;
}

#[tokio::test]
async fn test_e2e_graceful_shutdown_with_active_flow() {
    init_test_tracing();

    let env = Arc::new(TestEnvironment::new().await);

    // Add connections
    for i in 1..=5 {
        env.add_connection(&format!("conn-{}", i), 5)
            .await
            .unwrap();
    }

    // Start sending messages in background
    let env_clone = Arc::clone(&env);
    let send_task = tokio::spawn(async move {
        for _ in 0..100 {
            let uid = generate_unique_uid();
            let cot = generate_cot_message(&uid);

            let msg = InboundMessage {
                data: cot,
                source: "sender".to_string(),
                timestamp: Instant::now(),
            };

            let _ = env_clone.aggregator.sender().send_async(msg).await;
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });

    // Let it run for a bit
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Initiate shutdown while messages are flowing
    let env = Arc::try_unwrap(env).unwrap_or_else(|arc| {
        // If Arc can't be unwrapped, just get a reference
        Arc::try_unwrap(arc).expect("Failed to unwrap Arc")
    });

    env.shutdown().await;

    // Verify shutdown completed
    assert_eq!(env.pool.connection_count(), 0);

    // Cancel the send task
    send_task.abort();
}

#[tokio::test]
async fn test_e2e_message_ordering() {
    init_test_tracing();

    let env = TestEnvironment::new().await;

    let receiver_id = env.add_connection("receiver", 5).await.unwrap();

    // Send messages in sequence
    let uids: Vec<String> = (0..10).map(|i| format!("seq-uid-{}", i)).collect();

    for uid in &uids {
        let cot = generate_cot_message(uid);

        let msg = InboundMessage {
            data: cot,
            source: "sender".to_string(),
            timestamp: Instant::now(),
        };

        env.aggregator.sender().send_async(msg).await.unwrap();

        // Small delay to maintain order
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // Wait for processing
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Receive and verify order
    let conn = env.pool.get_connection(&receiver_id).unwrap();
    let mut received_uids = Vec::new();

    for _ in 0..10 {
        match tokio::time::timeout(Duration::from_millis(100), conn.rx.recv_async()).await {
            Ok(Ok(PoolMessage::Cot(data))) => {
                if let Some(uid) = extract_uid_from_cot(&data) {
                    received_uids.push(uid);
                }
            }
            _ => break,
        }
    }

    // All messages should be received
    assert_eq!(received_uids.len(), 10, "Should receive all 10 messages");

    // Messages should be in order (or close to it due to async processing)
    for (i, uid) in received_uids.iter().enumerate() {
        assert!(
            uid.contains(&i.to_string()),
            "Message ordering issue at position {}",
            i
        );
    }

    env.shutdown().await;
}

#[tokio::test]
async fn test_e2e_mixed_message_types() {
    init_test_tracing();

    let env = TestEnvironment::new().await;

    let conn_id = env.add_connection("mixed-receiver", 5).await.unwrap();

    // Send CoT messages
    for i in 0..5 {
        let uid = format!("cot-{}", i);
        let cot = generate_cot_message(&uid);

        let msg = InboundMessage {
            data: cot,
            source: "sender".to_string(),
            timestamp: Instant::now(),
        };

        env.aggregator.sender().send_async(msg).await.unwrap();
    }

    // Send some ping messages directly through pool
    for _ in 0..3 {
        env.pool
            .send_to_connection(&conn_id, PoolMessage::Ping)
            .await
            .unwrap();
    }

    // Wait for processing
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Receive messages
    let conn = env.pool.get_connection(&conn_id).unwrap();
    let mut cot_count = 0;
    let mut ping_count = 0;

    for _ in 0..10 {
        match tokio::time::timeout(Duration::from_millis(50), conn.rx.recv_async()).await {
            Ok(Ok(PoolMessage::Cot(_))) => cot_count += 1,
            Ok(Ok(PoolMessage::Ping)) => ping_count += 1,
            _ => break,
        }
    }

    assert!(cot_count >= 4, "Should receive CoT messages, got {}", cot_count);
    assert_eq!(ping_count, 3, "Should receive 3 ping messages");

    env.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_e2e_stress_many_connections() {
    init_test_tracing();

    let env = Arc::new(TestEnvironment::new().await);

    // Add 20 connections
    for i in 1..=20 {
        env.add_connection(&format!("conn-{}", i), 5)
            .await
            .unwrap();
    }

    // Each connection sends 50 messages
    let mut handles = vec![];

    for sender_id in 1..=20 {
        let env_clone = Arc::clone(&env);
        let handle = tokio::spawn(async move {
            for msg_id in 0..50 {
                let uid = format!("sender-{}-msg-{}", sender_id, msg_id);
                let cot = generate_cot_message(&uid);

                let msg = InboundMessage {
                    data: cot,
                    source: format!("conn-{}", sender_id),
                    timestamp: Instant::now(),
                };

                env_clone
                    .aggregator
                    .sender()
                    .send_async(msg)
                    .await
                    .unwrap();

                tokio::time::sleep(Duration::from_micros(100)).await;
            }
        });

        handles.push(handle);
    }

    // Wait for all senders
    for handle in handles {
        handle.await.unwrap();
    }

    // Wait for processing
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Check metrics
    let agg_metrics = env.aggregator.metrics();
    let unique = agg_metrics
        .unique_messages
        .load(std::sync::atomic::Ordering::Relaxed);

    let dist_metrics = env.distributor.metrics();
    let dist_sent = dist_metrics
        .messages_sent
        .load(std::sync::atomic::Ordering::Relaxed);

    assert!(
        unique >= 900,
        "Should have most of 1000 unique messages, got {}",
        unique
    );
    assert!(
        dist_sent >= 15000,
        "Should distribute to many connections (~19000), got {}",
        dist_sent
    );

    let env = Arc::try_unwrap(env).unwrap();
    env.shutdown().await;
}
