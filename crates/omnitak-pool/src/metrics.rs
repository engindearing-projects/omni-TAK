//! Metrics Collection
//!
//! Collects and exports metrics for the connection pool including:
//! - Messages per second (inbound/outbound)
//! - Connection counts
//! - Latency percentiles
//! - Error counters
//! - Prometheus export

use metrics::{counter, describe_counter, describe_gauge, describe_histogram, gauge, histogram};
use metrics_exporter_prometheus::{Matcher, PrometheusBuilder, PrometheusHandle};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::info;

/// Pool metrics collector
#[derive(Debug)]
pub struct PoolMetrics {
    messages_sent: AtomicU64,
    messages_received: AtomicU64,
    connections_added: AtomicU64,
    connections_removed: AtomicU64,
}

impl PoolMetrics {
    pub fn new() -> Self {
        // Describe metrics
        describe_counter!(
            "pool_messages_sent_total",
            "Total messages sent through pool"
        );
        describe_counter!(
            "pool_messages_received_total",
            "Total messages received by pool"
        );
        describe_counter!("pool_connections_added_total", "Total connections added");
        describe_counter!(
            "pool_connections_removed_total",
            "Total connections removed"
        );

        Self {
            messages_sent: AtomicU64::new(0),
            messages_received: AtomicU64::new(0),
            connections_added: AtomicU64::new(0),
            connections_removed: AtomicU64::new(0),
        }
    }

    pub fn record_message_sent(&self) {
        self.messages_sent.fetch_add(1, Ordering::Relaxed);
        counter!("pool_messages_sent_total").increment(1);
    }

    pub fn record_message_received(&self) {
        self.messages_received.fetch_add(1, Ordering::Relaxed);
        counter!("pool_messages_received_total").increment(1);
    }

    pub fn record_connection_added(&self) {
        self.connections_added.fetch_add(1, Ordering::Relaxed);
        counter!("pool_connections_added_total").increment(1);
    }

    pub fn record_connection_removed(&self) {
        self.connections_removed.fetch_add(1, Ordering::Relaxed);
        counter!("pool_connections_removed_total").increment(1);
    }

    pub fn get_messages_sent(&self) -> u64 {
        self.messages_sent.load(Ordering::Relaxed)
    }

    pub fn get_messages_received(&self) -> u64 {
        self.messages_received.load(Ordering::Relaxed)
    }
}

impl Default for PoolMetrics {
    fn default() -> Self {
        Self::new()
    }
}

/// Distributor metrics collector
#[derive(Debug)]
pub struct DistributorMetrics {
    messages_received: AtomicU64,
    messages_sent: AtomicU64,
    messages_dropped: AtomicU64,
    batches_processed: AtomicU64,
    start_time: Instant,
}

impl DistributorMetrics {
    pub fn new() -> Self {
        describe_counter!(
            "distributor_messages_received_total",
            "Total messages received by distributor"
        );
        describe_counter!(
            "distributor_messages_sent_total",
            "Total messages distributed"
        );
        describe_counter!(
            "distributor_messages_dropped_total",
            "Total messages dropped due to full channels"
        );
        describe_counter!(
            "distributor_batches_processed_total",
            "Total batches processed"
        );
        describe_histogram!(
            "distributor_latency_seconds",
            "Distribution latency in seconds"
        );
        describe_histogram!("distributor_batch_size", "Number of messages in each batch");
        describe_histogram!(
            "distributor_batch_duration_seconds",
            "Time to process each batch in seconds"
        );

        Self {
            messages_received: AtomicU64::new(0),
            messages_sent: AtomicU64::new(0),
            messages_dropped: AtomicU64::new(0),
            batches_processed: AtomicU64::new(0),
            start_time: Instant::now(),
        }
    }

    pub fn record_message_received(&self) {
        self.messages_received.fetch_add(1, Ordering::Relaxed);
        counter!("distributor_messages_received_total").increment(1);
    }

