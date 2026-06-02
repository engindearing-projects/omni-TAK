//! mDNS Discovery Service Implementation

use crate::config::{DiscoveryConfig, ServiceType};
use crate::error::{DiscoveryError, Result};
use crate::types::{DiscoveredService, ServiceEvent, ServiceEventType, ServiceStatus};
use async_channel::{Receiver, Sender};
use dashmap::DashMap;
use mdns_sd::{ServiceDaemon, ServiceEvent as MdnsEvent, ServiceInfo};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

/// Maximum number of events to buffer in the channel
const EVENT_CHANNEL_CAPACITY: usize = 1000;

/// Main discovery service managing mDNS operations
pub struct DiscoveryService {
    /// Configuration
    config: DiscoveryConfig,

    /// mDNS service daemon
    mdns: Arc<ServiceDaemon>,

    /// Registry of discovered services (keyed by full service name)
    services: Arc<DashMap<String, DiscoveredService>>,

    /// Event channel sender
    event_tx: Sender<ServiceEvent>,

    /// Event channel receiver (for external consumers)
    event_rx: Receiver<ServiceEvent>,

    /// Running state
    running: Arc<AtomicBool>,

    /// Background task handles
    tasks: Arc<DashMap<String, JoinHandle<()>>>,
}

impl DiscoveryService {
    /// Creates a new discovery service
    pub fn new(config: DiscoveryConfig) -> Result<Self> {
        // Validate configuration
        config
            .validate()
            .map_err(|e| DiscoveryError::InvalidConfig(e))?;

        // Create mDNS daemon
        let mdns = ServiceDaemon::new().map_err(|e| {
            DiscoveryError::MdnsInitFailed(format!("Failed to create mDNS daemon: {}", e))
        })?;

        // Create event channel
        let (event_tx, event_rx) = async_channel::bounded(EVENT_CHANNEL_CAPACITY);

        info!(
            enabled = config.enabled,
            announce = config.announce_enabled,
            "Discovery service created"
        );

        Ok(Self {
            config,
            mdns: Arc::new(mdns),
            services: Arc::new(DashMap::new()),
            event_tx,
            event_rx,
            running: Arc::new(AtomicBool::new(false)),
            tasks: Arc::new(DashMap::new()),
        })
    }

    /// Starts the discovery service
    pub async fn start(&self) -> Result<()> {
        if self.running.load(Ordering::SeqCst) {
            return Err(DiscoveryError::AlreadyStarted);
        }

        if !self.config.enabled {
            info!("Discovery service is disabled in configuration");
            return Ok(());
        }

        info!("Starting discovery service");
        self.running.store(true, Ordering::SeqCst);

        // Start browsing for each configured service type
        for service_type in &self.config.service_types {
            self.start_browser(service_type.clone()).await?;
        }

        // Start announcement if enabled
        if self.config.announce_enabled {
            self.start_announcement().await?;
        }

        // Start cleanup task
        self.start_cleanup_task().await;

        info!("Discovery service started successfully");
        Ok(())
    }

    /// Stops the discovery service
    pub async fn stop(&self) -> Result<()> {
        if !self.running.load(Ordering::SeqCst) {
            return Ok(());
        }

        info!("Stopping discovery service");
        self.running.store(false, Ordering::SeqCst);

        // Cancel all background tasks
        let tasks = self
            .tasks
            .iter()
            .map(|entry| entry.key().clone())
            .collect::<Vec<_>>();
        for task_name in tasks {
            if let Some((_, handle)) = self.tasks.remove(&task_name) {
                handle.abort();
            }
        }

        // Shutdown mDNS daemon
        self.mdns.shutdown().map_err(|e| {
            DiscoveryError::Internal(format!("Failed to shutdown mDNS daemon: {}", e))
        })?;

        info!("Discovery service stopped");
        Ok(())
    }

