mod csv_parser;
mod netlink_htb;
mod netlink_worker;
mod scheduler;
mod tc;

use anyhow::{Context, Result};
use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "bw-sequencer",
    version,
    about = "A bandwidth sequencer to emulate dynamic network conditions for mobile IoT and vehicle-mounted devices",
    long_about = "Applies time-series bandwidth profiles to a network interface via Linux TC/HTB.\n\
                  Reads a CSV file with (time_sec, bandwidth_kbps) rows and applies each\n\
                  bandwidth change at the specified timestamp with nanosecond-precision sleep."
)]
struct Cli {
    /// Network interface to throttle (e.g. eth0, veth0)
    #[arg(short, long)]
    interface: String,

    /// CSV trace file: columns time_sec,bandwidth_kbps
    #[arg(short, long)]
    csv: String,

    /// Abort if the scheduler wakes up this many milliseconds late
    #[arg(long, default_value_t = 1000)]
    max_drift_ms: u64,

    /// HTB ceiling bandwidth in kbps – sets the maximum possible rate
    #[arg(long, default_value_t = 1_000_000)]
    ceil_kbps: u64,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // ── 1. Load CSV trace into memory (no disk I/O during playback) ───────────
    let trace = csv_parser::load(&cli.csv)
        .with_context(|| format!("Failed to load CSV trace from '{}'", cli.csv))?;

    if trace.is_empty() {
        anyhow::bail!("CSV trace is empty – nothing to do.");
    }

    println!(
        "[bw-sequencer] Loaded {} events from '{}' for interface '{}'",
        trace.len(),
        cli.csv,
        cli.interface
    );

    // ── 2. Build HTB hierarchy via tc (one-shot; computes buffer/quantum) ─────
    tc::setup(&cli.interface, cli.ceil_kbps)
        .with_context(|| format!("TC setup failed on interface '{}'", cli.interface))?;

    println!(
        "[bw-sequencer] TC/HTB initialised on '{}' (ceil {} kbps)",
        cli.interface, cli.ceil_kbps
    );

    // ── 3. Signal handler: tear down TC and exit cleanly on Ctrl-C / SIGTERM ──
    let iface_sig = cli.interface.clone();
    ctrlc::set_handler(move || {
        println!("\n[bw-sequencer] Signal received – cleaning up...");
        if let Err(e) = tc::teardown(&iface_sig) {
            eprintln!("[bw-sequencer] Cleanup error: {e}");
        }
        std::process::exit(0);
    })
    .context("Failed to install signal handler")?;

    // ── 4. Spawn Netlink worker thread ────────────────────────────────────────
    let (tx, rx) = std::sync::mpsc::channel::<u64>();
    let iface_worker = cli.interface.clone();
    let worker = std::thread::spawn(move || netlink_worker::run(&iface_worker, rx));

    // ── 5. Run scheduler (blocks until trace completes or error) ─────────────
    let scheduler_result = scheduler::run(trace, tx, cli.max_drift_ms);

    // ── 6. Wait for worker and surface any initialisation error ───────────────
    // Dropping `tx` (moved into scheduler::run) signals the worker to stop.
    let worker_result = worker
        .join()
        .unwrap_or_else(|_| Err(anyhow::anyhow!("Netlink worker thread panicked")));

    // ── 7. Restore interface (signal handler already covers the SIGINT path) ──
    if let Err(e) = tc::teardown(&cli.interface) {
        eprintln!("[bw-sequencer] Teardown error: {e}");
    }

    // Report the first error encountered (scheduler takes priority)
    scheduler_result.and(worker_result)
}
