# UniFlow Transfer Benchmarks

Benchmarks for the self-optimizing parallel transfer core, via
`cargo bench` (criterion). Source: `benches/transfer_bench.rs`.

```powershell
cd D:\uniflow
cargo bench
# HTML reports: target/criterion/report/index.html
```

## Scenarios

Per the task's "PROVE IT" requirement, four scenarios are measured, each comparing
`ParallelTransport` (profile-tuned, multi-worker, AIMD) against a **naive
single-stream baseline** (`std::fs::read` + `std::fs::write`):

| Scenario | What it stresses | Bench group |
|---|---|---|
| Large single file (16 MiB) | sequential throughput, mmap read, positioned writes | `large_single_file` |
| Many small files (200 × 4 KiB) | per-file overhead, pool spin-up | `many_small_files` |
| High-latency link (modelled) | WAN-tuned plan: small chunks + 16 streams + zstd6 + ChaCha20 | `high_latency_link_modelled` |
| CPU-bound vs IO-bound | zstd-9 + AES-GCM (compute) vs raw copy (IO) | `cpu_vs_io_bound` |

### Methodology notes / honesty caveats

- All files are staged inside the security sandbox (`temp/uniflow_sandbox`).
- The **high-latency link** scenario is *modelled*: real network latency cannot be
  injected without a network harness, so we instead run the exact plan the Planner
  would select for a fat, slow WAN (1 MiB chunks, 16 streams, zstd-6, ChaCha20) over
  local files. It measures the *configuration overhead* of that plan, not wire RTT.
- The **baseline** is a genuinely naive whole-file read-then-write, which is the
  fairest "single-stream, no pipeline" comparison point. The legacy
  `LocalDeltaTransport` is **not** used as a throughput baseline because its delta
  step is a Phase-1 stub (returns a fabricated byte count) and would not be a
  meaningful copy-throughput comparison.
- Numbers are machine-specific; reproduce on your hardware. The table below is from
  the development machine (Windows 11, see `## Environment`).

## Results

> Populated by the `cargo bench` run during verification. Each cell is criterion's
> median estimate; throughput is criterion's `Throughput::Bytes` MB/s.

### Environment

| | |
|---|---|
| OS | Windows 11 (x86_64-pc-windows-msvc) |
| CPU | _filled from profiler output at bench time_ |
| RAM | _filled from profiler output_ |
| Storage | _filled from profiler output_ |
| rustc | 1.96.0 |

### large_single_file (16 MiB)

| Implementation | Median time | Throughput | Speedup vs naive |
|---|---|---|---|
| `parallel_core` | _tbd_ | _tbd_ MB/s | _tbd_ |
| `naive_single_stream` | _tbd_ | _tbd_ MB/s | 1.00× |

### many_small_files (200 × 4 KiB)

| Implementation | Median time | Throughput | Speedup vs naive |
|---|---|---|---|
| `parallel_core` | _tbd_ | _tbd_ MB/s | _tbd_ |
| `naive_single_stream` | _tbd_ | _tbd_ MB/s | 1.00× |

### high_latency_link_modelled (8 MiB, WAN plan)

| Implementation | Median time | Throughput |
|---|---|---|
| `parallel_core` (16 streams, zstd6+ChaCha20) | _tbd_ | _tbd_ MB/s |

### cpu_vs_io_bound (8 MiB)

| Mode | Median time | Throughput |
|---|---|---|
| `io_bound` (raw) | _tbd_ | _tbd_ MB/s |
| `cpu_bound` (zstd9+AES-GCM) | _tbd_ | _tbd_ MB/s |

## Interpreting the numbers

- On a **large file**, the parallel core should approach the storage sequential
  ceiling (it overlaps read/hash/write across workers), while the naive baseline is
  bounded by one read + one write with no overlap. Expect the parallel core to match
  or beat the baseline; on fast NVMe with small files the pipeline overhead can make
  them comparable — the planner accounts for this by sizing chunks to the medium.
- **Many small files** is dominated by per-file fixed costs (open, profile/plan,
  fsync, rename). This is where careful chunk/worker sizing matters most and where a
  naive copy can be competitive; the value of the engine here is correctness
  (atomic, verified, resumable), not raw small-file speed.
- **CPU-bound vs IO-bound** shows the cost the planner is reasoning about: zstd-9 +
  AES-GCM deliberately makes compute the bottleneck, which is exactly when the
  planner would *disable* compression for a fast local link (see the cost model in
  `docs/architecture.md`).
