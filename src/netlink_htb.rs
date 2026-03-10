//! Direct Netlink/RTnetlink implementation for HTB class rate updates.
//!
//! # Why not fork `tc`?
//! Forking a subprocess for every bandwidth event adds milliseconds of overhead
//! and risks flushing the in-flight packet queue inside the kernel TC subsystem.
//! Instead we keep a persistent `NETLINK_ROUTE` socket open and issue a single
//! `RTM_NEWTCLASS | NLM_F_REPLACE` message per rate change.
//!
//! # Strategy (Option A: RTM_GETTCLASS cache)
//! 1. After `tc` initialises the HTB hierarchy, call `RTM_GETTCLASS` to read
//!    back the exact `tc_htb_opt` bytes that `tc` wrote (buffer / cbuffer /
//!    quantum are complex to compute independently; reusing what `tc` wrote is
//!    both correct and keeps this code simple).
//! 2. Cache that struct.  For every rate change, copy the cached struct, patch
//!    only the `rate` / `ceil` 32-bit fields and send
//!    `RTM_NEWTCLASS | NLM_F_REPLACE` together with `TCA_HTB_RATE64` /
//!    `TCA_HTB_CEIL64` for full 64-bit precision.
//!
//! The leaf `fq_codel` qdisc is never replaced, so no packets are dropped.

use anyhow::{Context, Result};
use netlink_sys::{protocols::NETLINK_ROUTE, Socket, SocketAddr};
use std::mem;

// ── Linux kernel Netlink constants ────────────────────────────────────────────

const RTM_NEWTCLASS: u16 = 40;
const RTM_GETTCLASS: u16 = 42;

const NLM_F_REQUEST: u16 = 0x0001;
const NLM_F_ACK: u16 = 0x0004;
const NLM_F_REPLACE: u16 = 0x0100;
/// NLM_F_ROOT | NLM_F_MATCH — requests a full dump of all TC classes.
const NLM_F_DUMP: u16 = 0x0300;

const NLMSG_ERROR: u16 = 2;
const NLMSG_DONE: u16 = 3;

// TCA top-level attribute types
const TCA_KIND: u16 = 1;
const TCA_OPTIONS: u16 = 2;

// HTB-specific attributes (nested inside TCA_OPTIONS)
const TCA_HTB_PARMS: u16 = 1;
const TCA_HTB_RATE64: u16 = 7;
const TCA_HTB_CEIL64: u16 = 8;

const NLA_F_NESTED: u16 = 0x8000;

// Handle encoding: major:minor → (major << 16) | minor
const CLASS_LEAF_HANDLE: u32 = (1u32 << 16) | 0x10; // 1:10  (tc uses hex; 0x10 = 16 decimal)
const CLASS_LEAF_PARENT: u32 = (1u32 << 16) | 1; // 1:1

/// Maximum number of `recv` calls when collecting a dump response.
/// Prevents an infinite loop if the kernel never sends `NLMSG_DONE`.
const MAX_RECV_ITERS: usize = 32;

// ── Wire-format structs (must match Linux kernel ABI exactly) ─────────────────

/// `struct nlmsghdr` – 16 bytes
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct NlMsgHdr {
    len: u32,
    type_: u16,
    flags: u16,
    seq: u32,
    pid: u32,
}

/// `struct tcmsg` – 20 bytes
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct TcMsg {
    family: u8,
    _pad1: u8,
    _pad2: u16,
    ifindex: i32,
    handle: u32,
    parent: u32,
    info: u32,
}

/// `struct tc_ratespec` – 12 bytes (private: only used inside this module)
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct TcRateSpec {
    cell_log: u8,
    linklayer: u8,
    overhead: u16,
    cell_align: i16,
    mpu: u16,
    /// Rate in bytes/sec, capped at `u32::MAX` (~34 Gbps).
    /// Use `TCA_HTB_RATE64` for the actual 64-bit value.
    rate: u32,
}

