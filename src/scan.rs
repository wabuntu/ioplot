use std::collections::HashMap;
use std::fs;
use std::time::Instant;

#[derive(Debug, Clone, Copy, Default)]
pub struct ProcIo {
    pub rchar: u64,
    pub wchar: u64,
    pub read_bytes: u64,
    pub write_bytes: u64,
}

/// One process's I/O rate since the previous sample, in bytes/sec.
#[derive(Debug, Clone)]
pub struct ProcRate {
    pub pid: u32,
    pub name: String,
    pub read_bps: f64,
    pub write_bps: f64,
    /// Cumulative totals as of this sample, for the detail popup.
    pub read_bytes_total: u64,
    pub write_bytes_total: u64,
}

/// A full system sample: aggregate read/write rate plus every process's
/// individual rate, already sorted (see `Sort`).
pub struct SystemSample {
    pub total_read_bps: f64,
    pub total_write_bps: f64,
    pub processes: Vec<ProcRate>,
    /// True if any process's `/proc/[pid]/io` was unreadable (needs root to
    /// see other users' processes) — the totals above only reflect what was
    /// actually readable, so they may undercount.
    pub restricted: bool,
}

fn read_proc_name(pid: u32) -> String {
    fs::read_to_string(format!("/proc/{pid}/comm"))
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| format!("[{pid}]"))
}

fn read_proc_io(pid: u32) -> Option<ProcIo> {
    let text = fs::read_to_string(format!("/proc/{pid}/io")).ok()?;
    let mut io = ProcIo::default();
    for line in text.lines() {
        let (key, value) = line.split_once(':')?;
        let value: u64 = value.trim().parse().ok()?;
        match key {
            "rchar" => io.rchar = value,
            "wchar" => io.wchar = value,
            "read_bytes" => io.read_bytes = value,
            "write_bytes" => io.write_bytes = value,
            _ => {}
        }
    }
    Some(io)
}

fn list_pids() -> Vec<u32> {
    let Ok(entries) = fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().to_str()?.parse::<u32>().ok())
        .collect()
}

/// Polls `/proc/[pid]/io` for every process and turns the delta since the
/// previous poll into a rate. Keeps its own clock, so callers just need to
/// call `sample()` on whatever cadence they want.
pub struct Sampler {
    previous: HashMap<u32, ProcIo>,
    last_sample: Instant,
}

impl Sampler {
    pub fn new() -> Sampler {
        Sampler {
            previous: HashMap::new(),
            last_sample: Instant::now(),
        }
    }

    pub fn sample(&mut self) -> SystemSample {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_sample).as_secs_f64().max(1e-6);
        self.last_sample = now;

        let mut current: HashMap<u32, ProcIo> = HashMap::new();
        let mut processes = Vec::new();
        let mut restricted = false;
        let mut total_read_bps = 0.0;
        let mut total_write_bps = 0.0;

        for pid in list_pids() {
            let Some(io) = read_proc_io(pid) else {
                restricted = true;
                continue;
            };
            let (read_bps, write_bps) = match self.previous.get(&pid) {
                Some(prev) => (
                    (io.read_bytes.saturating_sub(prev.read_bytes)) as f64 / elapsed,
                    (io.write_bytes.saturating_sub(prev.write_bytes)) as f64 / elapsed,
                ),
                None => (0.0, 0.0), // first time we've seen this pid: no delta yet
            };
            total_read_bps += read_bps;
            total_write_bps += write_bps;

            if read_bps > 0.0 || write_bps > 0.0 {
                processes.push(ProcRate {
                    pid,
                    name: read_proc_name(pid),
                    read_bps,
                    write_bps,
                    read_bytes_total: io.read_bytes,
                    write_bytes_total: io.write_bytes,
                });
            }
            current.insert(pid, io);
        }

        self.previous = current;
        processes.sort_by(|a, b| {
            (b.read_bps + b.write_bps)
                .partial_cmp(&(a.read_bps + a.write_bps))
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        SystemSample {
            total_read_bps,
            total_write_bps,
            processes,
            restricted,
        }
    }
}

/// "1.2 MB/s"-style formatting, base-1000.
pub fn human_rate(bytes_per_sec: f64) -> String {
    const UNITS: [&str; 5] = ["B/s", "KB/s", "MB/s", "GB/s", "TB/s"];
    let mut v = bytes_per_sec;
    let mut unit = 0;
    while v >= 1000.0 && unit < UNITS.len() - 1 {
        v /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{:.0} {}", v, UNITS[unit])
    } else {
        format!("{:.1} {}", v, UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_rate_formats_common_ranges() {
        assert_eq!(human_rate(0.0), "0 B/s");
        assert_eq!(human_rate(999.0), "999 B/s");
        assert_eq!(human_rate(1_500.0), "1.5 KB/s");
        assert_eq!(human_rate(2_000_000.0), "2.0 MB/s");
        assert_eq!(human_rate(3_500_000_000.0), "3.5 GB/s");
    }

    #[test]
    fn sampler_reports_zero_rate_on_first_sample() {
        let mut sampler = Sampler::new();
        let sample = sampler.sample();
        // First sample has no prior baseline, so every rate must be 0 and
        // no process should show up in the (rate > 0) list.
        assert!(
            sample
                .processes
                .iter()
                .all(|p| p.read_bps == 0.0 && p.write_bps == 0.0)
        );
    }

    #[test]
    fn read_proc_io_parses_self() {
        let io = read_proc_io(std::process::id()).expect("own /proc/self/io must be readable");
        // Just confirm parsing succeeded with plausible (non-negative,
        // already-guaranteed by u64) fields; rchar is essentially always
        // nonzero since reading this very file counts as a read.
        assert!(io.rchar > 0);
    }
}
