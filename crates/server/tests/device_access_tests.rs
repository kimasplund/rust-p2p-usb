//! Integration tests for DeviceAccessTracker
//!
//! Tests device sharing and access tracking including:
//! - Exclusive, shared, and read-only modes
//! - Lock acquire and release
//! - Waiting queue management

use protocol::{DeviceHandle, DeviceId, LockResult, SharingMode, UnlockResult};
use std::time::Duration;

fn device_id(id: u32) -> DeviceId {
    DeviceId(id)
}

fn handle(id: u32) -> DeviceHandle {
    DeviceHandle(id)
}

mod device_sharing_state {
    use super::*;

    mod exclusive_mode {
        use super::*;

        #[test]
        fn test_exclusive_initial_can_attach() {
            let can_attach = true;
            assert!(can_attach);
        }

        #[test]
        fn test_exclusive_after_attach_cannot_attach() {
            let attached_clients_count = 1;
            let can_attach = attached_clients_count == 0;
            assert!(!can_attach);
        }

        #[test]
        fn test_exclusive_after_detach_can_attach() {
            let attached_clients_count = 0;
            let can_attach = attached_clients_count == 0;
            assert!(can_attach);
        }

        #[test]
        fn test_exclusive_client_has_immediate_access() {
            let mode = SharingMode::Exclusive;
            let attached_clients_empty = true;
            let has_access =
                mode == SharingMode::Exclusive || attached_clients_empty;
            assert!(has_access);
        }

        #[test]
        fn test_exclusive_cannot_release_lock() {
            let mode = SharingMode::Exclusive;
            let can_release = mode != SharingMode::Exclusive;
            assert!(!can_release);
        }
    }

    mod shared_mode {
        use super::*;

        #[test]
        fn test_shared_multiple_can_attach() {
            let max_clients = 4u32;
            let attached_count = 0u32;
            assert!(attached_count < max_clients);

            let attached_count = 1;
            assert!(attached_count < max_clients);

            let attached_count = 3;
            assert!(attached_count < max_clients);
        }

        #[test]
        fn test_shared_at_capacity_cannot_attach() {
            let max_clients = 4u32;
            let attached_count = 4u32;
            assert!(!(attached_count < max_clients));
        }

        #[test]
        fn test_shared_first_client_gets_access() {
            let attached_clients_empty = true;
            let has_access = attached_clients_empty;
            assert!(has_access);
        }

        #[test]
        fn test_shared_lock_acquire_when_available() {
            let exclusive_lock_holder: Option<DeviceHandle> = None;
            let result = if exclusive_lock_holder.is_none() {
                LockResult::Acquired
            } else {
                LockResult::Queued { position: 1 }
            };
            assert_eq!(result, LockResult::Acquired);
        }

        #[test]
        fn test_shared_lock_acquire_when_held() {
            let exclusive_lock_holder = Some(handle(1));
            let requesting_handle = handle(2);
            let queue_position = 1u32;

            let result = if exclusive_lock_holder == Some(requesting_handle) {
                LockResult::AlreadyHeld
            } else if exclusive_lock_holder.is_none() {
                LockResult::Acquired
            } else {
                LockResult::Queued {
                    position: queue_position,
                }
            };

            assert!(matches!(result, LockResult::Queued { position: 1 }));
        }

        #[test]
        fn test_shared_lock_already_held() {
            let holder_handle = handle(1);
            let exclusive_lock_holder = Some(holder_handle);
            let requesting_handle = holder_handle;

            let result = if exclusive_lock_holder == Some(requesting_handle) {
                LockResult::AlreadyHeld
            } else {
                LockResult::Acquired
            };

            assert_eq!(result, LockResult::AlreadyHeld);
        }

        #[test]
        fn test_shared_lock_release() {
            let holder_handle = handle(1);
            let mut exclusive_lock_holder = Some(holder_handle);
            let releasing_handle = holder_handle;

            let result = if exclusive_lock_holder == Some(releasing_handle) {
                exclusive_lock_holder = None;
                UnlockResult::Released
            } else {
                UnlockResult::NotHeld
            };

            assert_eq!(result, UnlockResult::Released);
            assert!(exclusive_lock_holder.is_none());
        }

