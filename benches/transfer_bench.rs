//! `cargo bench` suite for the parallel transfer core (the "PROVE IT" deliverable).
//!
//! Scenarios (per the task):
//!   * large single file
//!   * many small files
//!   * high-latency link (modelled via a WAN-tuned plan: small chunks, many streams)
//!   * CPU-bound (compress + encrypt) vs IO-bound (raw copy)
//!
//! Each ParallelTransport run is compared against a **naive single-stream baseline**
//! (sequential read-all + write-all). Results feed BENCHMARKS.md.
//!
//! NOTE: files are staged inside the security sandbox (`temp/uniflow_sandbox`).

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use uniflow::{
    CompressionCodec, Destination, EncryptionCodec, Endpoint, Job, Source, TransferMode, TransferPlan,
    Transport, TransportHint,
};
use uniflow::ParallelTransport;

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn sandbox() -> PathBuf {
    let d = std::env::temp_dir().join("uniflow_sandbox").join("bench");
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn gen(len: usize) -> Vec<u8> {
    // Mildly compressible (so the CPU-bound zstd path does real work).
    (0..len).map(|i| ((i / 64) % 251) as u8).collect()
}

fn stage_file(name: &str, len: usize) -> PathBuf {
    let p = sandbox().join(name);
    std::fs::write(&p, gen(len)).unwrap();
    p
}

fn local_job(src: &Path, dst: &Path, plan: Option<TransferPlan>) -> Job {
    let mut job = Job::new(
        Source::from(Endpoint::Local { path: src.to_path_buf() }),
        Destination::from(Endpoint::Local { path: dst.to_path_buf() }),
        TransferMode::Copy,
    );
    job.plan = plan;
    job
}

fn plan(chunk: u64, streams: u32, in_flight: u32, comp: CompressionCodec, enc: EncryptionCodec) -> TransferPlan {
    TransferPlan {
        chunk_size: chunk,
        stream_count: streams,
        max_in_flight: in_flight,
        worker_threads: in_flight,
        compression: comp,
        encryption: enc,
        use_gpu_offload: false,
        transport: TransportHint::LocalParallel,
        memory_budget_bytes: chunk * in_flight as u64,
        max_bps: None,
        cost_estimated_mbps: 0.0,
        cost_bottleneck: "bench".into(),
        explanation: "bench plan".into(),
    }
}

/// Naive single-stream baseline: read whole file, write whole file.
fn naive_copy(src: &Path, dst: &Path) {
    let data = std::fs::read(src).unwrap();
    std::fs::write(dst, &data).unwrap();
}

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap()
}

fn unique_dst() -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    sandbox().join(format!("dst_{n}.bin"))
}

fn bench_large_file(c: &mut Criterion) {
    let size = 16 * 1024 * 1024; // 16 MiB
    let src = stage_file("large_src.bin", size);
    let transport = ParallelTransport::new();
    let rt = rt();

    let mut group = c.benchmark_group("large_single_file");
    group.throughput(Throughput::Bytes(size as u64));

    group.bench_function(BenchmarkId::new("parallel_core", "16MiB"), |b| {
        b.iter(|| {
            let dst = unique_dst();
            let job = local_job(&src, &dst, Some(plan(4 * 1024 * 1024, 4, 8, CompressionCodec::None, EncryptionCodec::None)));
            rt.block_on(async { transport.execute(&job).await.unwrap() });
            let _ = std::fs::remove_file(&dst);
        })
    });

    group.bench_function(BenchmarkId::new("naive_single_stream", "16MiB"), |b| {
        b.iter(|| {
            let dst = unique_dst();
            naive_copy(&src, &dst);
            let _ = std::fs::remove_file(&dst);
        })
    });

    group.finish();
}

fn bench_many_small_files(c: &mut Criterion) {
    let count = 200;
    let each = 4 * 1024; // 4 KiB
    let srcs: Vec<PathBuf> = (0..count).map(|i| stage_file(&format!("small_{i}.bin"), each)).collect();
    let transport = ParallelTransport::new();
    let rt = rt();

    let mut group = c.benchmark_group("many_small_files");
    group.throughput(Throughput::Bytes((count * each) as u64));

    group.bench_function(BenchmarkId::new("parallel_core", "200x4KiB"), |b| {
        b.iter(|| {
            for (i, src) in srcs.iter().enumerate() {
                let dst = sandbox().join(format!("small_dst_{i}.bin"));
                let job = local_job(src, &dst, Some(plan(64 * 1024, 2, 4, CompressionCodec::None, EncryptionCodec::None)));
                rt.block_on(async { transport.execute(&job).await.unwrap() });
            }
        })
    });

    group.bench_function(BenchmarkId::new("naive_single_stream", "200x4KiB"), |b| {
        b.iter(|| {
            for (i, src) in srcs.iter().enumerate() {
                let dst = sandbox().join(format!("small_naive_{i}.bin"));
                naive_copy(src, &dst);
            }
        })
    });

    group.finish();
}

fn bench_high_latency_link(c: &mut Criterion) {
    // Modelled: a WAN-tuned plan (small chunks + many streams) over local files,
    // measuring the configuration overhead the planner would pick for a fat, slow link.
    let size = 8 * 1024 * 1024;
    let src = stage_file("wan_src.bin", size);
    let transport = ParallelTransport::new();
    let rt = rt();

    let mut group = c.benchmark_group("high_latency_link_modelled");
    group.throughput(Throughput::Bytes(size as u64));
    group.bench_function(BenchmarkId::new("parallel_core", "wan_plan"), |b| {
        b.iter(|| {
            let dst = unique_dst();
            let job = local_job(&src, &dst, Some(plan(1024 * 1024, 16, 16, CompressionCodec::Zstd { level: 6 }, EncryptionCodec::ChaCha20)));
            rt.block_on(async { transport.execute(&job).await.unwrap() });
            let _ = std::fs::remove_file(&dst);
        })
    });
    group.finish();
}

fn bench_cpu_vs_io_bound(c: &mut Criterion) {
    let size = 8 * 1024 * 1024;
    let src = stage_file("cpuio_src.bin", size);
    let transport = ParallelTransport::new();
    let rt = rt();

    let mut group = c.benchmark_group("cpu_vs_io_bound");
    group.throughput(Throughput::Bytes(size as u64));

    // IO-bound: no compression/encryption (pure copy pipeline).
    group.bench_function(BenchmarkId::new("io_bound", "raw"), |b| {
        b.iter(|| {
            let dst = unique_dst();
            let job = local_job(&src, &dst, Some(plan(4 * 1024 * 1024, 4, 8, CompressionCodec::None, EncryptionCodec::None)));
            rt.block_on(async { transport.execute(&job).await.unwrap() });
            let _ = std::fs::remove_file(&dst);
        })
    });

    // CPU-bound: zstd-9 + AES-GCM per chunk (compute dominates).
    group.bench_function(BenchmarkId::new("cpu_bound", "zstd9+aes"), |b| {
        b.iter(|| {
            let dst = unique_dst();
            let job = local_job(&src, &dst, Some(plan(2 * 1024 * 1024, 4, 8, CompressionCodec::Zstd { level: 9 }, EncryptionCodec::AesGcm)));
            rt.block_on(async { transport.execute(&job).await.unwrap() });
            let _ = std::fs::remove_file(&dst);
        })
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_large_file,
    bench_many_small_files,
    bench_high_latency_link,
    bench_cpu_vs_io_bound
);
criterion_main!(benches);
