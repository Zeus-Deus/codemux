//! Lightweight durable breadcrumbs for native startup and crash/restart attribution.
//! Only written in debug builds; paths under .codemux/ are ignored by Vite and the index watcher.

use std::collections::{HashMap, VecDeque};
use std::env;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;

const PERF_RING_CAPACITY: usize = 256;
const PERF_AGGREGATE_NAME_CAPACITY: usize = 64;
const PERF_AGGREGATE_SAMPLE_CAPACITY: usize = 512;

/// How many samples the (virtual) ring holds: the sample ring only ever
/// reported its length, so it is a counter saturating at
/// [`PERF_RING_CAPACITY`], not a buffer. `record_perf_timing` runs on every
/// daemon RPC (every keystroke), so this stays a lock-free atomic and the
/// only lock on that path is the one guarding the per-name aggregates.
static PERF_SAMPLE_COUNT: AtomicUsize = AtomicUsize::new(0);
#[derive(Default)]
struct PerfAggregate {
    count: usize,
    retained: VecDeque<f64>,
    max_ms: f64,
}

static PERF_AGGREGATES: LazyLock<Mutex<HashMap<&'static str, PerfAggregate>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static NATIVE_START: LazyLock<Instant> = LazyLock::new(Instant::now);

#[derive(Debug, Clone, Serialize)]
pub struct PerfTimingSummary {
    pub name: &'static str,
    pub count: usize,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub max_ms: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct NativePerformanceDiagnostics {
    pub version: u8,
    pub capacity: usize,
    pub sample_count: usize,
    pub timings: Vec<PerfTimingSummary>,
}

/// Add a sanitized native timing sample to a bounded in-memory ring.
///
/// Names are `&'static str` by construction: a caller cannot accidentally
/// retain a workspace id, path, session id, hostname, or command payload.
pub fn record_perf_timing(name: &'static str, elapsed: Duration) {
    let elapsed_ms = elapsed.as_secs_f64() * 1_000.0;
    // Saturating increment: a full ring drops its oldest entry for each new
    // one, so its length pins at the capacity. `Relaxed` is enough — the
    // count is a standalone statistic, ordered against nothing.
    let _ = PERF_SAMPLE_COUNT.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
        (count < PERF_RING_CAPACITY).then_some(count + 1)
    });

    let Ok(mut aggregates) = PERF_AGGREGATES.lock() else {
        return;
    };
    if !aggregates.contains_key(name) && aggregates.len() >= PERF_AGGREGATE_NAME_CAPACITY {
        return;
    }
    let aggregate = aggregates.entry(name).or_default();
    aggregate.count = aggregate.count.saturating_add(1);
    aggregate.max_ms = aggregate.max_ms.max(elapsed_ms);
    if aggregate.retained.len() == PERF_AGGREGATE_SAMPLE_CAPACITY {
        aggregate.retained.pop_front();
    }
    aggregate.retained.push_back(elapsed_ms);
}

pub fn record_startup_milestone(name: &'static str) {
    record_perf_timing(name, NATIVE_START.elapsed());
}

fn percentile(sorted: &[f64], percentile: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let rank = ((percentile / 100.0) * sorted.len() as f64).ceil() as usize;
    sorted[rank.saturating_sub(1).min(sorted.len() - 1)]
}

pub fn performance_diagnostics_snapshot() -> NativePerformanceDiagnostics {
    let sample_count = PERF_SAMPLE_COUNT.load(Ordering::Relaxed);
    let mut timings = PERF_AGGREGATES
        .lock()
        .map(|aggregates| {
            aggregates
                .iter()
                .map(|(name, aggregate)| {
                    let mut values: Vec<_> = aggregate.retained.iter().copied().collect();
            values.sort_by(f64::total_cmp);
            PerfTimingSummary {
                        name: *name,
                        count: aggregate.count,
                p50_ms: percentile(&values, 50.0),
                p95_ms: percentile(&values, 95.0),
                p99_ms: percentile(&values, 99.0),
                        max_ms: aggregate.max_ms,
            }
        })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    timings.sort_by_key(|summary| summary.name);
    NativePerformanceDiagnostics {
        version: 2,
        capacity: PERF_RING_CAPACITY,
        sample_count,
        timings,
    }
}

#[tauri::command]
pub fn get_performance_diagnostics() -> NativePerformanceDiagnostics {
    performance_diagnostics_snapshot()
}

#[cfg(debug_assertions)]
fn native_startup_log_path() -> Option<PathBuf> {
    let cwd = env::current_dir().ok()?;
    let dot = cwd.join(".codemux");
    if dot.exists() || cwd.join("package.json").exists() {
        return Some(dot.join("native-startup.log"));
    }
    let dot_alt = cwd.join("..").join(".codemux");
    if dot_alt.exists() {
        return Some(dot_alt.join("native-startup.log"));
    }
    Some(dot.join("native-startup.log"))
}

#[cfg(debug_assertions)]
fn native_global_log_path() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        let log_dir = dirs::data_local_dir()?.join("Codemux").join("logs");
        return Some(log_dir.join("codemux-native-launches.log"));
    }
    #[cfg(not(windows))]
    {
        let runtime_dir = env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .or_else(|| Some(std::env::temp_dir()))?;
        Some(runtime_dir.join("codemux-native-launches.log"))
    }
}

#[cfg(debug_assertions)]
fn append_debug_log(path: &PathBuf, line: &str) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(f, "{}", line);
        let _ = f.flush();
    }
}

/// Writes a durable native startup breadcrumb (debug builds only).
///
/// This is intended to correlate multi-process launch attempts and the
/// control-socket lifecycle. It writes to both:
/// - `.codemux/native-startup.log` (project-local)
/// - `$XDG_RUNTIME_DIR/codemux-native-launches.log` (global per-user)
pub fn native_startup_breadcrumb(line: &str) {
    #[cfg(debug_assertions)]
    {
        if let Some(path) = native_startup_log_path() {
            append_debug_log(&path, line);
        }
        if let Some(path) = native_global_log_path() {
            append_debug_log(&path, line);
        }
    }
}

/// Writes a line to stderr without panicking if the stream is closed.
pub fn stderr_line(line: &str) {
    let mut stderr = std::io::stderr().lock();
    let _ = writeln!(stderr, "{}", line);
    let _ = stderr.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentile_uses_nearest_rank() {
        let values = vec![1.0, 2.0, 3.0, 4.0, 100.0];
        assert_eq!(percentile(&values, 50.0), 3.0);
        assert_eq!(percentile(&values, 95.0), 100.0);
    }

    #[test]
    fn diagnostic_names_are_static_and_samples_are_bounded() {
        PERF_SAMPLE_COUNT.store(0, Ordering::Relaxed);
        PERF_AGGREGATES.lock().unwrap().clear();
        for i in 0..(PERF_RING_CAPACITY + 20) {
            record_perf_timing("test.static-operation", Duration::from_millis(i as u64));
        }
        let snapshot = performance_diagnostics_snapshot();
        assert_eq!(snapshot.sample_count, PERF_RING_CAPACITY);
        assert_eq!(snapshot.timings.len(), 1);
        assert_eq!(snapshot.timings[0].name, "test.static-operation");
        assert_eq!(snapshot.timings[0].count, PERF_RING_CAPACITY + 20);
        assert_eq!(snapshot.timings[0].p99_ms, 273.0);
        assert_eq!(snapshot.timings[0].max_ms, (PERF_RING_CAPACITY + 19) as f64);
    }
}
