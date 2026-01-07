//! Notification aggregation to prevent flood during rapid events
//!
//! Aggregates multiple rapid device notifications into batches to avoid
//! overwhelming clients during USB hot-plug storms (e.g., hub connect/disconnect).

use protocol::{
    AggregatedNotification, DeviceHandle, DeviceId, DeviceInfo, DeviceRemovalReason,
    DeviceStatusChangeReason,
};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tracing::debug;

/// Aggregation window for batching notifications (100ms)
const AGGREGATION_WINDOW: Duration = Duration::from_millis(100);

/// Maximum notifications to aggregate before forcing a flush
const MAX_PENDING_NOTIFICATIONS: usize = 50;

/// Pending notification waiting to be aggregated or sent
#[derive(Debug, Clone)]
pub enum PendingNotification {
    /// Device arrived
    Arrived(DeviceInfo),
    /// Device removed
    Removed {
        device_id: DeviceId,
        invalidated_handles: Vec<DeviceHandle>,
        reason: DeviceRemovalReason,
    },
    /// Device status changed
    StatusChanged {
        device_id: DeviceId,
        device_info: Option<DeviceInfo>,
        reason: DeviceStatusChangeReason,
    },
}

impl PendingNotification {
    /// Get the device ID associated with this notification
    pub fn device_id(&self) -> DeviceId {
        match self {
            PendingNotification::Arrived(info) => info.id,
            PendingNotification::Removed { device_id, .. } => *device_id,
            PendingNotification::StatusChanged { device_id, .. } => *device_id,
        }
    }
}

/// Notification aggregator state
///
/// Collects notifications over a short window and emits them as batches
/// or individual notifications depending on volume.
pub struct NotificationAggregator {
    /// Pending notifications keyed by device ID (for deduplication)
    pending: HashMap<DeviceId, PendingNotification>,
    /// Order of device IDs for stable ordering
    pending_order: Vec<DeviceId>,
    /// When the current aggregation window started
    window_start: Option<Instant>,
}

impl Default for NotificationAggregator {
    fn default() -> Self {
        Self::new()
    }
}

impl NotificationAggregator {
    /// Create a new notification aggregator
    pub fn new() -> Self {
        Self {
            pending: HashMap::new(),
            pending_order: Vec::new(),
            window_start: None,
        }
    }

    /// Add a notification to the pending queue
    ///
    /// Notifications for the same device are deduplicated - only the latest
    /// notification for a given device ID is kept. This handles rapid
    /// arrival/removal cycles by keeping only the final state.
    pub fn add(&mut self, notification: PendingNotification) {
        let device_id = notification.device_id();

        // Start window if this is the first notification
        if self.window_start.is_none() {
            self.window_start = Some(Instant::now());
        }

        // Track order if this is a new device
        if !self.pending.contains_key(&device_id) {
            self.pending_order.push(device_id);
        }

        // Insert/replace the notification for this device
        // This provides natural deduplication - rapid plug/unplug results
        // in only the final state being sent
        self.pending.insert(device_id, notification);

        debug!(
            "Aggregator: added notification for device {:?}, {} pending",
            device_id,
            self.pending.len()
        );
    }

    /// Check if the aggregation window has expired or if we should flush
    pub fn should_flush(&self) -> bool {
        if self.pending.is_empty() {
            return false;
        }

        // Force flush if too many notifications pending
        if self.pending.len() >= MAX_PENDING_NOTIFICATIONS {
            return true;
        }

        // Check window expiry
        if let Some(start) = self.window_start {
            return start.elapsed() >= AGGREGATION_WINDOW;
        }

        false
    }