/// `struct tc_htb_opt` – 44 bytes (private: kernel ABI detail)
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct TcHtbOpt {
    rate: TcRateSpec, // 12 bytes
    ceil: TcRateSpec, // 12 bytes
    buffer: u32,      // burst in psched ticks  (preserved from tc init)
    cbuffer: u32,     // ceil burst in psched ticks (preserved from tc init)
    quantum: u32,     // bytes dequeued per scheduling opportunity
    level: u32,       // hierarchy level (written by kernel on read)
    prio: u32,        // class priority
}

const _: () = assert!(mem::size_of::<TcHtbOpt>() == 44);
const _: () = assert!(mem::size_of::<TcMsg>() == 20);
const _: () = assert!(mem::size_of::<NlMsgHdr>() == 16);

// ── Byte helpers ──────────────────────────────────────────────────────────────

fn as_bytes<T: Copy>(val: &T) -> &[u8] {
    unsafe { std::slice::from_raw_parts(val as *const T as *const u8, mem::size_of::<T>()) }
}

fn push_attr(buf: &mut Vec<u8>, type_: u16, data: &[u8]) {
    let total = 4 + data.len();
    buf.extend_from_slice(&(total as u16).to_ne_bytes());
    buf.extend_from_slice(&type_.to_ne_bytes());
    buf.extend_from_slice(data);
    // Pad to 4-byte alignment
    let padded = (total + 3) & !3;
    buf.extend(std::iter::repeat_n(0u8, padded - total));
}

fn push_nested_attr(buf: &mut Vec<u8>, type_: u16, inner: &[u8]) {
    push_attr(buf, type_ | NLA_F_NESTED, inner);
}

// ── Message builders ──────────────────────────────────────────────────────────

fn build_gettclass_msg(seq: u32, ifindex: i32) -> Vec<u8> {
    let tc = TcMsg {
        ifindex,
        handle: 0,
        parent: 0,
        ..Default::default()
    };
    let body_len = mem::size_of::<NlMsgHdr>() + mem::size_of::<TcMsg>();
    let hdr = NlMsgHdr {
        len: body_len as u32,
        type_: RTM_GETTCLASS,
        flags: NLM_F_REQUEST | NLM_F_DUMP,
        seq,
        pid: 0,
    };
    let mut buf = Vec::with_capacity(body_len);
    buf.extend_from_slice(as_bytes(&hdr));
    buf.extend_from_slice(as_bytes(&tc));
    buf
}

fn build_newtclass_msg(seq: u32, ifindex: i32, opt: TcHtbOpt, rate_bytes_per_sec: u64) -> Vec<u8> {
    let mut options = Vec::new();
    push_attr(&mut options, TCA_HTB_PARMS, as_bytes(&opt));
    push_attr(
        &mut options,
        TCA_HTB_RATE64,
        &rate_bytes_per_sec.to_ne_bytes(),
    );
    push_attr(
        &mut options,
        TCA_HTB_CEIL64,
        &rate_bytes_per_sec.to_ne_bytes(),
    ); // ceil = rate → hard limit

    let mut attrs = Vec::new();
    push_attr(&mut attrs, TCA_KIND, b"htb\0");
    push_nested_attr(&mut attrs, TCA_OPTIONS, &options);

    let tc = TcMsg {
        ifindex,
        handle: CLASS_LEAF_HANDLE,
        parent: CLASS_LEAF_PARENT,
        ..Default::default()
    };
    let total_len = mem::size_of::<NlMsgHdr>() + mem::size_of::<TcMsg>() + attrs.len();
    let hdr = NlMsgHdr {
        len: total_len as u32,
        type_: RTM_NEWTCLASS,
        flags: NLM_F_REQUEST | NLM_F_ACK | NLM_F_REPLACE,
        seq,
        pid: 0,
    };
    let mut buf = Vec::with_capacity(total_len);
    buf.extend_from_slice(as_bytes(&hdr));
    buf.extend_from_slice(as_bytes(&tc));
    buf.extend_from_slice(&attrs);
    buf
}

