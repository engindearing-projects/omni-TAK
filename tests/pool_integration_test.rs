//! Integration tests for ConnectionPool

mod common;

use common::{generate_cot_message, generate_unique_uid, init_test_tracing, wait_for_condition};
use omnitak_pool::{ConnectionPool, PoolConfig, PoolMessage};
use std::sync::Arc;
use std::time::Duration;

#[tokio::test]
async fn test_pool_add_remove_connection() {
    init_test_tracing();

    let pool = ConnectionPool::new(PoolConfig::default());

    let conn_id = pool
        .add_connection(
            "test-1".to_string(),
            "Test Connection".to_string(),
            "localhost:8087".to_string(),
            5,
        )
        .await
        .unwrap();

    assert_eq!(pool.connection_count(), 1);
    assert!(pool.get_connection(&conn_id).is_some());

    pool.remove_connection(&conn_id).await.unwrap();
    assert_eq!(pool.connection_count(), 0);
    assert!(pool.get_connection(&conn_id).is_none());
}

#[tokio::test]
async fn test_pool_connection_capacity() {
    init_test_tracing();

    let config = PoolConfig {
        max_connections: 2,
        ..Default::default()
    };
    let pool = ConnectionPool::new(config);

    // Add first connection
    pool.add_connection(
        "conn-1".to_string(),
        "Connection 1".to_string(),
        "localhost:8087".to_string(),
        5,
    )
    .await
    .unwrap();

    // Add second connection
    pool.add_connection(
        "conn-2".to_string(),
        "Connection 2".to_string(),
        "localhost:8088".to_string(),
        5,
    )
    .await
    .unwrap();

    assert_eq!(pool.connection_count(), 2);

    // Try to add third connection - should fail
    let result = pool
        .add_connection(
            "conn-3".to_string(),
            "Connection 3".to_string(),
            "localhost:8089".to_string(),
            5,
        )
        .await;

    assert!(result.is_err());
    assert_eq!(pool.connection_count(), 2);
}

#[tokio::test]
async fn test_pool_duplicate_connection_id() {
    init_test_tracing();

    let pool = ConnectionPool::new(PoolConfig::default());

    pool.add_connection(
        "duplicate-id".to_string(),
        "First".to_string(),
        "localhost:8087".to_string(),
        5,
    )
    .await
    .unwrap();

    // Try to add connection with same ID
    let result = pool
        .add_connection(
            "duplicate-id".to_string(),
            "Second".to_string(),
            "localhost:8088".to_string(),
            5,
        )
        .await;

    assert!(result.is_err());
    assert_eq!(pool.connection_count(), 1);
}

#[tokio::test]
async fn test_pool_send_to_connection() {
    init_test_tracing();

    let pool = Arc::new(ConnectionPool::new(PoolConfig::default()));

    let conn_id = pool
        .add_connection(
            "test-conn".to_string(),
            "Test Connection".to_string(),
            "localhost:8087".to_string(),
            5,
        )
        .await
        .unwrap();

    // Send a message
    let message = PoolMessage::Cot(b"test message".to_vec());
    pool.send_to_connection(&conn_id, message)
        .await
        .unwrap();

    // Verify message was sent
    let connection = pool.get_connection(&conn_id).unwrap();
    let stats = connection.state.messages_sent.load(std::sync::atomic::Ordering::Relaxed);
    assert_eq!(stats, 1);

    // Receive the message
    let received = tokio::time::timeout(Duration::from_secs(1), connection.rx.recv_async())
        .await
        .unwrap()
        .unwrap();

    match received {
        PoolMessage::Cot(data) => assert_eq!(data, b"test message"),
        _ => panic!("Unexpected message type"),
    }
}

#[tokio::test]
async fn test_pool_broadcast() {
    init_test_tracing();

    let pool = Arc::new(ConnectionPool::new(PoolConfig::default()));

    // Add multiple connections
    for i in 1..=5 {
        pool.add_connection(
            format!("conn-{}", i),
            format!("Connection {}", i),
            format!("localhost:808{}", i),
            5,
        )
        .await
        .unwrap();
    }

    assert_eq!(pool.connection_count(), 5);

    // Broadcast a message
    let message = PoolMessage::Ping;
    let sent_count = pool.broadcast(message).await;

    assert_eq!(sent_count, 5);

    // Verify all connections received the message
    for i in 1..=5 {
        let conn = pool.get_connection(&format!("conn-{}", i)).unwrap();
        let received = tokio::time::timeout(Duration::from_millis(500), conn.rx.recv_async())
            .await
            .unwrap()
            .unwrap();

        matches!(received, PoolMessage::Ping);
    }
}