    /// Returns whether the service is running
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Gets all discovered services
    pub async fn get_discovered_services(&self) -> Vec<DiscoveredService> {
        self.services
            .iter()
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// Gets discovered services filtered by type
    pub async fn get_services_by_type(&self, service_type: &ServiceType) -> Vec<DiscoveredService> {
        self.services
            .iter()
            .filter(|entry| &entry.value().service_type == service_type)
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// Gets a specific service by ID
    pub async fn get_service(&self, id: &str) -> Option<DiscoveredService> {
        self.services
            .iter()
            .find(|entry| entry.value().id == id)
            .map(|entry| entry.value().clone())
    }

    /// Returns the event receiver for external consumers
    pub fn event_receiver(&self) -> Receiver<ServiceEvent> {
        self.event_rx.clone()
    }

    /// Manually refresh services (trigger a new browse)
    pub async fn refresh(&self) -> Result<()> {
        if !self.running.load(Ordering::SeqCst) {
            return Err(DiscoveryError::NotStarted);
        }

        info!("Manually refreshing service discovery");

        // The mdns-sd library handles ongoing browsing automatically
        // This is a no-op but could be extended to force re-query
        Ok(())
    }

    /// Starts browsing for a specific service type
    async fn start_browser(&self, service_type: ServiceType) -> Result<()> {
        let service_string = service_type.to_service_string();

        info!(
            service_type = service_string,
            description = service_type.description(),
            "Starting browser"
        );

        let receiver =
            self.mdns
                .browse(&service_string)
                .map_err(|e| DiscoveryError::BrowseFailed {
                    service_type: service_string.clone(),
                    reason: e.to_string(),
                })?;

        // Spawn task to handle browse events
        let services = self.services.clone();
        let event_tx = self.event_tx.clone();
        let running = self.running.clone();
        let svc_type = service_type.clone();
        let service_string_clone = service_string.clone();

        let task = tokio::spawn(async move {
            while running.load(Ordering::SeqCst) {
                match receiver.recv_async().await {
                    Ok(event) => {
                        Self::handle_mdns_event(event, &svc_type, &services, &event_tx).await;
                    }
                    Err(e) => {
                        error!(error = %e, "Error receiving mDNS event");
                        break;
                    }
                }
            }
            debug!("Browser task for {} stopped", service_string_clone);
        });

        let task_name = format!("browser_{}", service_string);
        self.tasks.insert(task_name, task);

        Ok(())
    }

    /// Handles an mDNS event
    async fn handle_mdns_event(
        event: MdnsEvent,
        service_type: &ServiceType,
        services: &DashMap<String, DiscoveredService>,
        event_tx: &Sender<ServiceEvent>,
    ) {
        match event {
            MdnsEvent::ServiceResolved(info) => {
                debug!(
                    service = info.get_fullname(),
                    hostname = info.get_hostname(),
                    port = info.get_port(),
                    "Service resolved"
                );

                let discovered = Self::convert_service_info(info, service_type.clone());
                let full_name = discovered.instance_name.clone();

                // Check if this is a new service or an update
                let (event_type, service) = if let Some(mut existing) = services.get_mut(&full_name)
                {
                    existing.mark_seen();
                    (ServiceEventType::Updated, existing.clone())
                } else {
                    services.insert(full_name.clone(), discovered.clone());
                    (ServiceEventType::Discovered, discovered)
                };

                // Send event notification
                let event = ServiceEvent::new(event_type, service);
                if let Err(e) = event_tx.send(event).await {
                    warn!(error = %e, "Failed to send service event");
                }
            }

            MdnsEvent::ServiceRemoved(typ, fullname) => {
                debug!(service = fullname, typ = typ, "Service removed");

                if let Some((_, mut service)) = services.remove(&fullname) {
                    service.mark_lost();
                    let event = ServiceEvent::new(ServiceEventType::Lost, service);
                    if let Err(e) = event_tx.send(event).await {
                        warn!(error = %e, "Failed to send service event");
                    }
                }
            }

            MdnsEvent::SearchStarted(typ) => {
                debug!(typ = typ, "Search started");
            }

            MdnsEvent::SearchStopped(typ) => {
                debug!(typ = typ, "Search stopped");
            }

            _ => {}
        }
    }

    /// Converts ServiceInfo from mdns-sd to our DiscoveredService
    fn convert_service_info(info: ServiceInfo, service_type: ServiceType) -> DiscoveredService {
        let addresses: Vec<IpAddr> = info.get_addresses().iter().copied().collect();

        let mut properties = HashMap::new();
        for property in info.get_properties().iter() {
            properties.insert(property.key().to_string(), property.val_str().to_string());
        }

        DiscoveredService::new(
            service_type,
            info.get_fullname().to_string(),
            info.get_hostname().to_string(),
            addresses,
            info.get_port(),
            properties,
        )
    }

    /// Starts announcing this aggregator as a discoverable service
    async fn start_announcement(&self) -> Result<()> {
        let instance_name =
            self.config.instance_name.clone().unwrap_or_else(|| {
                format!("OmniTAK-{}", hostname::get().unwrap().to_string_lossy())
            });

        let service_type = "_tak-aggregator._tcp.local.";
        let port = self.config.announce_port;

        info!(
            instance = instance_name,
            port = port,
            "Starting service announcement"
        );

        // Create service info with metadata
        let mut properties = HashMap::new();
        properties.insert("version".to_string(), env!("CARGO_PKG_VERSION").to_string());
        properties.insert(
            "description".to_string(),
            "OmniTAK Server Aggregator".to_string(),
        );
        properties.insert("api".to_string(), "rest+websocket".to_string());

        let service_info = ServiceInfo::new(
            service_type,
            &instance_name,
            &instance_name,
            "",
            port,
            properties,
        )
        .map_err(|e| DiscoveryError::RegisterFailed {
            service_name: instance_name.clone(),
            reason: e.to_string(),
        })?;

        // Register the service
        self.mdns
            .register(service_info)
            .map_err(|e| DiscoveryError::RegisterFailed {
                service_name: instance_name.clone(),
                reason: e.to_string(),
            })?;

        info!("Service announcement registered successfully");
        Ok(())
    }

    /// Starts the cleanup task to remove stale services
    async fn start_cleanup_task(&self) {
        let services = self.services.clone();
        let running = self.running.clone();
        let event_tx = self.event_tx.clone();
        let cleanup_interval = self.config.cleanup_interval();
        let stale_timeout = self.config.stale_timeout();

        let task = tokio::spawn(async move {
            while running.load(Ordering::SeqCst) {
                tokio::time::sleep(cleanup_interval).await;

                let now = chrono::Utc::now();
                let stale_threshold = now - chrono::Duration::from_std(stale_timeout).unwrap();

                // Find and mark stale services
                let mut to_remove = Vec::new();
                for mut entry in services.iter_mut() {
                    let service = entry.value_mut();

                    if service.last_seen_at < stale_threshold {
                        match service.status {
                            ServiceStatus::Active => {
                                service.mark_stale();
                                debug!(service = service.instance_name, "Service marked as stale");

                                let event =
                                    ServiceEvent::new(ServiceEventType::Stale, service.clone());
                                let _ = event_tx.send(event).await;
                            }
                            ServiceStatus::Stale => {
                                // If already stale for twice the timeout, remove it
                                let remove_threshold =
                                    now - chrono::Duration::from_std(stale_timeout * 2).unwrap();
                                if service.last_seen_at < remove_threshold {
                                    to_remove.push(entry.key().clone());
                                }
                            }
                            _ => {}
                        }
                    }
                }

                // Remove very old stale services
                for key in to_remove {
                    if let Some((_, mut service)) = services.remove(&key) {
                        service.mark_lost();
                        info!(service = service.instance_name, "Removing stale service");

                        let event = ServiceEvent::new(ServiceEventType::Lost, service);
                        let _ = event_tx.send(event).await;
                    }
                }
            }

            debug!("Cleanup task stopped");
        });

        self.tasks.insert("cleanup".to_string(), task);
    }
}

impl Drop for DiscoveryService {
    fn drop(&mut self) {
        if self.running.load(Ordering::SeqCst) {
            warn!("Discovery service dropped while still running");
            // Note: Can't use async in Drop, so we just shutdown the daemon synchronously
            let _ = self.mdns.shutdown();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_service_creation() {
        let config = DiscoveryConfig::default();
        let service = DiscoveryService::new(config);
        assert!(service.is_ok());
    }

    #[tokio::test]
    async fn test_invalid_config() {
        let config = DiscoveryConfig {
            announce_port: 0,
            ..Default::default()
        };
        let service = DiscoveryService::new(config);
        assert!(service.is_err());
    }

    #[tokio::test]
    async fn test_service_lifecycle() {
        let config = DiscoveryConfig {
            enabled: true,
            announce_enabled: false, // Don't announce in tests
            ..Default::default()
        };

        let service = DiscoveryService::new(config).unwrap();
        assert!(!service.is_running());

        // Note: Actual start may fail in CI without mDNS daemon
        // This test just verifies the API structure
    }
}
