use crate::csv_parser::TraceEvent;
use anyhow::Result;
use std::{
    sync::mpsc::Sender,
    time::{Duration, Instant},
};

/// Run the event-driven scheduler.
///
/// Sleeps until the timestamp of each event, then sends the target bandwidth
/// to the Netlink worker.  Aborts with an error if the scheduler wakes up more
/// than `max_drift_ms` milliseconds late (indicating the system is overloaded
/// or the CSV resolution is too fine).
pub fn run(trace: Vec<TraceEvent>, tx: Sender<u64>, max_drift_ms: u64) -> Result<()> {
    let max_drift = Duration::from_millis(max_drift_ms);
    let start = Instant::now();

    for event in &trace {
        let target_offset = Duration::from_secs_f64(event.time_sec);

        // How long until this event fires?
        let elapsed = start.elapsed();
        if target_offset > elapsed {
            let sleep_duration = target_offset - elapsed;
            std::thread::sleep(sleep_duration);
        }

        // Drift check: how far past the target did we wake up?
        let actual_offset = start.elapsed();
        let drift = if actual_offset > target_offset {
            actual_offset - target_offset
        } else {
            Duration::ZERO
        };

        if drift >= max_drift {
            anyhow::bail!(
                "Scheduler drift exceeded limit: target={:.3}s, actual={:.3}s, \
                 drift={:.0}ms > max {}ms. \
                 The CSV timestamp resolution may be too fine for this system.",
                target_offset.as_secs_f64(),
                actual_offset.as_secs_f64(),
                drift.as_millis(),
                max_drift_ms,
            );
        }

        println!(
            "[scheduler] t={:.3}s  bw={} kbps  (drift={:.1}ms)",
            actual_offset.as_secs_f64(),
            event.bandwidth_kbps,
            drift.as_secs_f64() * 1000.0,
        );

        // Non-blocking send – if the worker is busy the value queues up
        if tx.send(event.bandwidth_kbps).is_err() {
            // Worker thread has exited
            break;
        }
    }

    println!("[scheduler] All {} events dispatched.", trace.len());
    Ok(())
}
