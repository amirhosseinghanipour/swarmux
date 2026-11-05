//! The background service's adapters: where it listens, its end of the
//! connection and a window's, the lock that keeps it single, starting it
//! detached, and its log.

mod client;
mod lock;
mod log;
#[cfg(windows)]
mod security;
mod server;
mod spawn;
pub mod wire;

use std::path::Path;
use std::sync::Arc;

use swarmux_application::daemon::Shown;
use swarmux_application::{AppResult, Notification, Notifier, TerminalMode};

pub use client::{Connection, WindowClient, ask};
pub use lock::{Holder, LockError, ServiceLock, holder, is_held};
pub use log::{LOG_CAP, ServiceLog, log_path, rotate};
pub use server::WindowServer;
pub use spawn::spawn_detached;

/// Where the service of the data folder `data_dir` listens: one name per
/// user and data folder, the same for every swarmux version.
///
/// On Windows a named pipe; elsewhere `daemon.sock` in the data folder, or
/// a socket in the temporary folder when that path is too long for one.
pub fn endpoint(data_dir: &Path) -> String {
    let path = data_dir.to_string_lossy();
    // Windows paths are the same folder whatever their case.
    let path = if cfg!(windows) {
        path.to_lowercase()
    } else {
        path.into_owned()
    };
    let key = fnv1a(path.as_bytes());
    #[cfg(windows)]
    {
        format!(r"\\.\pipe\swarmux-{}-{key:016x}", crate::hook::user_name())
    }
    #[cfg(not(windows))]
    {
        // `sun_path` holds 104 bytes on macOS, 108 on Linux.
        const SOCKET_PATH_MAX: usize = 100;
        let own = data_dir.join("daemon.sock");
        if own.as_os_str().len() < SOCKET_PATH_MAX {
            return own.to_string_lossy().into_owned();
        }
        std::env::temp_dir()
            .join(format!(
                "swarmux-{}-{key:016x}.sock",
                crate::hook::user_name()
            ))
            .to_string_lossy()
            .into_owned()
    }
}

/// 64-bit FNV-1a: stable across builds, unlike the standard hasher.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// The terminal notification channel of the service: it has no terminal
/// of its own, so the sequence goes to the windows attached, and with none
/// attached the notification is left to the other channels.
pub struct WindowNotifier {
    server: Arc<WindowServer>,
    mode: TerminalMode,
}

impl WindowNotifier {
    pub fn new(server: Arc<WindowServer>, mode: TerminalMode) -> WindowNotifier {
        WindowNotifier { server, mode }
    }
}

impl Notifier for WindowNotifier {
    fn notify(&self, n: &Notification) -> AppResult<()> {
        let seq = crate::notify::terminal_sequence(self.mode, &n.title, &n.body);
        if !seq.is_empty() && self.server.windows() > 0 {
            self.server.broadcast(&Shown::Notify(seq));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_endpoint_per_data_folder_and_stable() {
        let a = endpoint(Path::new("/home/a/.swarmux"));
        assert_eq!(a, endpoint(Path::new("/home/a/.swarmux")));
        assert_ne!(a, endpoint(Path::new("/home/b/.swarmux")));
        // The published FNV-1a test vectors: the name must not change
        // between versions, or a new window would not find an old service.
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
    }
}