    pub fn record_message_sent(&self) {
        self.messages_sent.fetch_add(1, Ordering::Relaxed);
        counter!("distributor_messages_sent_total").increment(1);
    }

    pub fn record_drop(&self) {
        self.messages_dropped.fetch_add(1, Ordering::Relaxed);
        counter!("distributor_messages_dropped_total").increment(1);
    }

    pub fn record_distribution_latency(&self, latency: Duration) {
        histogram!("distributor_latency_seconds").record(latency.as_secs_f64());
    }

    pub fn record_batch_processed(&self, batch_size: usize, duration: Duration) {
        self.batches_processed.fetch_add(1, Ordering::Relaxed);
        counter!("distributor_batches_processed_total").increment(1);
        histogram!("distributor_batch_size").record(batch_size as f64);
        histogram!("distributor_batch_duration_seconds").record(duration.as_secs_f64());
    }

    pub fn get_throughput(&self) -> f64 {
        let elapsed = self.start_time.elapsed().as_secs_f64();
        if elapsed > 0.0 {
            self.messages_sent.load(Ordering::Relaxed) as f64 / elapsed
        } else {
            0.0
        }
    }
}

impl Default for DistributorMetrics {
    fn default() -> Self {
        Self::new()
    }
}

/// Aggregator metrics collector
#[derive(Debug)]
pub struct AggregatorMetrics {
    messages_received: AtomicU64,
    unique_messages: AtomicU64,
    duplicate_messages: AtomicU64,
    messages_no_uid: AtomicU64,
    cache_cleanups: AtomicU64,
}

impl AggregatorMetrics {
    pub fn new() -> Self {
        describe_counter!(
            "aggregator_messages_received_total",
            "Total messages received by aggregator"
        );
        describe_counter!(
            "aggregator_unique_messages_total",
            "Total unique messages forwarded"
        );
        describe_counter!(
            "aggregator_duplicate_messages_total",
            "Total duplicate messages dropped"
        );
        describe_counter!(
            "aggregator_messages_no_uid_total",
            "Total messages without UID"
        );
        describe_counter!(
            "aggregator_cache_cleanups_total",
            "Total cache cleanup operations"
        );
        describe_gauge!(
            "aggregator_dedup_ratio",
            "Deduplication ratio (duplicates / total)"
        );

        Self {
            messages_received: AtomicU64::new(0),
            unique_messages: AtomicU64::new(0),
            duplicate_messages: AtomicU64::new(0),
            messages_no_uid: AtomicU64::new(0),
            cache_cleanups: AtomicU64::new(0),
        }
    }

    pub fn record_message_received(&self) {
        self.messages_received.fetch_add(1, Ordering::Relaxed);
        counter!("aggregator_messages_received_total").increment(1);
    }

    pub fn record_unique(&self) {
        self.unique_messages.fetch_add(1, Ordering::Relaxed);
        counter!("aggregator_unique_messages_total").increment(1);
        self.update_dedup_ratio();
    }

    pub fn record_duplicate(&self) {
        self.duplicate_messages.fetch_add(1, Ordering::Relaxed);
        counter!("aggregator_duplicate_messages_total").increment(1);
        self.update_dedup_ratio();
    }

    pub fn record_no_uid(&self) {
        self.messages_no_uid.fetch_add(1, Ordering::Relaxed);
        counter!("aggregator_messages_no_uid_total").increment(1);
    }

    pub fn record_cache_cleanup(&self, entries_removed: usize) {
        self.cache_cleanups.fetch_add(1, Ordering::Relaxed);
        counter!("aggregator_cache_cleanups_total").increment(1);
        histogram!("aggregator_cache_cleanup_entries").record(entries_removed as f64);
    }

    fn update_dedup_ratio(&self) {
        let duplicates = self.duplicate_messages.load(Ordering::Relaxed) as f64;
        let total = self.messages_received.load(Ordering::Relaxed) as f64;
        if total > 0.0 {
            gauge!("aggregator_dedup_ratio").set(duplicates / total);
        }
    }