// ── Netlink message parser ────────────────────────────────────────────────────

fn iter_nlmsgs(buf: &[u8]) -> impl Iterator<Item = &[u8]> {
    struct Iter<'a> {
        buf: &'a [u8],
        offset: usize,
    }
    impl<'a> Iterator for Iter<'a> {
        type Item = &'a [u8];
        fn next(&mut self) -> Option<Self::Item> {
            let hdr_size = mem::size_of::<NlMsgHdr>();
            if self.offset + hdr_size > self.buf.len() {
                return None;
            }
            let len = u32::from_ne_bytes(self.buf[self.offset..self.offset + 4].try_into().unwrap())
                as usize;
            if len < hdr_size || self.offset + len > self.buf.len() {
                return None;
            }
            let msg = &self.buf[self.offset..self.offset + len];
            self.offset += (len + 3) & !3; // NLMSG_ALIGN
            Some(msg)
        }
    }
    Iter { buf, offset: 0 }
}

fn nlmsg_type(msg: &[u8]) -> u16 {
    u16::from_ne_bytes([msg[4], msg[5]])
}

fn nlmsg_handle(msg: &[u8]) -> u32 {
    // tcmsg layout: family(1)+pad1(1)+pad2(2)+ifindex(4) = 8 bytes before handle
    let offset = mem::size_of::<NlMsgHdr>() + 8;
    u32::from_ne_bytes(msg[offset..offset + 4].try_into().unwrap())
}

fn parse_nlattrs(data: &[u8]) -> std::collections::HashMap<u16, Vec<u8>> {
    let mut map = std::collections::HashMap::new();
    let mut off = 0;
    while off + 4 <= data.len() {
        let nla_len = u16::from_ne_bytes([data[off], data[off + 1]]) as usize;
        let nla_type = u16::from_ne_bytes([data[off + 2], data[off + 3]]) & !NLA_F_NESTED;
        if nla_len < 4 || off + nla_len > data.len() {
            break;
        }
        map.insert(nla_type, data[off + 4..off + nla_len].to_vec());
        off += (nla_len + 3) & !3;
    }
    map
}

fn extract_htb_opt(msg: &[u8]) -> Option<TcHtbOpt> {
    if nlmsg_type(msg) != RTM_NEWTCLASS {
        return None;
    }
    let min_len = mem::size_of::<NlMsgHdr>() + mem::size_of::<TcMsg>();
    if msg.len() < min_len || nlmsg_handle(msg) != CLASS_LEAF_HANDLE {
        return None;
    }
    let outer = parse_nlattrs(&msg[min_len..]);
    let tca_opts_data = outer.get(&TCA_OPTIONS)?;
    let inner = parse_nlattrs(tca_opts_data);
    let parms = inner.get(&TCA_HTB_PARMS)?;
    if parms.len() < mem::size_of::<TcHtbOpt>() {
        return None;
    }
    let mut opt = TcHtbOpt::default();
    unsafe {
        std::ptr::copy_nonoverlapping(
            parms.as_ptr(),
            &mut opt as *mut TcHtbOpt as *mut u8,
            mem::size_of::<TcHtbOpt>(),
        );
    }
    Some(opt)
}

