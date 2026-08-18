# ioplot

[![Crates.io](https://img.shields.io/crates/v/ioplot.svg)](https://crates.io/crates/ioplot)
[![docs.rs](https://img.shields.io/docsrs/ioplot)](https://docs.rs/ioplot)
[![CI](https://github.com/wabuntu/ioplot/actions/workflows/rust.yml/badge.svg)](https://github.com/wabuntu/ioplot/actions/workflows/rust.yml)

**`iotop` never left the 2000s. `ioplot` is what it would look like if it
were built today.**

Same job — rank processes by live disk I/O — but with a scrolling
read/write history graph on top instead of a bare snapshot, a colorful
heatmap that makes a hot process pop out of the list at a glance, and the
kind of keyboard-driven workflow you expect from a modern TUI: live
sorting, incremental search, pause, adjustable refresh rate, and a
confirm-before-you-regret-it kill dialog — all without leaving the
terminal or memorizing a flag.

Linux only — it reads `/proc/[pid]/io`, the same kernel interface `iotop`
itself uses, which doesn't exist anywhere else.

```
$ sudo ioplot
```

<img src="https://raw.githubusercontent.com/wabuntu/ioplot/main/docs/list.png" alt="ioplot's process list, colored by I/O intensity, with a scrolling read/write history graph on top" width="660">
<img src="https://raw.githubusercontent.com/wabuntu/ioplot/main/docs/detail.png" alt="ioplot's process detail popup showing read/write rate and cumulative totals" width="660">

## Why not just `iotop`?

`iotop` shows you a table. `ioplot` shows you a table *and the shape of
your last two minutes of I/O*, so a spike that already happened doesn't
just vanish — you can see it coming and going in the graph while you dig
into who caused it in the list below.

## Reading the screen

- Two sparklines up top: **Read** (cyan) and **Write** (magenta), each
  showing the system-wide rate over the last ~2 minutes.
- The process table below is colored by how hot each process currently
  is — idle processes stay a muted blue-gray, and the color climbs
  through cyan → yellow → orange → red as a process's I/O rate rises, so
  the busiest thing on the system is the first color your eye catches.
- Reading `/proc/[pid]/io` for processes owned by other users needs root,
  same requirement `iotop` itself has — without it, `ioplot` still works
  for your own processes and says so in the header.

## Keys

- `↑`/`↓`: move the selection
- `Enter`: show read/write totals for the selected process
- `k`: send a signal to the selected process — asks `t` (SIGTERM) or `k`
  (SIGKILL) first, never fires blind
- `s`: cycle the sort column (Total → Read → Write → PID → Name)
- `r`: reverse the current sort
- `/`: filter by process name or PID as you type
- `space` / `p`: pause/resume sampling
- `+` / `-`: speed up/slow down the refresh interval
- `q` / `Esc`: quit

## Usage

```
$ ioplot                    # default: 1s refresh, top 50 processes
$ sudo ioplot --top 100     # see more rows, and other users' processes
$ ioplot --interval 500     # refresh twice a second
```

## Install

- Cargo: `cargo install ioplot`
- Debian package: https://github.com/wabuntu/ioplot/tree/main/target/debian
- RPM package: https://github.com/wabuntu/ioplot/tree/main/target/release/rpmbuild/RPMS/x86_64
- Single binary: https://github.com/wabuntu/ioplot/tree/main/binaries