    pub fn get_dedup_ratio(&self) -> f64 {
        let duplicates = self.duplicate_messages.load(Ordering::Relaxed) as f64;
        let total = self.messages_received.load(Ordering::Relaxed) as f64;
        if total > 0.0 {
            duplicates / total
        } else {
            0.0
        }
    }
}

impl Default for AggregatorMetrics {
    fn default() -> Self {
        Self::new()
    }
}

/// Metrics exporter configuration
#[derive(Debug, Clone)]
pub struct MetricsConfig {
    /// Enable metrics export
    pub enabled: bool,
    /// Prometheus HTTP endpoint address
    pub bind_address: SocketAddr,
    /// Histogram buckets for latency (in seconds)
    pub latency_buckets: Vec<f64>,
    /// Histogram buckets for batch sizes
    pub batch_size_buckets: Vec<f64>,
}

impl Default for MetricsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            bind_address: "0.0.0.0:9090".parse().unwrap(),
            latency_buckets: vec![
                0.0001, 0.0005, 0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0,
            ],
            batch_size_buckets: vec![1.0, 5.0, 10.0, 25.0, 50.0, 100.0, 250.0, 500.0, 1000.0],
        }
    }
}

/// Metrics exporter
pub struct MetricsExporter {
    config: MetricsConfig,
    handle: Option<PrometheusHandle>,
}

impl MetricsExporter {
    /// Create a new metrics exporter
    pub fn new(config: MetricsConfig) -> Self {
        Self {
            config,
            handle: None,
        }
    }

    /// Initialize Prometheus exporter
    pub fn init(&mut self) -> anyhow::Result<()> {
        if !self.config.enabled {
            info!("Metrics export disabled");
            return Ok(());
        }

        let builder = PrometheusBuilder::new();

        // Configure histogram buckets for latency metrics
        let builder = builder.set_buckets_for_metric(
            Matcher::Suffix("latency_seconds".to_string()),
            &self.config.latency_buckets,
        )?;

        // Configure histogram buckets for batch size metrics
        let builder = builder.set_buckets_for_metric(
            Matcher::Suffix("batch_size".to_string()),
            &self.config.batch_size_buckets,
        )?;

        // Install the exporter
        let handle = builder.install_recorder()?;

        self.handle = Some(handle);

        info!(
            bind_address = %self.config.bind_address,
            "Prometheus metrics exporter initialized"
        );

        Ok(())
    }

    /// Start HTTP server for Prometheus scraping
    pub async fn start_server(&self) -> anyhow::Result<()> {
        if !self.config.enabled {
            return Ok(());
        }

        let handle = self
            .handle
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Metrics exporter not initialized"))?
            .clone();

        let addr = self.config.bind_address;

        tokio::spawn(async move {
            use std::convert::Infallible;

            let make_svc = hyper::service::make_service_fn(move |_conn| {
                let handle = handle.clone();
                async move {
                    Ok::<_, Infallible>(hyper::service::service_fn(move |_req| {
                        let handle = handle.clone();
                        async move {
                            let metrics = handle.render();
                            Ok::<_, Infallible>(hyper::Response::new(hyper::Body::from(metrics)))
                        }
                    }))
                }
            });

            let server = hyper::Server::bind(&addr).serve(make_svc);

            info!(
                bind_address = %addr,
                "Prometheus metrics HTTP server started"
            );

            if let Err(e) = server.await {
                tracing::error!(error = %e, "Metrics server error");
            }
        });

        Ok(())
    }

    /// Get current metrics snapshot as string
    pub fn render(&self) -> Option<String> {
        self.handle.as_ref().map(|h| h.render())
    }
}

/// Global metrics registry
pub struct MetricsRegistry {
    pool: Arc<PoolMetrics>,
    distributor: Arc<DistributorMetrics>,
    aggregator: Arc<AggregatorMetrics>,
    exporter: Arc<parking_lot::RwLock<MetricsExporter>>,
}