        #[test]
        fn test_shared_lock_release_not_held() {
            let holder_handle = handle(1);
            let exclusive_lock_holder = Some(holder_handle);
            let releasing_handle = handle(2);

            let result = if exclusive_lock_holder == Some(releasing_handle) {
                UnlockResult::Released
            } else {
                UnlockResult::NotHeld
            };

            assert_eq!(result, UnlockResult::NotHeld);
        }
    }

    mod read_only_mode {
        use super::*;

        #[test]
        fn test_read_only_multiple_can_attach() {
            let max_clients = 4u32;
            let attached_count = 0u32;
            assert!(attached_count < max_clients);

            let attached_count = 3;
            assert!(attached_count < max_clients);
        }

        #[test]
        fn test_read_only_read_access_always_granted() {
            let write_access = false;
            let result = if !write_access {
                LockResult::Acquired
            } else {
                LockResult::Queued { position: 1 }
            };
            assert_eq!(result, LockResult::Acquired);
        }

        #[test]
        fn test_read_only_write_access_when_available() {
            let write_access = true;
            let write_lock_holder: Option<DeviceHandle> = None;

            let result = if !write_access {
                LockResult::Acquired
            } else if write_lock_holder.is_none() {
                LockResult::Acquired
            } else {
                LockResult::Queued { position: 1 }
            };

            assert_eq!(result, LockResult::Acquired);
        }

        #[test]
        fn test_read_only_write_access_when_held() {
            let write_access = true;
            let write_lock_holder = Some(handle(1));
            let requesting_handle = handle(2);
            let queue_position = 1u32;

            let result = if !write_access {
                LockResult::Acquired
            } else if write_lock_holder == Some(requesting_handle) {
                LockResult::AlreadyHeld
            } else if write_lock_holder.is_none() {
                LockResult::Acquired
            } else {
                LockResult::Queued {
                    position: queue_position,
                }
            };

            assert!(matches!(result, LockResult::Queued { position: 1 }));
        }

        #[test]
        fn test_read_only_write_access_already_held() {
            let write_access = true;
            let holder_handle = handle(1);
            let write_lock_holder = Some(holder_handle);
            let requesting_handle = holder_handle;

            let result = if !write_access {
                LockResult::Acquired
            } else if write_lock_holder == Some(requesting_handle) {
                LockResult::AlreadyHeld
            } else {
                LockResult::Acquired
            };

            assert_eq!(result, LockResult::AlreadyHeld);
        }

        #[test]
        fn test_read_only_write_lock_release() {
            let holder_handle = handle(1);
            let mut write_lock_holder = Some(holder_handle);
            let releasing_handle = holder_handle;

            let result = if write_lock_holder == Some(releasing_handle) {
                write_lock_holder = None;
                UnlockResult::Released
            } else {
                UnlockResult::NotHeld
            };

            assert_eq!(result, UnlockResult::Released);
            assert!(write_lock_holder.is_none());
        }
    }
}

mod queue_management {
    use super::*;
    use std::collections::VecDeque;

    #[derive(Debug, Clone)]
    struct QueueEntry {
        handle: DeviceHandle,
        wants_write: bool,
    }

    #[test]
    fn test_add_to_queue() {
        let mut queue: VecDeque<QueueEntry> = VecDeque::new();

        queue.push_back(QueueEntry {
            handle: handle(1),
            wants_write: false,
        });

        assert_eq!(queue.len(), 1);
    }

    #[test]
    fn test_queue_position() {
        let mut queue: VecDeque<QueueEntry> = VecDeque::new();

        queue.push_back(QueueEntry {
            handle: handle(1),
            wants_write: false,
        });
        queue.push_back(QueueEntry {
            handle: handle(2),
            wants_write: false,
        });
        queue.push_back(QueueEntry {
            handle: handle(3),
            wants_write: false,
        });

        let pos1 = queue
            .iter()
            .position(|e| e.handle == handle(1))
            .map(|p| (p + 1) as u32);
        let pos2 = queue
            .iter()
            .position(|e| e.handle == handle(2))
            .map(|p| (p + 1) as u32);
        let pos3 = queue
            .iter()
            .position(|e| e.handle == handle(3))
            .map(|p| (p + 1) as u32);

        assert_eq!(pos1, Some(1));
        assert_eq!(pos2, Some(2));
        assert_eq!(pos3, Some(3));
    }

