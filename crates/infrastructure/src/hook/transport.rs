//! The local socket: a named pipe on Windows, a Unix domain socket elsewhere.

use std::io;
use std::time::Duration;

/// Environment variable naming the socket, set on every `claude` child.
pub const ENV_PIPE: &str = "SWARMUX_HOOK_PIPE";
/// Environment variable with the job id, set on every `claude` child.
pub const ENV_JOB: &str = "SWARMUX_JOB_ID";

/// The user name, reduced to characters safe in a pipe or file name.
pub(crate) fn user() -> String {
    let raw = std::env::var(if cfg!(windows) { "USERNAME" } else { "USER" }).unwrap_or_default();
    let safe: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        .take(32)
        .collect();
    if safe.is_empty() { "user".into() } else { safe }
}

/// The socket name for the swarmux process `pid`.
pub fn endpoint_for(pid: u32) -> String {
    #[cfg(windows)]
    {
        format!(r"\\.\pipe\swarmux-{}-{pid}", user())
    }
    #[cfg(not(windows))]
    {
        std::env::temp_dir()
            .join(format!("swarmux-{}-{pid}.sock", user()))
            .to_string_lossy()
            .into_owned()
    }
}

/// How long a client keeps retrying a busy pipe.
const BUSY_WAIT: Duration = Duration::from_secs(2);

/// Connects to the app's socket.
#[cfg(windows)]
pub fn connect(name: &str) -> io::Result<std::fs::File> {
    const ERROR_PIPE_BUSY: i32 = 231;
    let start = std::time::Instant::now();
    loop {
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(name)
        {
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) && start.elapsed() < BUSY_WAIT => {
                std::thread::sleep(Duration::from_millis(2));
            }
            other => return other,
        }
    }
}

/// Connects to the app's socket.
#[cfg(not(windows))]
pub fn connect(name: &str) -> io::Result<std::os::unix::net::UnixStream> {
    let _ = BUSY_WAIT;
    std::os::unix::net::UnixStream::connect(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_names_are_per_process() {
        let a = endpoint_for(1);
        assert_ne!(a, endpoint_for(2));
        assert!(a.contains("swarmux-"));
        #[cfg(windows)]
        assert!(a.starts_with(r"\\.\pipe\"));
    }

    #[test]
    fn missing_socket_fails_fast() {
        let start = std::time::Instant::now();
        assert!(connect(&endpoint_for(u32::MAX)).is_err());
        assert!(start.elapsed() < Duration::from_secs(1));
    }
}
