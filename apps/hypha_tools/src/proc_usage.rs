//! CPU and memory of processes, read from the operating system: the node processes of
//! `hypha top` and the workers of `hypha load`.
//!
//! CPU is the CPU time a process used between two samples over the wall time between
//! them, like `htop` (100 % is one core). Memory is the resident set size.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// What one process uses.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Usage {
    /// Percent of one core over the last sample interval; `None` until two samples.
    pub cpu_percent: Option<f64>,
    /// Resident memory in bytes.
    pub rss_bytes: u64,
}

/// One reading: total CPU time used and resident bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Reading {
    pub cpu: Duration,
    pub rss_bytes: u64,
}

/// Keeps the previous reading of each PID to turn CPU time into a rate.
#[derive(Debug, Default)]
pub struct ProcSampler {
    previous: BTreeMap<i32, (Reading, Instant)>,
    usage: BTreeMap<i32, Usage>,
}

impl ProcSampler {
    /// The newest usage of `pid`, or `None` when the process could not be read.
    #[must_use]
    pub fn usage(&self, pid: i32) -> Option<Usage> {
        self.usage.get(&pid).copied()
    }

    /// Reads `pids` now. A PID that cannot be read (gone, not permitted) loses its
    /// usage rather than keeping an old one.
    pub fn sample(&mut self, pids: &[i32], now: Instant) {
        let readings = read(pids);
        self.update(&readings, now);
    }

    fn update(&mut self, readings: &BTreeMap<i32, Reading>, now: Instant) {
        let mut usage = BTreeMap::new();
        let mut previous = BTreeMap::new();
        for (&pid, &reading) in readings {
            let cpu_percent = self.previous.get(&pid).and_then(|(before, at)| {
                let wall = now.checked_duration_since(*at)?.as_secs_f64();
                let used = reading.cpu.checked_sub(before.cpu)?.as_secs_f64();
                (wall > 0.0).then(|| used / wall * 100.0)
            });
            usage.insert(pid, Usage { cpu_percent, rss_bytes: reading.rss_bytes });
            previous.insert(pid, (reading, now));
        }
        self.usage = usage;
        self.previous = previous;
    }
}

/// Linux reads `/proc`; any other system runs `ps`. Both readers are built on every
/// system, so a macOS build still type-checks the Linux one.
fn read(pids: &[i32]) -> BTreeMap<i32, Reading> {
    if cfg!(target_os = "linux") { read_proc(pids) } else { read_ps(pids) }
}

/// A PID whose CPU clock or status cannot be read (gone, not permitted) has no reading.
fn read_proc(pids: &[i32]) -> BTreeMap<i32, Reading> {
    let mut readings: BTreeMap<i32, Reading> = BTreeMap::new();
    for &pid in pids {
        let Some(cpu) = process_cpu_time(pid) else {
            continue;
        };
        let Ok(status) = std::fs::read_to_string(format!("/proc/{pid}/status")) else {
            continue;
        };
        let Some(rss_bytes) = parse_vm_rss(&status) else {
            continue;
        };
        readings.insert(pid, Reading { cpu, rss_bytes });
    }
    readings
}

/// The CPU time all threads of `pid` have used, ended threads included, read from
/// the process CPU clock in nanoseconds.
#[cfg(target_os = "linux")]
fn process_cpu_time(pid: i32) -> Option<Duration> {
    let clock = nix::time::clock_getcpuclockid(nix::unistd::Pid::from_raw(pid)).ok()?;
    let used = nix::time::clock_gettime(clock).ok()?;
    Some(Duration::from(used))
}

/// Only Linux gives the CPU clock of another process; `read` runs `ps` elsewhere.
#[cfg(not(target_os = "linux"))]
fn process_cpu_time(_pid: i32) -> Option<Duration> {
    None
}

