//! btop-style machine readings through `sysinfo`: CPU, memory, network,
//! disk, uptime and Claude Code processes.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use swarmux_application::{DiskUsage, ProcessStat, SystemStats};
use sysinfo::{
    CpuRefreshKind, Disks, MemoryRefreshKind, Networks, ProcessRefreshKind, ProcessesToUpdate,
    RefreshKind, System,
};

use super::battery;

/// Samples kept for the history graphs.
pub const HISTORY: usize = 60;
/// Processes are listed less often: enumerating them is the costly part.
const PROCESS_EVERY: Duration = Duration::from_secs(5);
const DISK_EVERY: Duration = Duration::from_secs(30);
const BATTERY_EVERY: Duration = Duration::from_secs(30);

/// Keeps the sysinfo handles and histories between readings.
pub struct SystemCollector {
    sys: System,
    networks: Option<Networks>,
    home: PathBuf,
    cpu_history: VecDeque<f64>,
    core_history: Vec<VecDeque<f64>>,
    net_history: VecDeque<(u64, u64)>,
    last_net: Option<Instant>,
    procs_at: Option<Instant>,
    procs: Vec<ProcessStat>,
    disk_at: Option<Instant>,
    disk: Option<DiskUsage>,
    battery_at: Option<Instant>,
    battery: Option<swarmux_application::Battery>,
}

impl SystemCollector {
    /// `None` when the platform can't be queried at all.
    pub fn new(home: &Path) -> Option<SystemCollector> {
        let kind = RefreshKind::nothing()
            .with_cpu(CpuRefreshKind::nothing().with_usage())
            .with_memory(MemoryRefreshKind::everything());
        let sys = System::new_with_specifics(kind).ok()?;
        Some(SystemCollector {
            sys,
            networks: Networks::new_with_refreshed_list().ok(),
            home: home.to_path_buf(),
            cpu_history: VecDeque::new(),
            core_history: Vec::new(),
            net_history: VecDeque::new(),
            last_net: Some(Instant::now()),
            procs_at: None,
            procs: Vec::new(),
            disk_at: None,
            disk: None,
            battery_at: None,
            battery: None,
        })
    }

    /// Takes a reading. CPU figures need two readings, so the first is 0.
    pub fn collect(&mut self) -> SystemStats {
        let now = Instant::now();
        self.sys.refresh_cpu_usage();
        self.sys.refresh_memory();
        let cpu = f64::from(self.sys.global_cpu_usage());
        let cores: Vec<f64> = self
            .sys
            .cpus()
            .iter()
            .map(|c| f64::from(c.usage()))
            .collect();
        push(&mut self.cpu_history, cpu);
        self.core_history.resize_with(cores.len(), VecDeque::new);
        for (h, &c) in self.core_history.iter_mut().zip(&cores) {
            push(h, c);
        }

        let (rx, tx) = self.network_rate(now);
        if let (Some(rx), Some(tx)) = (rx, tx) {
            push(&mut self.net_history, (rx, tx));
        }
        if due(self.procs_at, PROCESS_EVERY, now) {
            self.procs = self.claude_processes();
            self.procs_at = Some(now);
        }
        if due(self.disk_at, DISK_EVERY, now) {
            self.disk = home_disk(&self.home);
            self.disk_at = Some(now);
        }
        if due(self.battery_at, BATTERY_EVERY, now) {
            self.battery = battery::read();
            self.battery_at = Some(now);
        }
        SystemStats {
            cpu,
            cores,
            cpu_history: self.cpu_history.iter().copied().collect(),
            core_history: self
                .core_history
                .iter()
                .map(|h| h.iter().copied().collect())
                .collect(),
            mem_used: self.sys.used_memory(),
            mem_total: self.sys.total_memory(),
            swap_used: self.sys.used_swap(),
            swap_total: self.sys.total_swap(),
            net_rx_per_sec: rx,
            net_tx_per_sec: tx,
            net_history: self.net_history.iter().copied().collect(),
            disk: self.disk.clone(),
            battery: self.battery.clone(),
            uptime_secs: System::uptime().unwrap_or(0),
            claude_processes: self.procs.clone(),
        }
    }