    /// Check if there are any pending notifications
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Flush pending notifications and return them
    ///
    /// Returns `None` if there are no pending notifications.
    /// Returns `Some(Vec<...>)` with the aggregated notifications.
    pub fn flush(&mut self) -> Option<Vec<AggregatedNotification>> {
        if self.pending.is_empty() {
            return None;
        }

        // Collect notifications in order
        let notifications: Vec<AggregatedNotification> = self
            .pending_order
            .iter()
            .filter_map(|id| self.pending.get(id))
            .map(|pending| match pending {
                PendingNotification::Arrived(device) => {
                    AggregatedNotification::Arrived(device.clone())
                }
                PendingNotification::Removed {
                    device_id,
                    invalidated_handles,
                    reason,
                } => AggregatedNotification::Removed {
                    device_id: *device_id,
                    invalidated_handles: invalidated_handles.clone(),
                    reason: reason.clone(),
                },
                PendingNotification::StatusChanged {
                    device_id,
                    device_info,
                    reason,
                } => AggregatedNotification::StatusChanged {
                    device_id: *device_id,
                    device_info: device_info.clone(),
                    reason: reason.clone(),
                },
            })
            .collect();

        debug!("Aggregator: flushing {} notifications", notifications.len());

        // Clear state
        self.pending.clear();
        self.pending_order.clear();
        self.window_start = None;

        Some(notifications)
    }

    /// Time until the aggregation window expires
    ///
    /// Returns `None` if no window is active.
    pub fn time_until_flush(&self) -> Option<Duration> {
        self.window_start.map(|start| {
            let elapsed = start.elapsed();
            if elapsed >= AGGREGATION_WINDOW {
                Duration::ZERO
            } else {
                AGGREGATION_WINDOW - elapsed
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::DeviceSpeed;

    fn mock_device_info(id: u32) -> DeviceInfo {
        DeviceInfo {
            id: DeviceId(id),
            vendor_id: 0x1234,
            product_id: 0x5678,
            bus_number: 1,
            device_address: id as u8,
            manufacturer: Some("Test".to_string()),
            product: Some("Test Device".to_string()),
            serial_number: None,
            class: 0,
            subclass: 0,
            protocol: 0,
            speed: DeviceSpeed::High,
            num_configurations: 1,
        }
    }

    #[test]
    fn test_aggregator_empty() {
        let mut agg = NotificationAggregator::new();
        assert!(!agg.should_flush());
        assert!(!agg.has_pending());
        assert!(agg.flush().is_none());
    }

    #[test]
    fn test_aggregator_single_notification() {
        let mut agg = NotificationAggregator::new();
        let device = mock_device_info(1);

        agg.add(PendingNotification::Arrived(device.clone()));

        assert!(agg.has_pending());

        // Wait for window to expire
        std::thread::sleep(Duration::from_millis(150));
        assert!(agg.should_flush());

        let batch = agg.flush();
        assert!(batch.is_some());
        let notifications = batch.unwrap();
        assert_eq!(notifications.len(), 1);

        match &notifications[0] {
            AggregatedNotification::Arrived(d) => {
                assert_eq!(d.id.0, 1);
            }
            _ => panic!("Expected Arrived notification"),
        }
    }

    #[test]
    fn test_aggregator_deduplication() {
        let mut agg = NotificationAggregator::new();

        // Add arrival, then removal for same device
        agg.add(PendingNotification::Arrived(mock_device_info(1)));
        agg.add(PendingNotification::Removed {
            device_id: DeviceId(1),
            invalidated_handles: vec![],
            reason: DeviceRemovalReason::Unplugged,
        });

        // Should only have one notification (the removal)
        std::thread::sleep(Duration::from_millis(150));
        let batch = agg.flush().unwrap();
        assert_eq!(batch.len(), 1);

        match &batch[0] {
            AggregatedNotification::Removed { device_id, .. } => {
                assert_eq!(device_id.0, 1);
            }
            _ => panic!("Expected Removed notification"),
        }
    }

    #[test]
    fn test_aggregator_multiple_devices() {
        let mut agg = NotificationAggregator::new();

        agg.add(PendingNotification::Arrived(mock_device_info(1)));
        agg.add(PendingNotification::Arrived(mock_device_info(2)));
        agg.add(PendingNotification::Arrived(mock_device_info(3)));

        std::thread::sleep(Duration::from_millis(150));
        let batch = agg.flush().unwrap();
        assert_eq!(batch.len(), 3);
    }
}
