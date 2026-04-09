//! Per-tool per-phase duration metrics for MCP tool calls.

use crate::metrics::types::HistogramVec;

/// Histogram buckets covering 1ms to 30s.
pub(crate) const DURATION_BOUNDS: &[f64] = &[
    0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0,
];

pub struct ToolMetrics {
    /// Key: "tool|phase" e.g. "memory_search|embed", "memory_search|total"
    pub phase_duration: HistogramVec,
}

impl ToolMetrics {
    pub(crate) fn new() -> Self {
        Self {
            phase_duration: HistogramVec::new(DURATION_BOUNDS),
        }
    }

    /// Record a phase duration. `tool` = "memory_search", `phase` = "embed".
    #[inline]
    pub fn record(&self, tool: &str, phase: &str, duration_secs: f64) {
        let key = format!("{tool}|{phase}");
        self.phase_duration.observe(&key, duration_secs);
    }
}
