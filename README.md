# bw-sequencer

**A bandwidth sequencer to emulate dynamic network conditions for mobile IoT devices.**

`bw-sequencer` replays a CSV time-series trace on a Linux network interface,
applying the specified egress bandwidth at each timestamp via the kernel's
**HTB (Hierarchical Token Bucket)** traffic shaper.

---

## Features

- **Zero packet drop on rate change** — rate updates patch only the HTB class
  parameters via a persistent Netlink socket; the leaf `fq_codel` qdisc and its
  queued packets are never touched
- **Arbitrary real-world traces** — accepts any `(time_sec, bandwidth_kbps)`
  CSV; generate traces with Python, MATLAB, or Excel
- **Single static binary** — compiled with musl libc; no dynamic library
  dependencies, no package manager, no kernel modules required

---

## Requirements

| Requirement | Notes |
|---|---|
| Linux kernel ≥ 4.x | `sch_htb` and `sch_fq_codel` modules (present in all mainstream distros: Ubuntu, Alpine, Yocto, …) |
| `iproute2` (`tc` command) | Used at startup and teardown to build / remove the HTB hierarchy |
| `CAP_NET_ADMIN` | Required to configure TC rules (`sudo` or equivalent) |

---

## Quick start

### 1. Download binary

x86_64 Linux:

```bash
wget https://github.com/hiochiai/bw-sequencer/releases/latest/download/bw-sequencer-linux-x86_64 -O bw-sequencer && chmod +x bw-sequencer
```

ARM64 (Raspberry Pi):

```bash
wget https://github.com/hiochiai/bw-sequencer/releases/latest/download/bw-sequencer-linux-aarch64 -O bw-sequencer && chmod +x bw-sequencer
```

### 2. Create a trace CSV

```bash
cat << 'EOF' > trace.csv
time_sec,bandwidth_kbps
0.0,10000
1.0,5000
2.0,1000
3.0,10000
EOF
```

### 3. Run

```bash
sudo ./bw-sequencer --interface eth0 --csv trace.csv
```

---

## Options

```
Usage: bw-sequencer [OPTIONS] --interface <INTERFACE> --csv <CSV>

Options:
  -i, --interface <INTERFACE>  Network interface to throttle (e.g. eth0)
  -c, --csv <CSV>              CSV trace file: columns time_sec,bandwidth_kbps
      --max-drift-ms <MS>      Abort if scheduler wakes >MS ms late [default: 1000]
      --ceil-kbps <KBPS>       HTB ceiling bandwidth in kbps [default: 1000000]
  -h, --help                   Print help
  -V, --version                Print version
```

---

## CSV format

```csv
time_sec,bandwidth_kbps
0.0,10000
1.5,9800
2.1,9200
2.15,8000
10.3,500
```

| Column | Type | Description |
|---|---|---|
| `time_sec` | float | Seconds from start (arbitrary sub-second precision) |
| `bandwidth_kbps` | integer | Egress bandwidth in kbit/s (must be > 0) |

- Lines starting with `#` are comments.
- Rows need not be pre-sorted; they are sorted by `time_sec` at load time.
- bw-sequencer does not generate traces itself — use Python, MATLAB, or Excel.
  See [`examples/generate_trace.py`](examples/generate_trace.py).

---

## Generating traces

```bash
# Sinusoidal oscillation (60 s, 0.5 s resolution)
python3 examples/generate_trace.py sine > sine.csv

# Rapid tunnel entry/exit (50 ms drop, 200 ms recovery)
python3 examples/generate_trace.py tunnel > tunnel.csv

# Realistic 5G highway trace with handover dips
python3 examples/generate_trace.py highway > highway.csv

sudo ./bw-sequencer -i eth0 -c tunnel.csv
```

---

## Real-World Example: Raspberry Pi Router

This example demonstrates how to use a Raspberry Pi as a network router to test `bw-sequencer`'s effect on a target device.

### Topology

The throttler shapes the egress traffic on its LAN-side interface (`eth1`), affecting the download speed of the Target Device.