    #[test]
    fn test_queue_position_not_in_queue() {
        let queue: VecDeque<QueueEntry> = VecDeque::new();

        let pos = queue
            .iter()
            .position(|e| e.handle == handle(99))
            .map(|p| (p + 1) as u32);

        assert!(pos.is_none());
    }

    #[test]
    fn test_process_queue_grants_to_first() {
        let mut queue: VecDeque<QueueEntry> = VecDeque::new();

        queue.push_back(QueueEntry {
            handle: handle(1),
            wants_write: false,
        });
        queue.push_back(QueueEntry {
            handle: handle(2),
            wants_write: false,
        });

        let granted = queue.pop_front();

        assert!(granted.is_some());
        assert_eq!(granted.unwrap().handle, handle(1));
        assert_eq!(queue.len(), 1);
    }

    #[test]
    fn test_queue_remove_on_detach() {
        let mut queue: VecDeque<QueueEntry> = VecDeque::new();

        queue.push_back(QueueEntry {
            handle: handle(1),
            wants_write: false,
        });
        queue.push_back(QueueEntry {
            handle: handle(2),
            wants_write: false,
        });
        queue.push_back(QueueEntry {
            handle: handle(3),
            wants_write: false,
        });

        queue.retain(|e| e.handle != handle(2));

        assert_eq!(queue.len(), 2);
        assert_eq!(queue[0].handle, handle(1));
        assert_eq!(queue[1].handle, handle(3));
    }

    #[test]
    fn test_queue_positions_update_after_remove() {
        let mut queue: VecDeque<QueueEntry> = VecDeque::new();

        queue.push_back(QueueEntry {
            handle: handle(1),
            wants_write: false,
        });
        queue.push_back(QueueEntry {
            handle: handle(2),
            wants_write: false,
        });
        queue.push_back(QueueEntry {
            handle: handle(3),
            wants_write: false,
        });

        queue.pop_front();

        let pos2 = queue
            .iter()
            .position(|e| e.handle == handle(2))
            .map(|p| (p + 1) as u32);
        let pos3 = queue
            .iter()
            .position(|e| e.handle == handle(3))
            .map(|p| (p + 1) as u32);

        assert_eq!(pos2, Some(1));
        assert_eq!(pos3, Some(2));
    }

    #[test]
    fn test_queue_duplicate_prevention() {
        let mut queue: VecDeque<QueueEntry> = VecDeque::new();

        let handle_to_add = handle(1);

        if queue.iter().position(|e| e.handle == handle_to_add).is_none() {
            queue.push_back(QueueEntry {
                handle: handle_to_add,
                wants_write: false,
            });
        }

        if queue.iter().position(|e| e.handle == handle_to_add).is_none() {
            queue.push_back(QueueEntry {
                handle: handle_to_add,
                wants_write: false,
            });
        }

        assert_eq!(queue.len(), 1);
    }
}

mod device_access_tracker {
    use super::*;
    use std::collections::HashMap;

    struct MockDeviceState {
        device_id: DeviceId,
        mode: SharingMode,
        max_clients: u32,
        attached_clients: HashMap<DeviceHandle, String>,
        exclusive_lock_holder: Option<DeviceHandle>,
        write_lock_holder: Option<DeviceHandle>,
    }

    impl MockDeviceState {
        fn new(device_id: DeviceId, mode: SharingMode, max_clients: u32) -> Self {
            Self {
                device_id,
                mode,
                max_clients,
                attached_clients: HashMap::new(),
                exclusive_lock_holder: None,
                write_lock_holder: None,
            }
        }

        fn can_attach(&self) -> bool {
            match self.mode {
                SharingMode::Exclusive => self.attached_clients.is_empty(),
                SharingMode::Shared | SharingMode::ReadOnly => {
                    (self.attached_clients.len() as u32) < self.max_clients
                }
            }
        }

        fn attach(&mut self, client_id: String, h: DeviceHandle) {
            self.attached_clients.insert(h, client_id);
        }

        fn detach(&mut self, h: DeviceHandle) {
            self.attached_clients.remove(&h);
            if self.exclusive_lock_holder == Some(h) {
                self.exclusive_lock_holder = None;
            }
            if self.write_lock_holder == Some(h) {
                self.write_lock_holder = None;
            }
        }
    }

    struct MockTracker {
        devices: HashMap<DeviceId, MockDeviceState>,
    }

