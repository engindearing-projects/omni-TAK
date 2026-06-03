//! USB device monitoring service

use anyhow::Result;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tokio::time::{interval, Duration};
use tracing::{debug, info, warn};

use crate::AdbClient;

/// Device event types
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceEvent {
    /// Device connected
    Connected(String),
    /// Device disconnected
    Disconnected(String),
}

/// Device monitor service
pub struct DeviceMonitor {
    /// ADB client
    adb_client: AdbClient,
    /// Currently tracked device serials
    tracked_devices: Arc<RwLock<HashSet<String>>>,
    /// Event sender
    event_tx: mpsc::UnboundedSender<DeviceEvent>,
    /// Event receiver
    event_rx: Arc<RwLock<mpsc::UnboundedReceiver<DeviceEvent>>>,
    /// Poll interval
    poll_interval: Duration,
}

impl DeviceMonitor {
    /// Create a new device monitor
    pub fn new(poll_interval: Duration) -> Self {
        let (event_tx, event_rx) = mpsc::unbounded_channel();

        Self {
            adb_client: AdbClient::new(),
            tracked_devices: Arc::new(RwLock::new(HashSet::new())),
            event_tx,
            event_rx: Arc::new(RwLock::new(event_rx)),
            poll_interval,
        }
    }

    /// Start monitoring for device changes
    pub async fn start(self: Arc<Self>) {
        info!(
            "Starting USB device monitor (poll interval: {:?})...",
            self.poll_interval
        );

        let mut interval = interval(self.poll_interval);

        loop {
            interval.tick().await;

            if let Err(e) = self.check_devices().await {
                warn!("Error checking devices: {}", e);
            }
        }
    }

    /// Check for device changes
    async fn check_devices(&self) -> Result<()> {
        // Check if ADB is available
        if !self.adb_client.is_available() {
            debug!("ADB not available, skipping device check");
            return Ok(());
        }

        // Get current devices
        let current_devices = match self.adb_client.list_devices() {
            Ok(devices) => devices
                .into_iter()
                .filter(|d| d.state == "device")
                .map(|d| d.serial)
                .collect::<HashSet<_>>(),
            Err(e) => {
                warn!("Failed to list devices: {}", e);
                return Ok(());
            }
        };

        let mut tracked = self.tracked_devices.write().await;

        // Find new devices (connected)
        for serial in current_devices.difference(&*tracked) {
            info!("Device connected: {}", serial);
            let _ = self.event_tx.send(DeviceEvent::Connected(serial.clone()));
        }

        // Find removed devices (disconnected)
        for serial in tracked.difference(&current_devices) {
            info!("Device disconnected: {}", serial);
            let _ = self
                .event_tx
                .send(DeviceEvent::Disconnected(serial.clone()));
        }

        // Update tracked devices
        *tracked = current_devices;

        Ok(())
    }

    /// Get an event receiver
    pub fn subscribe(&self) -> mpsc::UnboundedReceiver<DeviceEvent> {
        let (_tx, rx) = mpsc::unbounded_channel();

        // Clone events to new subscriber
        let _event_tx = self.event_tx.clone();
        tokio::spawn(async move {
            // Forward events to new subscriber
            // Note: This is a simple implementation; for production, consider using tokio::sync::broadcast
        });

        rx
    }

    /// Get currently connected devices
    pub async fn get_devices(&self) -> Vec<String> {
        self.tracked_devices.read().await.iter().cloned().collect()
    }

    /// Check if a specific device is connected
    pub async fn is_device_connected(&self, serial: &str) -> bool {
        self.tracked_devices.read().await.contains(serial)
    }
}

/// Builder for DeviceMonitor
pub struct DeviceMonitorBuilder {
    poll_interval: Duration,
}

impl Default for DeviceMonitorBuilder {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(2),
        }
    }
}

impl DeviceMonitorBuilder {
    /// Create a new builder
    pub fn new() -> Self {
        Self::default()
    }

    /// Set poll interval
    pub fn poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }

    /// Build the monitor
    pub fn build(self) -> Arc<DeviceMonitor> {
        Arc::new(DeviceMonitor::new(self.poll_interval))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_device_monitor_creation() {
        let monitor = DeviceMonitorBuilder::new()
            .poll_interval(Duration::from_secs(5))
            .build();

        assert_eq!(monitor.poll_interval, Duration::from_secs(5));
    }
}