#[tokio::test]
async fn test_pool_get_connections_by_priority() {
    init_test_tracing();

    let pool = ConnectionPool::new(PoolConfig::default());

    // Add connections with different priorities
    pool.add_connection(
        "low-pri".to_string(),
        "Low Priority".to_string(),
        "localhost:8087".to_string(),
        1,
    )
    .await
    .unwrap();

    pool.add_connection(
        "high-pri".to_string(),
        "High Priority".to_string(),
        "localhost:8088".to_string(),
        10,
    )
    .await
    .unwrap();

    pool.add_connection(
        "med-pri".to_string(),
        "Medium Priority".to_string(),
        "localhost:8089".to_string(),
        5,
    )
    .await
    .unwrap();

    let connections = pool.get_connections_by_priority();

    // Verify sorted by priority (descending)
    assert_eq!(connections[0].priority, 10);
    assert_eq!(connections[1].priority, 5);
    assert_eq!(connections[2].priority, 1);
}

#[tokio::test]
async fn test_pool_connection_channels() {
    init_test_tracing();

    let pool = Arc::new(ConnectionPool::new(PoolConfig::default()));

    let conn_id = pool
        .add_connection(
            "chan-test".to_string(),
            "Channel Test".to_string(),
            "localhost:8087".to_string(),
            5,
        )
        .await
        .unwrap();

    let connection = pool.get_connection(&conn_id).unwrap();

    // Send message through tx channel
    let cot_data = generate_cot_message(&generate_unique_uid());
    connection
        .tx
        .send_async(PoolMessage::Cot(cot_data.clone()))
        .await
        .unwrap();

    // Receive through rx channel (after connection handler processes it)
    let received = tokio::time::timeout(Duration::from_secs(1), connection.rx.recv_async())
        .await
        .unwrap()
        .unwrap();

    match received {
        PoolMessage::Cot(data) => assert_eq!(data, cot_data),
        _ => panic!("Unexpected message type"),
    }
}

#[tokio::test]
async fn test_pool_graceful_shutdown() {
    init_test_tracing();

    let pool = Arc::new(ConnectionPool::new(PoolConfig::default()));

    // Add several connections
    for i in 1..=10 {
        pool.add_connection(
            format!("conn-{}", i),
            format!("Connection {}", i),
            format!("localhost:808{}", i),
            5,
        )
        .await
        .unwrap();
    }

    assert_eq!(pool.connection_count(), 10);

    // Shutdown pool
    pool.shutdown().await.unwrap();

    // Verify all connections removed
    assert_eq!(pool.connection_count(), 0);

    // Verify all connections are inactive
    let stats = pool.stats();
    assert_eq!(stats.active_connections, 0);
}

#[tokio::test]
async fn test_pool_stats() {
    init_test_tracing();

    let pool = Arc::new(ConnectionPool::new(PoolConfig::default()));

    // Add connections
    for i in 1..=3 {
        pool.add_connection(
            format!("conn-{}", i),
            format!("Connection {}", i),
            format!("localhost:808{}", i),
            5,
        )
        .await
        .unwrap();
    }

    // Send some messages
    for i in 1..=3 {
        let conn_id = format!("conn-{}", i);
        pool.send_to_connection(&conn_id, PoolMessage::Ping)
            .await
            .unwrap();
    }

    tokio::time::sleep(Duration::from_millis(100)).await;

    let stats = pool.stats();
    assert_eq!(stats.total_connections, 3);
    assert_eq!(stats.active_connections, 3);
    assert_eq!(stats.total_messages_sent, 3);
}

#[tokio::test]
async fn test_pool_list_connections() {
    init_test_tracing();

    let pool = ConnectionPool::new(PoolConfig::default());

    let ids = vec!["conn-1", "conn-2", "conn-3"];

    for id in &ids {
        pool.add_connection(
            id.to_string(),
            format!("Connection {}", id),
            format!("localhost:808{}", id.chars().last().unwrap()),
            5,
        )
        .await
        .unwrap();
    }

    let connection_ids = pool.list_connections();
    assert_eq!(connection_ids.len(), 3);

    for id in ids {
        assert!(connection_ids.contains(&id.to_string()));
    }
}