fn read_ps(pids: &[i32]) -> BTreeMap<i32, Reading> {
    let list: Vec<String> = pids.iter().filter(|&&pid| pid > 0).map(i32::to_string).collect();
    if list.is_empty() {
        return BTreeMap::new();
    }
    // `ps` exits 1 when one of the PIDs is gone; the others are still printed.
    let output = std::process::Command::new("ps")
        .args(["-o", "pid=,cputime=,rss=", "-p", &list.join(",")])
        .stderr(std::process::Stdio::null())
        .output();
    output.map_or_default(|out| parse_ps(&String::from_utf8_lossy(&out.stdout)))
}

/// The `VmRSS` of a `/proc/<pid>/status` text, in bytes.
fn parse_vm_rss(text: &str) -> Option<u64> {
    parse_status_kib(text, "VmRSS:")
}

/// The field `key` (such as `VmRSS:`) of a `/proc/<pid>/status` text, given in KiB, in bytes.
fn parse_status_kib(text: &str, key: &str) -> Option<u64> {
    let line = text.lines().find(|line| line.starts_with(key))?;
    let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    kib.checked_mul(1024)
}

/// The memory of one process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Memory {
    /// Resident bytes, shared pages included.
    pub rss_bytes: u64,
    /// Resident shared memory (`RssShmem`, Linux only): the shared-memory pages the
    /// process has mapped and touched. `None` where the system does not separate it.
    pub shared_bytes: Option<u64>,
}

/// Where the shared part of a process's resident memory is not told: every system but
/// Linux.
pub const SHARED_MEMORY_UNAVAILABLE: &str =
    "only Linux separates the resident shared memory of a process (RssShmem)";

/// The memory of `pid` now, or `None` when it cannot be read (gone, not permitted).
/// Linux reads `/proc/<pid>/status`; any other system runs `ps`.
#[must_use]
pub fn memory(pid: i32) -> Option<Memory> {
    if cfg!(target_os = "linux") {
        let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
        parse_status_memory(&status)
    } else {
        let output = std::process::Command::new("ps")
            .args(["-o", "rss=", "-p", &pid.to_string()])
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?;
        let kib: u64 = String::from_utf8_lossy(&output.stdout).trim().parse().ok()?;
        Some(Memory { rss_bytes: kib.checked_mul(1024)?, shared_bytes: None })
    }
}

/// The memory a `/proc/<pid>/status` text names: `VmRSS` and `RssShmem`.
fn parse_status_memory(text: &str) -> Option<Memory> {
    Some(Memory {
        rss_bytes: parse_vm_rss(text)?,
        shared_bytes: Some(parse_status_kib(text, "RssShmem:")?),
    })
}

/// Lines of `ps -o pid=,cputime=,rss=`: PID, `[[dd-]hh:]mm:ss.cc` and KiB.
fn parse_ps(text: &str) -> BTreeMap<i32, Reading> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let pid = fields.next()?.parse().ok()?;
            let cpu = parse_cputime(fields.next()?)?;
            let rss_bytes = fields.next()?.parse::<u64>().ok()?.checked_mul(1024)?;
            Some((pid, Reading { cpu, rss_bytes }))
        })
        .collect()
}

/// `[[dd-]hh:]mm:ss[.ff]` as a duration.
fn parse_cputime(text: &str) -> Option<Duration> {
    let (days, clock) = match text.split_once('-') {
        Some((days, clock)) => (days.parse::<u64>().ok()?, clock),
        None => (0, text),
    };
    let mut parts: Vec<&str> = clock.split(':').collect();
    let seconds: f64 = parts.pop()?.parse().ok()?;
    let mut whole: u64 = 0;
    for part in parts {
        whole = whole.checked_mul(60)?.checked_add(part.parse().ok()?)?;
    }
    let whole_secs = days.checked_mul(86_400)?.checked_add(whole.checked_mul(60)?)?;
    let total =
        Duration::from_secs(whole_secs).checked_add(Duration::try_from_secs_f64(seconds).ok()?)?;
    Some(total)
}

