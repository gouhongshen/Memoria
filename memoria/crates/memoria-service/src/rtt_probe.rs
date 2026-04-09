//! Periodic database RTT probe.
//! Executes `SELECT 1` every 30s on the global user pool and records latency.

use sqlx::MySqlPool;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

const ORD: Ordering = Ordering::Relaxed;
const PROBE_INTERVAL_SECS: u64 = 30;

/// Histogram for DB RTT (seconds).
static RTT_BUCKETS: &[f64] = &[
    0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0,
];

struct RttHistogram {
    buckets: Vec<AtomicU64>,
    sum: AtomicU64,
    count: AtomicU64,
}

impl RttHistogram {
    fn new() -> Self {
        Self {
            buckets: (0..RTT_BUCKETS.len() + 1).map(|_| AtomicU64::new(0)).collect(),
            sum: AtomicU64::new(0),
            count: AtomicU64::new(0),
        }
    }

    fn observe(&self, secs: f64) {
        for (i, &bound) in RTT_BUCKETS.iter().enumerate() {
            if secs <= bound {
                self.buckets[i].fetch_add(1, ORD);
            }
        }
        self.buckets[RTT_BUCKETS.len()].fetch_add(1, ORD);
        self.count.fetch_add(1, ORD);
        let mut old = self.sum.load(ORD);
        loop {
            let new_val = f64::from_bits(old) + secs;
            match self.sum.compare_exchange_weak(old, new_val.to_bits(), ORD, ORD) {
                Ok(_) => break,
                Err(actual) => old = actual,
            }
        }
    }
}

static INSTANCE: OnceLock<RttHistogram> = OnceLock::new();

fn global() -> &'static RttHistogram {
    INSTANCE.get_or_init(RttHistogram::new)
}

/// Spawn the background RTT probe. Call once at startup.
pub fn spawn_rtt_probe(pool: MySqlPool) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(PROBE_INTERVAL_SECS));
        interval.tick().await; // skip immediate
        loop {
            interval.tick().await;
            if pool.is_closed() { break; }
            let t = std::time::Instant::now();
            match sqlx::query("SELECT 1").execute(&pool).await {
                Ok(_) => global().observe(t.elapsed().as_secs_f64()),
                Err(_) => {} // connection errors tracked elsewhere
            }
        }
    });
}

/// Render Prometheus histogram for /metrics.
pub fn render(out: &mut String) {
    let h = global();
    out.push_str("# HELP memoria_db_rtt_seconds Database round-trip time (SELECT 1 probe).\n");
    out.push_str("# TYPE memoria_db_rtt_seconds histogram\n");
    for (i, &bound) in RTT_BUCKETS.iter().enumerate() {
        out.push_str(&format!(
            "memoria_db_rtt_seconds_bucket{{le=\"{bound}\"}} {}\n",
            h.buckets[i].load(ORD)
        ));
    }
    out.push_str(&format!(
        "memoria_db_rtt_seconds_bucket{{le=\"+Inf\"}} {}\n",
        h.buckets[RTT_BUCKETS.len()].load(ORD)
    ));
    out.push_str(&format!(
        "memoria_db_rtt_seconds_sum {}\n",
        f64::from_bits(h.sum.load(ORD))
    ));
    out.push_str(&format!(
        "memoria_db_rtt_seconds_count {}\n",
        h.count.load(ORD)
    ));
}