#[tokio::test]
async fn test_pool_get_active_connections() {
    init_test_tracing();

    let pool = Arc::new(ConnectionPool::new(PoolConfig::default()));

    // Add connections
    for i in 1..=5 {
        pool.add_connection(
            format!("conn-{}", i),
            format!("Connection {}", i),
            format!("localhost:808{}", i),
            5,
        )
        .await
        .unwrap();
    }

    let active = pool.get_active_connections();
    assert_eq!(active.len(), 5);

    // Deactivate one connection
    if let Some(conn) = pool.get_connection(&"conn-3".to_string()) {
        conn.state.deactivate();
    }

    // Wait a bit for state to propagate
    tokio::time::sleep(Duration::from_millis(50)).await;

    let active = pool.get_active_connections();
    assert_eq!(active.len(), 4);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_pool_concurrent_operations() {
    init_test_tracing();

    let pool = Arc::new(ConnectionPool::new(PoolConfig {
        max_connections: 100,
        ..Default::default()
    }));

    // Spawn multiple tasks adding connections concurrently
    let mut handles = vec![];

    for i in 0..50 {
        let pool_clone = Arc::clone(&pool);
        let handle = tokio::spawn(async move {
            pool_clone
                .add_connection(
                    format!("concurrent-{}", i),
                    format!("Concurrent Connection {}", i),
                    format!("localhost:{}", 9000 + i),
                    5,
                )
                .await
        });
        handles.push(handle);
    }

    // Wait for all tasks
    for handle in handles {
        handle.await.unwrap().unwrap();
    }

    assert_eq!(pool.connection_count(), 50);

    // Now remove them concurrently
    let conn_ids: Vec<_> = pool.list_connections();
    let mut handles = vec![];

    for id in conn_ids {
        let pool_clone = Arc::clone(&pool);
        let handle = tokio::spawn(async move { pool_clone.remove_connection(&id).await });
        handles.push(handle);
    }

    for handle in handles {
        handle.await.unwrap().unwrap();
    }

    assert_eq!(pool.connection_count(), 0);
}

#[tokio::test]
async fn test_pool_metrics() {
    init_test_tracing();

    let pool = Arc::new(ConnectionPool::new(PoolConfig::default()));

    let metrics = pool.metrics();

    pool.add_connection(
        "metrics-test".to_string(),
        "Metrics Test".to_string(),
        "localhost:8087".to_string(),
        5,
    )
    .await
    .unwrap();

    assert_eq!(metrics.get_messages_sent(), 0);
    assert_eq!(metrics.get_messages_received(), 0);

    // Send message
    pool.send_to_connection(&"metrics-test".to_string(), PoolMessage::Ping)
        .await
        .unwrap();

    assert_eq!(metrics.get_messages_sent(), 1);

    pool.remove_connection(&"metrics-test".to_string())
        .await
        .unwrap();
}

#[tokio::test]
async fn test_pool_connection_state() {
    init_test_tracing();

    let pool = ConnectionPool::new(PoolConfig::default());

    let conn_id = pool
        .add_connection(
            "state-test".to_string(),
            "State Test".to_string(),
            "localhost:8087".to_string(),
            5,
        )
        .await
        .unwrap();

    let connection = pool.get_connection(&conn_id).unwrap();

    // Initial state
    assert!(connection.state.is_active());
    assert_eq!(
        connection
            .state
            .messages_sent
            .load(std::sync::atomic::Ordering::Relaxed),
        0
    );
    assert_eq!(
        connection
            .state
            .messages_received
            .load(std::sync::atomic::Ordering::Relaxed),
        0
    );
    assert_eq!(
        connection
            .state
            .errors
            .load(std::sync::atomic::Ordering::Relaxed),
        0
    );

    // Record sent/received
    connection.state.record_sent();
    connection.state.record_received();

    assert_eq!(
        connection
            .state
            .messages_sent
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );
    assert_eq!(
        connection
            .state
            .messages_received
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );

    // Record error
    connection.state.record_error("Test error".to_string());
    assert_eq!(
        connection
            .state
            .errors
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );
    assert_eq!(
        connection.state.last_error.read().clone(),
        Some("Test error".to_string())
    );

    // Deactivate
    connection.state.deactivate();
    assert!(!connection.state.is_active());
}