fn check_ack(buf: &[u8]) -> Result<()> {
    for msg in iter_nlmsgs(buf) {
        if nlmsg_type(msg) == NLMSG_ERROR {
            if msg.len() < 20 {
                anyhow::bail!("NLMSG_ERROR too short");
            }
            let code = i32::from_ne_bytes(msg[16..20].try_into().unwrap());
            if code != 0 {
                return Err(std::io::Error::from_raw_os_error(-code))
                    .context("Netlink RTM_NEWTCLASS returned error");
            }
        }
    }
    Ok(())
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Holds a persistent `NETLINK_ROUTE` socket and cached HTB class parameters
/// for zero-fork, zero-queue-drop rate updates.
pub struct HtbUpdater {
    sock: Socket,
    ifindex: i32,
    cached_opt: TcHtbOpt,
    seq: u32,
}

impl HtbUpdater {
    /// Open a Netlink socket and cache the HTB class parameters that `tc`
    /// computed during initialisation via `RTM_GETTCLASS`.
    pub fn new(iface: &str) -> Result<Self> {
        let ifindex = read_ifindex(iface)?;

        let mut sock = Socket::new(NETLINK_ROUTE).context("open NETLINK_ROUTE socket")?;
        sock.bind_auto().context("bind netlink socket")?;
        sock.connect(&SocketAddr::new(0, 0))
            .context("connect netlink socket to kernel")?;

        let seq = 1;
        let cached_opt = fetch_htb_opt(&mut sock, ifindex, seq)
            .context("RTM_GETTCLASS failed – ensure tc setup ran successfully")?;

        Ok(Self {
            sock,
            ifindex,
            cached_opt,
            seq, // next update will use seq+1, seq+2, …
        })
    }

    /// Apply `bandwidth_kbps` to the HTB leaf class via a single Netlink message.
    ///
    /// Only `rate` and `ceil` are updated; `buffer` / `cbuffer` / `quantum`
    /// keep the values that `tc` computed at setup time.  The leaf `fq_codel`
    /// qdisc is never touched, so no packets are dropped.
    pub fn set_rate(&mut self, bandwidth_kbps: u64) -> Result<()> {
        // 1 kbps = 1000 bit/s = 125 byte/s
        let rate_bytes_per_sec = bandwidth_kbps.saturating_mul(125);

        let mut opt = self.cached_opt;
        opt.rate.rate = rate_bytes_per_sec.min(u32::MAX as u64) as u32;
        opt.ceil.rate = rate_bytes_per_sec.min(u32::MAX as u64) as u32;

        self.seq += 1;
        let msg = build_newtclass_msg(self.seq, self.ifindex, opt, rate_bytes_per_sec);
        self.sock.send(&msg, 0).context("send RTM_NEWTCLASS")?;

        let mut ack = Vec::with_capacity(4096);
        self.sock
            .recv(&mut ack, 0)
            .context("recv RTM_NEWTCLASS ACK")?;
        check_ack(&ack)
    }
}

// ── Private helpers ───────────────────────────────────────────────────────────

fn read_ifindex(iface: &str) -> Result<i32> {
    let path = format!("/sys/class/net/{iface}/ifindex");
    let s = std::fs::read_to_string(&path).with_context(|| {
        format!("Cannot read ifindex for '{iface}' – does the interface exist?")
    })?;
    s.trim()
        .parse::<i32>()
        .with_context(|| format!("Parse ifindex from '{path}'"))
}

fn fetch_htb_opt(sock: &mut Socket, ifindex: i32, seq: u32) -> Result<TcHtbOpt> {
    sock.send(&build_gettclass_msg(seq, ifindex), 0)
        .context("send RTM_GETTCLASS")?;

    let mut full = Vec::new();
    for _ in 0..MAX_RECV_ITERS {
        let mut chunk = Vec::with_capacity(65536);
        sock.recv(&mut chunk, 0)
            .context("recv RTM_GETTCLASS response")?;
        full.extend_from_slice(&chunk);

        let done = iter_nlmsgs(&full).any(|m| nlmsg_type(m) == NLMSG_DONE);
        let err = iter_nlmsgs(&full)
            .last()
            .is_some_and(|m| nlmsg_type(m) == NLMSG_ERROR);
        if done || err {
            break;
        }
    }

    let found = iter_nlmsgs(&full).find_map(extract_htb_opt);
    found.with_context(|| {
        format!(
            "HTB class 1:10 not found in RTM_GETTCLASS dump for ifindex {ifindex}. \
             Ensure tc setup completed before creating HtbUpdater."
        )
    })
}
