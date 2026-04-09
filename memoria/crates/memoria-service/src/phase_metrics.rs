//! Lightweight per-tool per-phase timing metrics.
//! Recorded in memoria-service, scraped by memoria-api for /metrics.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{OnceLock, RwLock};

const ORD: Ordering = Ordering::Relaxed;

/// Histogram bucket boundaries (seconds).
pub const BOUNDS: &[f64] = &[
    0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0,
];

struct Histogram {
    buckets: Vec<AtomicU64>,
    sum: AtomicU64,
    count: AtomicU64,
}

impl Histogram {
    fn new() -> Self {
        Self {
            buckets: (0..BOUNDS.len() + 1).map(|_| AtomicU64::new(0)).collect(),
            sum: AtomicU64::new(0),
            count: AtomicU64::new(0),
        }
    }

    fn observe(&self, value: f64) {
        for (i, &bound) in BOUNDS.iter().enumerate() {
            if value <= bound {
                self.buckets[i].fetch_add(1, ORD);
            }
        }
        self.buckets[BOUNDS.len()].fetch_add(1, ORD); // +Inf
        self.count.fetch_add(1, ORD);
        // CAS loop for float sum
        let mut old = self.sum.load(ORD);
        loop {
            let new_val = f64::from_bits(old) + value;
            match self.sum.compare_exchange_weak(old, new_val.to_bits(), ORD, ORD) {
                Ok(_) => break,
                Err(actual) => old = actual,
            }
        }
    }

    fn snapshot(&self) -> HistogramSnapshot {
        HistogramSnapshot {
            buckets: self.buckets.iter().map(|b| b.load(ORD)).collect(),
            sum: f64::from_bits(self.sum.load(ORD)),
            count: self.count.load(ORD),
        }
    }
}

pub struct HistogramSnapshot {
    pub buckets: Vec<u64>,
    pub sum: f64,
    pub count: u64,
}

struct PhaseMetricsInner {
    histograms: RwLock<HashMap<String, Histogram>>,
}

impl PhaseMetricsInner {
    fn new() -> Self {
        Self {
            histograms: RwLock::new(HashMap::new()),
        }
    }

    fn observe(&self, key: &str, value: f64) {
        // Fast path: read lock
        if let Ok(map) = self.histograms.read() {
            if let Some(h) = map.get(key) {
                h.observe(value);
                return;
            }
        }
        // Slow path: write lock to insert
        if let Ok(mut map) = self.histograms.write() {
            let h = map.entry(key.to_string()).or_insert_with(Histogram::new);
            h.observe(value);
        }
    }

    fn snapshot(&self) -> Vec<(String, HistogramSnapshot)> {
        let map = self.histograms.read().unwrap();
        map.iter().map(|(k, h)| (k.clone(), h.snapshot())).collect()
    }
}

static INSTANCE: OnceLock<PhaseMetricsInner> = OnceLock::new();

fn global() -> &'static PhaseMetricsInner {
    INSTANCE.get_or_init(PhaseMetricsInner::new)
}

/// Record a phase duration. Called from service/tool code.
pub fn record(tool: &str, phase: &str, secs: f64) {
    global().observe(&format!("{tool}|{phase}"), secs);
}

/// Get all recorded histograms. Called from API metrics renderer.
pub fn snapshot() -> Vec<(String, HistogramSnapshot)> {
    global().snapshot()
}