impl MetricsRegistry {
    /// Create a new metrics registry
    pub fn new(config: MetricsConfig) -> Self {
        Self {
            pool: Arc::new(PoolMetrics::new()),
            distributor: Arc::new(DistributorMetrics::new()),
            aggregator: Arc::new(AggregatorMetrics::new()),
            exporter: Arc::new(parking_lot::RwLock::new(MetricsExporter::new(config))),
        }
    }

    /// Initialize metrics exporter
    pub fn init(&self) -> anyhow::Result<()> {
        self.exporter.write().init()
    }

    /// Start metrics HTTP server
    // The exporter is only mutated by init(); holding the read guard for the
    // lifetime of the server is intentional and does not block any writer.
    #[allow(clippy::await_holding_lock)]
    pub async fn start_server(&self) -> anyhow::Result<()> {
        self.exporter.read().start_server().await
    }

    /// Get pool metrics
    pub fn pool(&self) -> Arc<PoolMetrics> {
        Arc::clone(&self.pool)
    }

    /// Get distributor metrics
    pub fn distributor(&self) -> Arc<DistributorMetrics> {
        Arc::clone(&self.distributor)
    }

    /// Get aggregator metrics
    pub fn aggregator(&self) -> Arc<AggregatorMetrics> {
        Arc::clone(&self.aggregator)
    }

    /// Update gauge metrics (should be called periodically)
    pub fn update_gauges(&self, pool: &crate::pool::ConnectionPool) {
        let stats = pool.stats();

        gauge!("pool_connections_total").set(stats.total_connections as f64);
        gauge!("pool_connections_active").set(stats.active_connections as f64);
        gauge!("pool_connections_inactive").set(stats.inactive_connections as f64);
        gauge!("pool_messages_sent_total").set(stats.total_messages_sent as f64);
        gauge!("pool_messages_received_total").set(stats.total_messages_received as f64);
        gauge!("pool_errors_total").set(stats.total_errors as f64);
    }

    /// Get metrics snapshot
    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            pool_messages_sent: self.pool.get_messages_sent(),
            pool_messages_received: self.pool.get_messages_received(),
            distributor_throughput: self.distributor.get_throughput(),
            aggregator_dedup_ratio: self.aggregator.get_dedup_ratio(),
        }
    }
}

/// Metrics snapshot for reporting
#[derive(Debug, Clone)]
pub struct MetricsSnapshot {
    pub pool_messages_sent: u64,
    pub pool_messages_received: u64,
    pub distributor_throughput: f64,
    pub aggregator_dedup_ratio: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pool_metrics() {
        let metrics = PoolMetrics::new();

        metrics.record_message_sent();
        metrics.record_message_sent();
        metrics.record_message_received();

        assert_eq!(metrics.get_messages_sent(), 2);
        assert_eq!(metrics.get_messages_received(), 1);
    }

    #[test]
    fn test_distributor_metrics() {
        let metrics = DistributorMetrics::new();

        metrics.record_message_received();
        metrics.record_message_sent();
        metrics.record_drop();

        assert_eq!(metrics.messages_received.load(Ordering::Relaxed), 1);
        assert_eq!(metrics.messages_sent.load(Ordering::Relaxed), 1);
        assert_eq!(metrics.messages_dropped.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_aggregator_metrics() {
        let metrics = AggregatorMetrics::new();

        metrics.record_message_received();
        metrics.record_message_received();
        metrics.record_unique();
        metrics.record_duplicate();

        assert_eq!(metrics.messages_received.load(Ordering::Relaxed), 2);
        assert_eq!(metrics.unique_messages.load(Ordering::Relaxed), 1);
        assert_eq!(metrics.duplicate_messages.load(Ordering::Relaxed), 1);
        assert_eq!(metrics.get_dedup_ratio(), 0.5);
    }

    #[test]
    fn test_metrics_config_default() {
        let config = MetricsConfig::default();
        assert!(config.enabled);
        assert!(!config.latency_buckets.is_empty());
        assert!(!config.batch_size_buckets.is_empty());
    }
}