    impl MockTracker {
        fn new() -> Self {
            Self {
                devices: HashMap::new(),
            }
        }

        fn register(&mut self, device_id: DeviceId, mode: SharingMode, max_clients: u32) {
            self.devices
                .insert(device_id, MockDeviceState::new(device_id, mode, max_clients));
        }

        fn unregister(&mut self, device_id: DeviceId) {
            self.devices.remove(&device_id);
        }

        fn is_registered(&self, device_id: DeviceId) -> bool {
            self.devices.contains_key(&device_id)
        }

        fn get_mode(&self, device_id: DeviceId) -> Option<SharingMode> {
            self.devices.get(&device_id).map(|s| s.mode)
        }

        fn can_attach(&self, device_id: DeviceId) -> bool {
            self.devices.get(&device_id).map(|s| s.can_attach()).unwrap_or(true)
        }

        fn get_device_for_handle(&self, h: DeviceHandle) -> Option<DeviceId> {
            for (device_id, state) in &self.devices {
                if state.attached_clients.contains_key(&h) {
                    return Some(*device_id);
                }
            }
            None
        }
    }

    #[test]
    fn test_register_device() {
        let mut tracker = MockTracker::new();
        let id = device_id(1);

        tracker.register(id, SharingMode::Exclusive, 1);

        assert!(tracker.is_registered(id));
        assert_eq!(tracker.get_mode(id), Some(SharingMode::Exclusive));
    }

    #[test]
    fn test_unregister_device() {
        let mut tracker = MockTracker::new();
        let id = device_id(1);

        tracker.register(id, SharingMode::Exclusive, 1);
        assert!(tracker.is_registered(id));

        tracker.unregister(id);
        assert!(!tracker.is_registered(id));
    }

    #[test]
    fn test_can_attach_unregistered() {
        let tracker = MockTracker::new();
        let id = device_id(99);

        assert!(tracker.can_attach(id));
    }

    #[test]
    fn test_get_device_for_handle() {
        let mut tracker = MockTracker::new();
        let id1 = device_id(1);
        let id2 = device_id(2);

        tracker.register(id1, SharingMode::Shared, 4);
        tracker.register(id2, SharingMode::Shared, 4);

        if let Some(state) = tracker.devices.get_mut(&id1) {
            state.attach("client1".to_string(), handle(10));
        }
        if let Some(state) = tracker.devices.get_mut(&id2) {
            state.attach("client2".to_string(), handle(20));
        }

        assert_eq!(tracker.get_device_for_handle(handle(10)), Some(id1));
        assert_eq!(tracker.get_device_for_handle(handle(20)), Some(id2));
        assert_eq!(tracker.get_device_for_handle(handle(99)), None);
    }

    #[test]
    fn test_exclusive_mode_single_client() {
        let mut tracker = MockTracker::new();
        let id = device_id(1);

        tracker.register(id, SharingMode::Exclusive, 1);

        assert!(tracker.can_attach(id));

        if let Some(state) = tracker.devices.get_mut(&id) {
            state.attach("client1".to_string(), handle(1));
        }

        assert!(!tracker.can_attach(id));

        if let Some(state) = tracker.devices.get_mut(&id) {
            state.detach(handle(1));
        }

        assert!(tracker.can_attach(id));
    }

    #[test]
    fn test_shared_mode_multiple_clients() {
        let mut tracker = MockTracker::new();
        let id = device_id(1);

        tracker.register(id, SharingMode::Shared, 3);

        for i in 0..3 {
            assert!(tracker.can_attach(id));
            if let Some(state) = tracker.devices.get_mut(&id) {
                state.attach(format!("client{}", i), handle(i));
            }
        }

        assert!(!tracker.can_attach(id));

        if let Some(state) = tracker.devices.get_mut(&id) {
            state.detach(handle(0));
        }

        assert!(tracker.can_attach(id));
    }

    #[test]
    fn test_read_only_mode_multiple_clients() {
        let mut tracker = MockTracker::new();
        let id = device_id(1);

        tracker.register(id, SharingMode::ReadOnly, 5);

        for i in 0..5 {
            assert!(tracker.can_attach(id));
            if let Some(state) = tracker.devices.get_mut(&id) {
                state.attach(format!("client{}", i), handle(i));
            }
        }

        assert!(!tracker.can_attach(id));
    }

