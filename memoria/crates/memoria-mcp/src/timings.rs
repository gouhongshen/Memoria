//! Per-tool phase timing data returned alongside tool results.

/// Phase timings for a tool call. All durations in seconds.
/// Phases are mutually exclusive and sum to total.
#[derive(Default, Debug)]
pub struct ToolTimings {
    pub tool: String,
    pub phases: Vec<(&'static str, f64)>,
    pub total_secs: f64,
}

impl ToolTimings {
    pub fn new(tool: &str) -> Self {
        Self {
            tool: tool.to_string(),
            ..Default::default()
        }
    }

    pub fn phase(&mut self, name: &'static str, secs: f64) {
        self.phases.push((name, secs));
    }

    pub fn finish(&mut self, start: std::time::Instant) {
        self.total_secs = start.elapsed().as_secs_f64();
    }
}
