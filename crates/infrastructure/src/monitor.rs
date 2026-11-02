use std::cell::RefCell;

use swarmux_application::ports::SystemMonitor;
use sysinfo::{MemoryRefreshKind, RefreshKind, System};

/// Free memory as the operating system reports it.
pub struct SysinfoMonitor {
    /// `None` when the platform can't be queried at all.
    system: Option<RefCell<System>>,
}

impl SysinfoMonitor {
    pub fn new() -> SysinfoMonitor {
        let kind = RefreshKind::nothing().with_memory(MemoryRefreshKind::nothing().with_ram());
        SysinfoMonitor {
            system: System::new_with_specifics(kind).ok().map(RefCell::new),
        }
    }
}

impl Default for SysinfoMonitor {
    fn default() -> SysinfoMonitor {
        SysinfoMonitor::new()
    }
}

impl SystemMonitor for SysinfoMonitor {
    fn free_memory_bytes(&self) -> Option<u64> {
        let mut system = self.system.as_ref()?.borrow_mut();
        system.refresh_memory_specifics(MemoryRefreshKind::nothing().with_ram());
        // 0 means the platform didn't report it.
        Some(system.available_memory()).filter(|&free| free > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_some_free_memory() {
        let free = SysinfoMonitor::new().free_memory_bytes();
        assert!(free.is_some_and(|f| f > 1024 * 1024), "{free:?}");
    }
}
