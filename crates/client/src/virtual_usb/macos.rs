//! macOS virtual USB implementation (not yet implemented)
//!
//! # Future Implementation Path
//!
//! macOS support will require using one of the following approaches:
//!
//! 1. **IOKit User Client** (macOS 10.x - 12.x)
//!    - Create a kernel extension (kext) that implements a USB device
//!    - Use IOKit to communicate between userspace and kext
//!    - Requires disabling System Integrity Protection (SIP) for development
//!    - Deprecated in macOS 13+
//!
//! 2. **DriverKit** (macOS 13+, recommended)
//!    - Modern replacement for kernel extensions
//!    - Runs in userspace with restricted privileges
//!    - Requires developer account and entitlements
//!    - Better security model than kexts
//!
//! 3. **USB Gadget Emulation via VM** (workaround)
//!    - Run a Linux VM with USB gadget support
//!    - Forward USB traffic to/from VM
//!    - Lower performance but no kernel development needed
//!
//! # Current Status
//!
//! Phase 5 focuses on Linux implementation. macOS support is deferred to v2.

use anyhow::{Result, anyhow};
use protocol::DeviceHandle;
use std::sync::Arc;

use crate::network::device_proxy::DeviceProxy;

/// macOS virtual USB manager (stub implementation)
pub struct MacOsVirtualUsbManager;

impl MacOsVirtualUsbManager {
    /// Create a new macOS virtual USB manager
    ///
    /// # Errors
    ///
    /// Always returns an error indicating macOS is not yet supported.
    pub async fn new() -> Result<Self> {
        Err(anyhow!(
            "macOS virtual USB support is not yet implemented. \
             Please use Linux for Phase 5. \
             Future implementation will use DriverKit (macOS 13+) or IOKit."
        ))
    }

    /// Attach a device (not implemented)
    pub async fn attach_device(&mut self, _device_proxy: Arc<DeviceProxy>) -> Result<DeviceHandle> {
        Err(anyhow!("macOS virtual USB not implemented"))
    }

    /// Detach a device (not implemented)
    pub async fn detach_device(&mut self, _handle: DeviceHandle) -> Result<()> {
        Err(anyhow!("macOS virtual USB not implemented"))
    }

    /// List devices (not implemented)
    pub async fn list_devices(&self) -> Vec<DeviceHandle> {
        Vec::new()
    }
}
