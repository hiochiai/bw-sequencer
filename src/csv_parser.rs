use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Clone)]
pub struct TraceEvent {
    /// Seconds from the start of the trace (may have sub-second precision)
    pub time_sec: f64,
    /// Target bandwidth in kbps
    pub bandwidth_kbps: u64,
}

#[derive(Debug, Deserialize)]
struct CsvRow {
    time_sec: f64,
    bandwidth_kbps: u64,
}

/// Load and validate a CSV trace file.
///
/// The CSV must have a header row with at least `time_sec` and `bandwidth_kbps`
/// columns (additional columns are ignored).  Rows are sorted by `time_sec`
/// before being returned so the caller need not worry about ordering.
pub fn load(path: &str) -> Result<Vec<TraceEvent>> {
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(true)
        .trim(csv::Trim::All)
        .comment(Some(b'#'))
        .from_path(path)
        .with_context(|| format!("Cannot open '{path}'"))?;

    let mut events: Vec<TraceEvent> = Vec::new();

    for (line, result) in reader.deserialize::<CsvRow>().enumerate() {
        let row = result.with_context(|| format!("Parse error at data row {}", line + 1))?;

        if row.time_sec < 0.0 {
            anyhow::bail!(
                "Row {}: time_sec must be >= 0, got {}",
                line + 1,
                row.time_sec
            );
        }
        if row.bandwidth_kbps == 0 {
            anyhow::bail!(
                "Row {}: bandwidth_kbps must be > 0 (use a small value like 8 for near-zero)",
                line + 1
            );
        }

        events.push(TraceEvent {
            time_sec: row.time_sec,
            bandwidth_kbps: row.bandwidth_kbps,
        });
    }

    // Stable sort so equal timestamps preserve CSV order
    events.sort_by(|a, b| a.time_sec.partial_cmp(&b.time_sec).unwrap());

    Ok(events)
}