    /// Bytes per second received and sent since the previous reading.
    fn network_rate(&mut self, now: Instant) -> (Option<u64>, Option<u64>) {
        let Some(networks) = self.networks.as_mut() else {
            return (None, None);
        };
        networks.refresh(true);
        let (rx, tx) = networks.values().fold((0u64, 0u64), |(r, t), n| {
            (r + n.received(), t + n.transmitted())
        });
        let secs = self
            .last_net
            .map(|at| now.duration_since(at).as_secs_f64())
            .filter(|s| *s > 0.05);
        self.last_net = Some(now);
        match secs {
            Some(s) => (Some((rx as f64 / s) as u64), Some((tx as f64 / s) as u64)),
            None => (None, None),
        }
    }

    /// Claude Code processes (`claude`, `claude.exe`), most memory first.
    fn claude_processes(&mut self) -> Vec<ProcessStat> {
        self.sys.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing().with_cpu().with_memory(),
        );
        let mut out: Vec<ProcessStat> = self
            .sys
            .processes()
            .values()
            .filter_map(|p| {
                let name = p.name().to_string_lossy();
                is_claude(&name).then(|| ProcessStat {
                    pid: p.pid().as_u32(),
                    name: name.into_owned(),
                    rss: p.memory(),
                    cpu: f64::from(p.cpu_usage()),
                })
            })
            .collect();
        out.sort_by(|a, b| b.rss.cmp(&a.rss).then(a.pid.cmp(&b.pid)));
        out
    }
}

fn push<T>(h: &mut VecDeque<T>, v: T) {
    h.push_back(v);
    while h.len() > HISTORY {
        h.pop_front();
    }
}

fn due(last: Option<Instant>, every: Duration, now: Instant) -> bool {
    last.is_none_or(|at| now.duration_since(at) >= every)
}

/// `claude` or `claude.exe`, any case.
pub fn is_claude(process_name: &str) -> bool {
    let n = process_name.to_ascii_lowercase();
    n == "claude" || n == "claude.exe"
}

/// The disk whose mount point holds `home` (the longest matching one).
fn home_disk(home: &Path) -> Option<DiskUsage> {
    let disks = Disks::new_with_refreshed_list().ok()?;
    let home = normalize(home);
    disks
        .iter()
        .filter(|d| home.starts_with(&normalize(d.mount_point())))
        .max_by_key(|d| d.mount_point().as_os_str().len())
        .map(|d| DiskUsage {
            mount: d.mount_point().display().to_string(),
            used: d.total_space().saturating_sub(d.available_space()),
            total: d.total_space(),
        })
}

/// Lower case with `/` separators, so `C:\` matches `c:/Users`.
fn normalize(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    if cfg!(windows) {
        s.to_ascii_lowercase()
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_process_names() {
        assert!(is_claude("claude.exe"));
        assert!(is_claude("Claude"));
        assert!(!is_claude("claude-helper"));
        assert!(!is_claude("node"));
    }

    #[test]
    fn readings_fill_histories() {
        let home = std::env::temp_dir();
        let Some(mut c) = SystemCollector::new(&home) else {
            return;
        };
        let first = c.collect();
        std::thread::sleep(Duration::from_millis(250));
        let second = c.collect();
        assert!(second.mem_total > 0 && second.mem_used <= second.mem_total);
        assert!(!second.cores.is_empty());
        assert_eq!(second.cpu_history.len(), 2);
        assert_eq!(second.core_history.len(), second.cores.len());
        assert!((0.0..=100.0).contains(&second.cpu));
        assert!(second.net_rx_per_sec.is_some());
        assert!(second.uptime_secs > 0);
        assert!(
            first.disk.as_ref().is_some_and(|d| d.total >= d.used),
            "{:?}",
            first.disk
        );
    }

    #[test]
    fn history_is_capped() {
        let mut h = VecDeque::new();
        for i in 0..100 {
            push(&mut h, i);
        }
        assert_eq!(h.len(), HISTORY);
        assert_eq!(h.front(), Some(&40));
    }
}