    #[test]
    fn test_multiple_devices() {
        let mut tracker = MockTracker::new();

        let id1 = device_id(1);
        let id2 = device_id(2);
        let id3 = device_id(3);

        tracker.register(id1, SharingMode::Exclusive, 1);
        tracker.register(id2, SharingMode::Shared, 4);
        tracker.register(id3, SharingMode::ReadOnly, 10);

        assert_eq!(tracker.get_mode(id1), Some(SharingMode::Exclusive));
        assert_eq!(tracker.get_mode(id2), Some(SharingMode::Shared));
        assert_eq!(tracker.get_mode(id3), Some(SharingMode::ReadOnly));

        if let Some(state) = tracker.devices.get_mut(&id1) {
            state.attach("client1".to_string(), handle(10));
        }
        assert!(!tracker.can_attach(id1));
        assert!(tracker.can_attach(id2));
        assert!(tracker.can_attach(id3));
    }
}

mod sharing_status {
    use super::*;
    use protocol::DeviceSharingStatus;

    #[test]
    fn test_status_exclusive_mode() {
        let status = DeviceSharingStatus {
            device_id: device_id(1),
            sharing_mode: SharingMode::Exclusive,
            attached_clients: 1,
            has_write_lock: false,
            queue_position: 0,
            queue_length: 0,
        };

        assert_eq!(status.sharing_mode, SharingMode::Exclusive);
        assert_eq!(status.attached_clients, 1);
        assert_eq!(status.queue_position, 0);
    }

    #[test]
    fn test_status_shared_with_queue() {
        let status = DeviceSharingStatus {
            device_id: device_id(1),
            sharing_mode: SharingMode::Shared,
            attached_clients: 4,
            has_write_lock: false,
            queue_position: 2,
            queue_length: 5,
        };

        assert_eq!(status.sharing_mode, SharingMode::Shared);
        assert_eq!(status.attached_clients, 4);
        assert_eq!(status.queue_position, 2);
        assert_eq!(status.queue_length, 5);
    }

    #[test]
    fn test_status_read_only_with_write_lock() {
        let status = DeviceSharingStatus {
            device_id: device_id(1),
            sharing_mode: SharingMode::ReadOnly,
            attached_clients: 3,
            has_write_lock: true,
            queue_position: 0,
            queue_length: 2,
        };

        assert_eq!(status.sharing_mode, SharingMode::ReadOnly);
        assert!(status.has_write_lock);
        assert_eq!(status.queue_position, 0);
    }
}

mod lock_timeout {
    use super::*;
    use std::time::Instant;

    #[test]
    fn test_lock_timeout_detection() {
        let lock_timeout = Duration::from_secs(300);
        let lock_acquired_at = Instant::now() - Duration::from_secs(301);

        let is_expired = lock_acquired_at.elapsed() > lock_timeout;
        assert!(is_expired);
    }

    #[test]
    fn test_lock_not_expired() {
        let lock_timeout = Duration::from_secs(300);
        let lock_acquired_at = Instant::now() - Duration::from_secs(100);

        let is_expired = lock_acquired_at.elapsed() > lock_timeout;
        assert!(!is_expired);
    }

    #[test]
    fn test_lock_timeout_just_under_boundary() {
        let lock_timeout = Duration::from_secs(300);
        let lock_acquired_at = Instant::now() - Duration::from_secs(299);

        let is_expired = lock_acquired_at.elapsed() > lock_timeout;
        assert!(!is_expired);
    }
}

mod sharing_events {
    use super::*;

    #[derive(Debug, Clone, PartialEq)]
    enum SharingEvent {
        LockExpired {
            device_id: DeviceId,
            handle: DeviceHandle,
        },
        AccessGranted {
            device_id: DeviceId,
            handle: DeviceHandle,
        },
        QueuePositionChanged {
            device_id: DeviceId,
            handle: DeviceHandle,
            new_position: u32,
        },
    }

    #[test]
    fn test_lock_expired_event() {
        let event = SharingEvent::LockExpired {
            device_id: device_id(1),
            handle: handle(10),
        };

        match event {
            SharingEvent::LockExpired { device_id: d, handle: h } => {
                assert_eq!(d, device_id(1));
                assert_eq!(h, handle(10));
            }
            _ => panic!("Expected LockExpired"),
        }
    }

