//! Load and performance tests
//!
//! Run these with: cargo test --release load_test -- --nocapture --test-threads=1
//! These tests are marked with #[ignore] by default to avoid running in CI

mod common;

use common::{generate_cot_message, generate_unique_uid, init_test_tracing, TestEnvironment};
use omnitak_pool::{AggregatorConfig, DistributorConfig, InboundMessage, PoolConfig};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "Long running load test"]
async fn load_test_1000_concurrent_connections() {
    init_test_tracing();

    let start = Instant::now();

    let env = Arc::new(
        TestEnvironment::with_config(
            PoolConfig {
                max_connections: 2000,
                channel_capacity: 1000,
                ..Default::default()
            },
            DistributorConfig {
                max_workers: 16,
                channel_capacity: 20000,
                ..Default::default()
            },
            AggregatorConfig {
                worker_count: 8,
                channel_capacity: 20000,
                ..Default::default()
            },
        )
        .await,
    );

    println!("Environment setup completed in {:?}", start.elapsed());

    // Add 1000 connections
    let add_start = Instant::now();
    let mut handles = vec![];

    for i in 1..=1000 {
        let env_clone = Arc::clone(&env);
        let handle = tokio::spawn(async move {
            env_clone
                .add_connection(&format!("load-conn-{}", i), 5)
                .await
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.await.unwrap().unwrap();
    }

    println!(
        "Added 1000 connections in {:?}",
        add_start.elapsed()
    );
    assert_eq!(env.pool.connection_count(), 1000);

    // Send messages for 10 seconds
    let send_start = Instant::now();
    let sent_count = Arc::new(AtomicU64::new(0));

    let send_task = {
        let env_clone = Arc::clone(&env);
        let sent_clone = Arc::clone(&sent_count);
        tokio::spawn(async move {
            while send_start.elapsed() < Duration::from_secs(10) {
                let uid = generate_unique_uid();
                let cot = generate_cot_message(&uid);

                let msg = InboundMessage {
                    data: cot,
                    source: "load-tester".to_string(),
                    timestamp: Instant::now(),
                };

                if env_clone.aggregator.sender().send_async(msg).await.is_ok() {
                    sent_clone.fetch_add(1, Ordering::Relaxed);
                }
            }
        })
    };

    send_task.await.unwrap();

    let total_sent = sent_count.load(Ordering::Relaxed);
    let throughput = total_sent as f64 / 10.0;

    println!(
        "Sent {} messages in 10 seconds ({:.0} msg/s)",
        total_sent, throughput
    );

    // Wait for processing
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Check metrics
    let stats = env.pool.stats();
    println!("Pool stats: {:?}", stats);

    let agg_metrics = env.aggregator.metrics();
    let unique = agg_metrics.unique_messages.load(Ordering::Relaxed);
    println!("Unique messages: {}", unique);

    let dist_metrics = env.distributor.metrics();
    let dist_throughput = dist_metrics.get_throughput();
    println!("Distribution throughput: {:.0} msg/s", dist_throughput);

    println!("Total test time: {:?}", start.elapsed());

    assert!(throughput >= 1000.0, "Throughput should be at least 1000 msg/s");

    let env = Arc::try_unwrap(env).unwrap();
    env.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "Long running load test"]
async fn load_test_10k_messages_per_second() {
    init_test_tracing();

    let env = Arc::new(
        TestEnvironment::with_config(
            PoolConfig::default(),
            DistributorConfig {
                max_workers: 16,
                channel_capacity: 50000,
                batch_size: 100,
                ..Default::default()
            },
            AggregatorConfig {
                worker_count: 8,
                channel_capacity: 50000,
                ..Default::default()
            },
        )
        .await,
    );

    // Add 10 connections
    for i in 1..=10 {
        env.add_connection(&format!("perf-conn-{}", i), 5)
            .await
            .unwrap();
    }

    let start = Instant::now();
    let target_messages = 100_000;
    let sent_count = Arc::new(AtomicU64::new(0));

    // Spawn multiple sender tasks
    let mut handles = vec![];

    for _ in 0..8 {
        let env_clone = Arc::clone(&env);
        let sent_clone = Arc::clone(&sent_count);

        let handle = tokio::spawn(async move {
            for _ in 0..(target_messages / 8) {
                let uid = generate_unique_uid();
                let cot = generate_cot_message(&uid);

                let msg = InboundMessage {
                    data: cot,
                    source: "perf-tester".to_string(),
                    timestamp: Instant::now(),
                };

                if env_clone.aggregator.sender().send_async(msg).await.is_ok() {
                    sent_clone.fetch_add(1, Ordering::Relaxed);
                }
            }
        });

        handles.push(handle);
    }

    // Wait for all senders
    for handle in handles {
        handle.await.unwrap();
    }

    let send_duration = start.elapsed();
    let total_sent = sent_count.load(Ordering::Relaxed);
    let throughput = total_sent as f64 / send_duration.as_secs_f64();

    println!(
        "Sent {} messages in {:?} ({:.0} msg/s)",
        total_sent, send_duration, throughput
    );

    // Wait for processing
    tokio::time::sleep(Duration::from_secs(3)).await;

    let agg_metrics = env.aggregator.metrics();
    let processed = agg_metrics.messages_received.load(Ordering::Relaxed);

    println!("Processed {} messages", processed);

    assert!(
        throughput >= 10_000.0,
        "Throughput should be at least 10k msg/s, got {:.0}",
        throughput
    );

    let env = Arc::try_unwrap(env).unwrap();
    env.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "Long running load test"]
async fn load_test_memory_usage() {
    init_test_tracing();

    let env = Arc::new(TestEnvironment::new().await);

    // Add 100 connections
    for i in 1..=100 {
        env.add_connection(&format!("mem-conn-{}", i), 5)
            .await
            .unwrap();
    }

    // Send messages continuously and monitor cache size
    let start = Instant::now();
    let mut max_cache_size = 0;

    for iteration in 0..10 {
        // Send 1000 messages
        for _ in 0..1000 {
            let uid = generate_unique_uid();
            let cot = generate_cot_message(&uid);

            let msg = InboundMessage {
                data: cot,
                source: "mem-tester".to_string(),
                timestamp: Instant::now(),
            };

            env.aggregator.sender().send_async(msg).await.unwrap();
        }

        tokio::time::sleep(Duration::from_millis(500)).await;

        let (cache_entries, _) = env.aggregator.cache_stats();
        max_cache_size = max_cache_size.max(cache_entries);

        println!(
            "Iteration {}: Cache size = {} entries",
            iteration + 1,
            cache_entries
        );
    }

    println!("Max cache size: {} entries", max_cache_size);
    println!("Test duration: {:?}", start.elapsed());

    // Cache should stay within reasonable bounds
    assert!(
        max_cache_size < 50_000,
        "Cache size should stay under 50k entries"
    );

    let env = Arc::try_unwrap(env).unwrap();
    env.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "Long running load test"]
async fn load_test_latency_p99() {
    init_test_tracing();

    let env = Arc::new(TestEnvironment::new().await);

    // Add 5 connections
    for i in 1..=5 {
        env.add_connection(&format!("lat-conn-{}", i), 5)
            .await
            .unwrap();
    }

    // Send messages and track latencies
    let mut latencies = Vec::new();

    for _ in 0..10_000 {
        let send_time = Instant::now();
        let uid = generate_unique_uid();
        let cot = generate_cot_message(&uid);

        let msg = InboundMessage {
            data: cot,
            source: "latency-tester".to_string(),
            timestamp: send_time,
        };

        env.aggregator.sender().send_async(msg).await.unwrap();

        // Sample latency every 100 messages
        if latencies.len() < 100 && latencies.len() % 10 == 0 {
            latencies.push(send_time.elapsed());
        }
    }

    // Wait for processing
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Calculate p99 latency
    latencies.sort();
    let p99_index = (latencies.len() as f64 * 0.99) as usize;
    let p99_latency = latencies.get(p99_index).copied().unwrap_or_default();

    let p50_index = latencies.len() / 2;
    let p50_latency = latencies.get(p50_index).copied().unwrap_or_default();

    println!("P50 latency: {:?}", p50_latency);
    println!("P99 latency: {:?}", p99_latency);

    assert!(
        p99_latency < Duration::from_millis(10),
        "P99 latency should be under 10ms, got {:?}",
        p99_latency
    );

    let env = Arc::try_unwrap(env).unwrap();
    env.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "Long running load test"]
async fn load_test_connection_churn() {
    init_test_tracing();

    let env = Arc::new(
        TestEnvironment::with_config(
            PoolConfig {
                max_connections: 500,
                ..Default::default()
            },
            DistributorConfig::default(),
            AggregatorConfig::default(),
        )
        .await,
    );

    let start = Instant::now();
    let test_duration = Duration::from_secs(30);

    // Continuously add and remove connections
    let add_remove_task = {
        let env_clone = Arc::clone(&env);
        tokio::spawn(async move {
            let mut conn_id = 0;

            while start.elapsed() < test_duration {
                // Add 10 connections
                let mut added = Vec::new();
                for _ in 0..10 {
                    conn_id += 1;
                    if let Ok(id) = env_clone
                        .add_connection(&format!("churn-{}", conn_id), 5)
                        .await
                    {
                        added.push(id);
                    }
                }

                tokio::time::sleep(Duration::from_millis(500)).await;

                // Remove half of them
                for id in added.iter().take(5) {
                    let _ = env_clone.pool.remove_connection(id).await;
                }

                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        })
    };

    // Send messages during churn
    let send_task = {
        let env_clone = Arc::clone(&env);
        let sent_count = Arc::new(AtomicU64::new(0));
        let sent_clone = Arc::clone(&sent_count);

        tokio::spawn(async move {
            while start.elapsed() < test_duration {
                let uid = generate_unique_uid();
                let cot = generate_cot_message(&uid);

                let msg = InboundMessage {
                    data: cot,
                    source: "churn-tester".to_string(),
                    timestamp: Instant::now(),
                };

                if env_clone.aggregator.sender().send_async(msg).await.is_ok() {
                    sent_clone.fetch_add(1, Ordering::Relaxed);
                }

                tokio::time::sleep(Duration::from_millis(10)).await;
            }

            sent_count.load(Ordering::Relaxed)
        })
    };

    add_remove_task.await.unwrap();
    let total_sent = send_task.await.unwrap();

    println!(
        "Sent {} messages during {} of connection churn",
        total_sent,
        test_duration.as_secs()
    );

    // System should remain stable
    let final_conn_count = env.pool.connection_count();
    println!("Final connection count: {}", final_conn_count);

    assert!(
        total_sent > 1000,
        "Should have sent messages successfully during churn"
    );

    let env = Arc::try_unwrap(env).unwrap();
    env.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "Long running load test"]
async fn load_test_sustained_throughput() {
    init_test_tracing();

    let env = Arc::new(
        TestEnvironment::with_config(
            PoolConfig::default(),
            DistributorConfig {
                max_workers: 16,
                channel_capacity: 50000,
                ..Default::default()
            },
            AggregatorConfig {
                worker_count: 8,
                channel_capacity: 50000,
                ..Default::default()
            },
        )
        .await,
    );

    // Add 20 connections
    for i in 1..=20 {
        env.add_connection(&format!("sustained-{}", i), 5)
            .await
            .unwrap();
    }

    let test_duration = Duration::from_secs(60);
    let start = Instant::now();
    let sent_count = Arc::new(AtomicU64::new(0));

    // Spawn multiple sender tasks for sustained load
    let mut handles = vec![];

    for _ in 0..8 {
        let env_clone = Arc::clone(&env);
        let sent_clone = Arc::clone(&sent_count);
        let start_time = start;

        let handle = tokio::spawn(async move {
            while start_time.elapsed() < test_duration {
                let uid = generate_unique_uid();
                let cot = generate_cot_message(&uid);

                let msg = InboundMessage {
                    data: cot,
                    source: "sustained-tester".to_string(),
                    timestamp: Instant::now(),
                };

                if env_clone.aggregator.sender().send_async(msg).await.is_ok() {
                    sent_clone.fetch_add(1, Ordering::Relaxed);
                }
            }
        });

        handles.push(handle);
    }

    // Monitor progress every 10 seconds
    for interval in 1..=6 {
        tokio::time::sleep(Duration::from_secs(10)).await;
        let current_sent = sent_count.load(Ordering::Relaxed);
        let throughput = current_sent as f64 / (interval as f64 * 10.0);

        println!(
            "{}s: {} messages sent ({:.0} msg/s)",
            interval * 10,
            current_sent,
            throughput
        );
    }

    // Wait for all senders
    for handle in handles {
        handle.await.unwrap();
    }

    let total_sent = sent_count.load(Ordering::Relaxed);
    let avg_throughput = total_sent as f64 / test_duration.as_secs_f64();

    println!(
        "Total: {} messages in 60s (avg {:.0} msg/s)",
        total_sent, avg_throughput
    );

    // Wait for final processing
    tokio::time::sleep(Duration::from_secs(3)).await;

    let stats = env.pool.stats();
    println!("Final stats: {:?}", stats);

    assert!(
        avg_throughput >= 1000.0,
        "Should sustain at least 1000 msg/s"
    );

    let env = Arc::try_unwrap(env).unwrap();
    env.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "Long running load test"]
async fn load_test_deduplication_performance() {
    init_test_tracing();

    let env = Arc::new(
        TestEnvironment::with_config(
            PoolConfig::default(),
            DistributorConfig::default(),
            AggregatorConfig {
                worker_count: 8,
                channel_capacity: 50000,
                max_cache_entries: 100_000,
                ..Default::default()
            },
        )
        .await,
    );

    let _conn = env.add_connection("dedup-perf", 5).await.unwrap();

    // Create 10k unique UIDs
    let uids: Vec<String> = (0..10_000).map(|_| generate_unique_uid()).collect();
    let uids = Arc::new(uids);

    let start = Instant::now();

    // Each UID sent 10 times from different sources
    let mut handles = vec![];

    for replica in 0..10 {
        let env_clone = Arc::clone(&env);
        let uids_clone = Arc::clone(&uids);

        let handle = tokio::spawn(async move {
            for uid in uids_clone.iter() {
                let cot = generate_cot_message(uid);

                let msg = InboundMessage {
                    data: cot,
                    source: format!("source-{}", replica),
                    timestamp: Instant::now(),
                };

                env_clone.aggregator.sender().send_async(msg).await.unwrap();
            }
        });

        handles.push(handle);
    }

    for handle in handles {
        handle.await.unwrap();
    }

    let send_duration = start.elapsed();
    println!("Sent 100k messages in {:?}", send_duration);

    // Wait for processing
    tokio::time::sleep(Duration::from_secs(3)).await;

    let agg_metrics = env.aggregator.metrics();
    let unique = agg_metrics.unique_messages.load(Ordering::Relaxed);
    let dupes = agg_metrics.duplicate_messages.load(Ordering::Relaxed);
    let dedup_ratio = agg_metrics.get_dedup_ratio();

    println!("Unique messages: {}", unique);
    println!("Duplicate messages: {}", dupes);
    println!("Deduplication ratio: {:.2}%", dedup_ratio * 100.0);

    assert!(
        unique >= 9_500,
        "Should have ~10k unique messages, got {}",
        unique
    );
    assert!(
        dupes >= 85_000,
        "Should have ~90k duplicates, got {}",
        dupes
    );

    let env = Arc::try_unwrap(env).unwrap();
    env.shutdown().await;
}
