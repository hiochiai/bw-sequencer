//! Linux Traffic Control setup/teardown via `tc` (iproute2).
//!
//! We build a simple HTB hierarchy:
//!
//!   root qdisc  (HTB, handle 1:)
//!   └── class 1:1   (HTB, rate=ceil, ceil=ceil)    ← parent class
//!       └── class 1:10  (HTB, rate=ceil, ceil=ceil) ← leaf class (throttled)
//!           └── qdisc fq_codel (handle 10:)         ← AQM; never touched on rate change
//!
//! Setup and teardown are one-shot operations performed via the `tc` command.
//! Dynamic rate updates on the hot path are handled by `netlink_htb::HtbUpdater`,
//! which talks directly to the kernel via a persistent Netlink socket.

use anyhow::{Context, Result};
use std::process::Command;

// ── Handle / class-id constants ───────────────────────────────────────────────
/// Root qdisc handle: 1:
const HANDLE_ROOT: &str = "1:";
/// Root HTB class: 1:1
const CLASS_ROOT: &str = "1:1";
/// Leaf HTB class whose rate we vary: 1:10
const CLASS_LEAF: &str = "1:10";
/// Leaf fq_codel qdisc handle: 10:
const HANDLE_LEAF: &str = "10:";

// ──────────────────────────────────────────────────────────────────────────────

fn run_tc(args: &[&str]) -> Result<()> {
    let status = Command::new("tc")
        .args(args)
        .status()
        .context("Failed to execute 'tc' – is iproute2 installed?")?;
    if !status.success() {
        anyhow::bail!("tc {:?} exited with status {}", args, status);
    }
    Ok(())
}

fn del_root_qdisc(iface: &str) {
    let _ = Command::new("tc")
        .args(["qdisc", "del", "dev", iface, "root"])
        .output();
}

/// Build the HTB hierarchy on `iface`.
///
/// Uses `tc` so that `buffer`, `cbuffer`, and `quantum` are computed
/// correctly by iproute2.  After this call, `netlink_htb::HtbUpdater::new`
/// reads those values back via `RTM_GETTCLASS` and caches them for fast
/// in-place updates.
pub fn setup(iface: &str, ceil_kbps: u64) -> Result<()> {
    del_root_qdisc(iface);

    let ceil = format!("{ceil_kbps}kbit");

    run_tc(&[
        "qdisc",
        "add",
        "dev",
        iface,
        "root",
        "handle",
        HANDLE_ROOT,
        "htb",
        "default",
        "10",
    ])
    .context("Add root HTB qdisc")?;

    run_tc(&[
        "class",
        "add",
        "dev",
        iface,
        "parent",
        HANDLE_ROOT,
        "classid",
        CLASS_ROOT,
        "htb",
        "rate",
        &ceil,
        "ceil",
        &ceil,
    ])
    .context("Add root HTB class 1:1")?;

    run_tc(&[
        "class", "add", "dev", iface, "parent", CLASS_ROOT, "classid", CLASS_LEAF, "htb", "rate",
        &ceil, "ceil", &ceil,
    ])
    .context("Add leaf HTB class 1:10")?;

    // fq_codel is preferred: it provides AQM (CoDel algorithm) and per-flow
    // fair queuing, both important for realistic mobile-network emulation.
    // Fall back to sfq on kernels that lack CONFIG_NET_SCH_FQ_CODEL (e.g.
    // the linuxkit kernel used by Docker Desktop).  sfq still provides
    // per-flow fairness; only the CoDel-based latency management is absent.
    if run_tc(&[
        "qdisc",
        "add",
        "dev",
        iface,
        "parent",
        CLASS_LEAF,
        "handle",
        HANDLE_LEAF,
        "fq_codel",
    ])
    .is_err()
    {
        eprintln!(
            "[tc] Warning: fq_codel unavailable on this kernel – falling back to sfq. \
             CoDel AQM will not be active."
        );
        run_tc(&[
            "qdisc",
            "add",
            "dev",
            iface,
            "parent",
            CLASS_LEAF,
            "handle",
            HANDLE_LEAF,
            "sfq",
        ])
        .context("Add sfq leaf qdisc (fq_codel unavailable on this kernel)")?;
    }

    Ok(())
}

/// Remove the root qdisc and restore the interface to its default state.
pub fn teardown(iface: &str) -> Result<()> {
    run_tc(&["qdisc", "del", "dev", iface, "root"]).context("Remove root qdisc")?;
    println!("[tc] Qdisc removed from '{iface}' – interface restored.");
    Ok(())
}