/// Bytes as `12.3M` style text, binary units.
#[must_use]
pub fn format_memory(bytes: u64) -> String {
    let units = ["B", "K", "M", "G", "T"];
    // The float conversion loses precision above 2^53 bytes, far beyond any RSS.
    #[allow(clippy::cast_precision_loss)]
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < units.len().saturating_sub(1) {
        value /= 1024.0;
        unit = unit.saturating_add(1);
    }
    let suffix = units.get(unit).copied().unwrap_or("B");
    if unit == 0 { format!("{bytes}{suffix}") } else { format!("{value:.1}{suffix}") }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]
mod tests {
    use super::*;

    #[test]
    fn vm_rss_is_read_in_kib() {
        let status = "Name:\tnode\nVmPeak:\t  9000 kB\nVmRSS:\t   2048 kB\nThreads:\t3\n";
        assert_eq!(parse_vm_rss(status), Some(2048 * 1024));
        assert_eq!(parse_vm_rss("Name:\tkernel_thread\n"), None);
    }

    #[test]
    fn status_memory_reads_the_resident_and_the_shared_part() {
        let status = "VmRSS:\t   2048 kB\nRssAnon:\t 1024 kB\nRssShmem:\t  512 kB\n";
        assert_eq!(
            parse_status_memory(status),
            Some(Memory { rss_bytes: 2048 * 1024, shared_bytes: Some(512 * 1024) })
        );
        assert_eq!(parse_status_memory("VmRSS:\t 1 kB\n"), None, "a missing field is no reading");
    }

    #[test]
    fn this_process_memory_can_be_read() {
        let pid = i32::try_from(std::process::id()).unwrap();
        let memory = memory(pid).expect("own memory");
        assert!(memory.rss_bytes > 0);
        assert_eq!(memory.shared_bytes.is_some(), cfg!(target_os = "linux"));
    }

    #[test]
    fn ps_lines_and_cputime_formats() {
        let text = "    1  46:18.04  15936\n11678   0:00.01   3040\n  77 1-02:03:04.50 8\n";
        let readings = parse_ps(text);
        assert_eq!(readings[&1].cpu, Duration::from_millis(46 * 60_000 + 18_040));
        assert_eq!(readings[&11678].rss_bytes, 3040 * 1024);
        assert_eq!(readings[&77].cpu, Duration::from_millis(93_784_500));
        assert_eq!(parse_cputime("1:02:03"), Some(Duration::from_secs(3723)));
        assert!(parse_ps("ps: process id too large: 999999\n").is_empty());
    }

    #[test]
    fn cpu_is_cpu_time_over_wall_time() {
        let mut sampler = ProcSampler::default();
        let start = Instant::now();
        let reading = |ms: u64| {
            BTreeMap::from([(7, Reading { cpu: Duration::from_millis(ms), rss_bytes: 1 })])
        };
        sampler.update(&reading(1000), start);
        assert_eq!(sampler.usage(7).unwrap().cpu_percent, None, "one sample has no rate");
        sampler.update(&reading(1500), start + Duration::from_secs(1));
        let percent = sampler.usage(7).unwrap().cpu_percent.unwrap();
        assert!((percent - 50.0).abs() < 1e-9, "{percent}");
        sampler.update(&BTreeMap::new(), start + Duration::from_secs(2));
        assert_eq!(sampler.usage(7), None, "a process that cannot be read shows nothing");
    }

    #[test]
    fn this_process_can_be_read() {
        let mut sampler = ProcSampler::default();
        let pid = i32::try_from(std::process::id()).unwrap();
        sampler.sample(&[pid], Instant::now());
        let usage = sampler.usage(pid).expect("own usage");
        assert!(usage.rss_bytes > 0);
    }

    #[test]
    fn memory_is_formatted_in_binary_units() {
        assert_eq!(format_memory(512), "512B");
        assert_eq!(format_memory(3 * 1024 * 1024 / 2), "1.5M");
        assert_eq!(format_memory(2 * 1024 * 1024 * 1024), "2.0G");
    }
}