```
                               +------------------+
                               |  iperf3 Server   |
                               |  (e.g. Docker)   |
                               +------------------+
                                       ^
                                       |
+--------------------------+           |
| Local Network (e.g. LAN) |-----------+
+--------------------------+
        ^
        |
+-------+------------------+
| eth0  (WAN Interface)    |
|                          |
|  Router (Raspberry Pi)   |
| (bw-sequencer installed) |
|                          |
| eth1  (LAN Interface)    |
+-------+------------------+
        |
        | (Traffic to be throttled)
        v
+-------+------------------+
|      Target Device       |
|  (iperf3 client, -R)     |
+--------------------------+
```

### Router Setup (Raspberry Pi)

This setup requires the Raspberry Pi to have two network interfaces: one for the WAN (`eth0`) and one for the LAN (`eth1`). Since most Raspberry Pi models have only one built-in Ethernet port, `eth1` will typically be an external USB-to-LAN adapter.

1.  **Prepare the OS**: Start with a clean Raspberry Pi OS Lite (64-bit) installation and perform basic updates.

    ```bash
    sudo apt-get update
    sudo apt-get upgrade
    ```

2.  **Enable IP Forwarding**: Allow the Raspberry Pi to forward packets between its network interfaces.

    ```bash
    echo "net.ipv4.ip_forward=1" | sudo tee -a /etc/sysctl.conf
    sudo sysctl -p
    ```

3.  **Configure as a Router**: Use NetworkManager to configure the LAN-side interface (`eth1`) in `shared` mode. This automatically sets up a DHCP server and NAT (Masquerade) for the target device network.

    ```bash
    sudo nmcli con add type ethernet ifname eth1 con-name lan-router ipv4.method shared
    sudo nmcli con up lan-router
    ```

4.  **Install bw-sequencer**: Download the pre-compiled binary for ARM64.

    ```bash
    wget https://github.com/your-org/bw-sequencer/releases/latest/download/bw-sequencer-linux-aarch64 
         -O bw-sequencer && chmod +x bw-sequencer
    ```

5.  **Create a Trace File**: Create a sample CSV file for the test. This example trace changes the bandwidth every 10 seconds.

    ```bash
    cat > test_trace.csv << EOF
    time_sec,bandwidth_kbps
    0,100000
    10,50000
    20,10000
    30,100000
    EOF
    ```

### Test Procedure

1.  **Start iperf3 Server**: On a machine on the local network (or the router itself), start the `iperf3` server.

    ```bash
    # Example: running iperf3 server in Docker
    docker run --rm -it -p 5201:5201 networkstatic/iperf3 -s
    ```

2.  **Start iperf3 Client on Target Device**: On the target device, start the `iperf3` client. The `-R` flag is crucial to test in reverse mode (server sends, client receives), which measures the download bandwidth on the target.

    ```bash
    # Replace ${IPERF3_SERVER_IP} with the actual IP of the iperf3 server
    docker run --rm -it networkstatic/iperf3 -c ${IPERF3_SERVER_IP} -t 60 -R
    ```

3.  **Run bw-sequencer on Router**: While the `iperf3` test is running, start `bw-sequencer` on the router, attached to the LAN-side interface (`eth1`).

    ```bash
    sudo ./bw-sequencer -i eth1 -c test_trace.csv
    ```

4.  **Verify Results**: Observe the output from the `iperf3` client. The bitrate should change according to the schedule in the CSV file.

    ```console
    [ ID] Interval           Transfer     Bitrate
    [  5]   0.00-1.00   sec  11.2 MBytes  93.9 Mbits/sec
    ...
    [  5]   9.00-10.00  sec  9.09 MBytes  76.3 Mbits/sec   <-- Rate change begins
    [  5]  10.00-11.00  sec  5.69 MBytes  47.7 Mbits/sec   <-- Throttled to ~50 Mbit/s
    [  5]  11.00-12.00  sec  5.68 MBytes  47.6 Mbits/sec
    [  5]  12.00-13.00  sec  5.68 MBytes  47.7 Mbits/sec
    ...
    [  5]  19.00-20.00  sec  5.71 MBytes  47.9 Mbits/sec   <-- Rate change begins
    [  5]  20.00-21.00  sec  1.13 MBytes  9.47 Mbits/sec   <-- Throttled to ~10 Mbit/s
    ...
    ```

