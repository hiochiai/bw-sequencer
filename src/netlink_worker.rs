//! Netlink worker thread.
//!
//! Owns an `HtbUpdater` and drains the mpsc channel sent by the scheduler,
//! applying each rate change directly via a persistent Netlink socket.
//! Running in a separate thread ensures that the microsecond-level Netlink
//! round-trip never delays the scheduler's sleep/wake cycle.

use crate::netlink_htb::HtbUpdater;
use anyhow::{Context as _, Result};
use std::sync::mpsc::Receiver;

/// Receive rate values from the scheduler and apply them to the HTB class.
///
/// Returns `Err` if the Netlink socket cannot be initialised.  Rate-change
/// errors are logged but treated as non-fatal (best-effort delivery); the
/// scheduler's drift check will catch any resulting overload condition.
pub fn run(iface: &str, rx: Receiver<u64>) -> Result<()> {
    let mut updater = HtbUpdater::new(iface).context("Failed to initialise Netlink HTB updater")?;

    while let Ok(bandwidth_kbps) = rx.recv() {
        if let Err(e) = updater.set_rate(bandwidth_kbps) {
            eprintln!("[worker] set_rate({bandwidth_kbps} kbps) failed: {e}");
        }
    }
    Ok(())
}