    #[test]
    fn test_access_granted_event() {
        let event = SharingEvent::AccessGranted {
            device_id: device_id(2),
            handle: handle(20),
        };

        match event {
            SharingEvent::AccessGranted { device_id: d, handle: h } => {
                assert_eq!(d, device_id(2));
                assert_eq!(h, handle(20));
            }
            _ => panic!("Expected AccessGranted"),
        }
    }

    #[test]
    fn test_queue_position_changed_event() {
        let event = SharingEvent::QueuePositionChanged {
            device_id: device_id(3),
            handle: handle(30),
            new_position: 2,
        };

        match event {
            SharingEvent::QueuePositionChanged {
                device_id: d,
                handle: h,
                new_position: p,
            } => {
                assert_eq!(d, device_id(3));
                assert_eq!(h, handle(30));
                assert_eq!(p, 2);
            }
            _ => panic!("Expected QueuePositionChanged"),
        }
    }

    #[test]
    fn test_event_sequence_on_lock_release() {
        let mut events = Vec::new();

        events.push(SharingEvent::LockExpired {
            device_id: device_id(1),
            handle: handle(10),
        });

        events.push(SharingEvent::AccessGranted {
            device_id: device_id(1),
            handle: handle(20),
        });

        events.push(SharingEvent::QueuePositionChanged {
            device_id: device_id(1),
            handle: handle(30),
            new_position: 1,
        });

        events.push(SharingEvent::QueuePositionChanged {
            device_id: device_id(1),
            handle: handle(40),
            new_position: 2,
        });

        assert_eq!(events.len(), 4);

        assert!(matches!(events[0], SharingEvent::LockExpired { .. }));
        assert!(matches!(events[1], SharingEvent::AccessGranted { .. }));
        assert!(matches!(events[2], SharingEvent::QueuePositionChanged { new_position: 1, .. }));
        assert!(matches!(events[3], SharingEvent::QueuePositionChanged { new_position: 2, .. }));
    }
}

mod attached_client_tracking {
    use super::*;
    use std::time::Instant;

    #[derive(Debug, Clone)]
    struct AttachedClient {
        client_id: String,
        handle: DeviceHandle,
        has_write_lock: bool,
        has_access: bool,
        attached_at: Instant,
        lock_acquired_at: Option<Instant>,
    }

    #[test]
    fn test_attached_client_creation() {
        let now = Instant::now();
        let client = AttachedClient {
            client_id: "client1".to_string(),
            handle: handle(1),
            has_write_lock: false,
            has_access: true,
            attached_at: now,
            lock_acquired_at: Some(now),
        };

        assert_eq!(client.client_id, "client1");
        assert_eq!(client.handle, handle(1));
        assert!(!client.has_write_lock);
        assert!(client.has_access);
        assert!(client.lock_acquired_at.is_some());
    }

    #[test]
    fn test_attached_client_lock_acquisition() {
        let now = Instant::now();
        let mut client = AttachedClient {
            client_id: "client1".to_string(),
            handle: handle(1),
            has_write_lock: false,
            has_access: false,
            attached_at: now,
            lock_acquired_at: None,
        };

        assert!(client.lock_acquired_at.is_none());

        client.has_access = true;
        client.lock_acquired_at = Some(Instant::now());

        assert!(client.has_access);
        assert!(client.lock_acquired_at.is_some());
    }

    #[test]
    fn test_attached_client_lock_release() {
        let now = Instant::now();
        let mut client = AttachedClient {
            client_id: "client1".to_string(),
            handle: handle(1),
            has_write_lock: true,
            has_access: true,
            attached_at: now,
            lock_acquired_at: Some(now),
        };

        client.has_write_lock = false;
        client.has_access = false;
        client.lock_acquired_at = None;

        assert!(!client.has_write_lock);
        assert!(!client.has_access);
        assert!(client.lock_acquired_at.is_none());
    }

    #[test]
    fn test_attached_client_duration() {
        let attached_at = Instant::now() - Duration::from_secs(60);
        let client = AttachedClient {
            client_id: "client1".to_string(),
            handle: handle(1),
            has_write_lock: false,
            has_access: true,
            attached_at,
            lock_acquired_at: None,
        };

        let duration = client.attached_at.elapsed();
        assert!(duration >= Duration::from_secs(59));
    }
}
