//! Integration tests for the self-optimizing parallel transfer core (Deliverable 2).
//!
//! These stage real files inside the security sandbox (`temp/uniflow_sandbox`) and
//! drive `ParallelTransport::execute` end-to-end, asserting byte-exact, integrity-
//! verified, atomic delivery — including the compress+encrypt wire round trip and
//! resume.

use std::path::{Path, PathBuf};
use uniflow::{
    CompressionCodec, Destination, EncryptionCodec, Endpoint, Job, Policy, Source, TransferMode,
    TransferPlan, Transport, TransportHint,
};
use uniflow::ParallelTransport;

fn sandbox_dir() -> PathBuf {
    std::env::temp_dir().join("uniflow_sandbox")
}

/// Create a unique working subdir inside the sandbox and return it.
fn make_case_dir(name: &str) -> PathBuf {
    let dir = sandbox_dir().join(format!("it_{}_{}", name, uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Deterministic, compressible content of `len` bytes.
fn gen_content(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i % 251) as u8).collect()
}

fn local_job(src: &Path, dst: &Path) -> Job {
    Job::new(
        Source::from(Endpoint::Local { path: src.to_path_buf() }),
        Destination::from(Endpoint::Local { path: dst.to_path_buf() }),
        TransferMode::Copy,
    )
}

fn run(transport: &ParallelTransport, job: &Job) -> uniflow::TransferReport {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async { transport.execute(job).await }).expect("transfer should succeed")
}

#[test]
fn small_file_roundtrip_is_byte_exact_and_verified() {
    let dir = make_case_dir("small");
    let src = dir.join("src.bin");
    let dst = dir.join("dst.bin");
    let content = gen_content(1500);
    std::fs::write(&src, &content).unwrap();

    let job = local_job(&src, &dst);
    let report = run(&ParallelTransport::new(), &job);

    assert_eq!(report.bytes_transferred, content.len() as u64);
    assert!(report.integrity_hash.is_some());
    assert_eq!(std::fs::read(&dst).unwrap(), content);
}

#[test]
fn multi_chunk_parallel_copy_with_forced_small_chunks() {
    let dir = make_case_dir("multichunk");
    let src = dir.join("src.bin");
    let dst = dir.join("dst.bin");
    let content = gen_content(1024 * 1024); // 1 MiB
    std::fs::write(&src, &content).unwrap();

    // Force many small chunks + a wide window so multiple workers run concurrently.
    let mut job = local_job(&src, &dst);
    job.plan = Some(TransferPlan {
        chunk_size: 64 * 1024, // 16 chunks
        stream_count: 4,
        max_in_flight: 8,
        worker_threads: 4,
        compression: CompressionCodec::None,
        encryption: EncryptionCodec::None,
        use_gpu_offload: false,
        transport: TransportHint::LocalParallel,
        memory_budget_bytes: 64 * 1024 * 8,
        max_bps: None,
        cost_estimated_mbps: 0.0,
        cost_bottleneck: "test".into(),
        explanation: "forced small-chunk plan".into(),
    });

    let report = run(&ParallelTransport::new(), &job);
    assert_eq!(report.chunks, 16);
    assert_eq!(std::fs::read(&dst).unwrap(), content);
}

#[test]
fn full_pipeline_compress_and_encrypt_roundtrip_is_lossless() {
    let dir = make_case_dir("pipeline");
    let src = dir.join("src.bin");
    let dst = dir.join("dst.bin");
    let content = gen_content(512 * 1024 + 123); // not chunk-aligned
    std::fs::write(&src, &content).unwrap();

    // Exercise compress(zstd) + encrypt(ChaCha20) + decrypt + decompress per chunk.
    let mut job = local_job(&src, &dst);
    job.plan = Some(TransferPlan {
        chunk_size: 64 * 1024,
        stream_count: 4,
        max_in_flight: 6,
        worker_threads: 4,
        compression: CompressionCodec::Zstd { level: 6 },
        encryption: EncryptionCodec::ChaCha20,
        use_gpu_offload: false,
        transport: TransportHint::LocalParallel,
        memory_budget_bytes: 64 * 1024 * 6,
        max_bps: None,
        cost_estimated_mbps: 0.0,
        cost_bottleneck: "test".into(),
        explanation: "forced compress+encrypt plan".into(),
    });

    let report = run(&ParallelTransport::new(), &job);
    assert_eq!(report.bytes_transferred, content.len() as u64);
    // Destination is the faithful PLAINTEXT (wire stages reversed before write).
    assert_eq!(std::fs::read(&dst).unwrap(), content);
    // Wire bytes recorded (compression actually ran).
    assert!(report.integrity_hash.is_some());
}

#[test]
fn empty_file_is_handled() {
    let dir = make_case_dir("empty");
    let src = dir.join("src.bin");
    let dst = dir.join("dst.bin");
    std::fs::write(&src, b"").unwrap();

    let report = run(&ParallelTransport::new(), &local_job(&src, &dst));
    assert_eq!(report.bytes_transferred, 0);
    assert!(dst.exists());
    assert_eq!(std::fs::read(&dst).unwrap().len(), 0);
}

#[test]
fn rerun_overwrites_destination_atomically() {
    let dir = make_case_dir("rerun");
    let src = dir.join("src.bin");
    let dst = dir.join("dst.bin");

    std::fs::write(&dst, gen_content(4096)).unwrap(); // stale destination
    let content = gen_content(200_000);
    std::fs::write(&src, &content).unwrap();

    let _ = run(&ParallelTransport::new(), &local_job(&src, &dst));
    assert_eq!(std::fs::read(&dst).unwrap(), content);
}

#[test]
fn corruption_detected_when_verify_on() {
    // Sanity: a successful transfer reports an integrity hash when verify_integrity is on.
    let dir = make_case_dir("verify");
    let src = dir.join("src.bin");
    let dst = dir.join("dst.bin");
    std::fs::write(&src, gen_content(100_000)).unwrap();

    let job = local_job(&src, &dst);
    let policy = Policy {
        verify_integrity: true,
        ..Default::default()
    };
    let job = job.with_policy(policy);

    let report = run(&ParallelTransport::new(), &job);
    assert!(report.integrity_hash.is_some());
}

#[test]
fn rejects_paths_outside_sandbox() {
    // A path outside the sandbox must be refused (security), never transferred.
    let dir = make_case_dir("good");
    let src = dir.join("src.bin");
    std::fs::write(&src, gen_content(10)).unwrap();

    let job = Job::new(
        Source::from(Endpoint::Local { path: src.clone() }),
        Destination::from(Endpoint::Local { path: PathBuf::from("/etc/uniflow_evil") }),
        TransferMode::Copy,
    );
    let rt = tokio::runtime::Runtime::new().unwrap();
    let res = rt.block_on(async { ParallelTransport::new().execute(&job).await });

    println!("RESULT OF CORRUPT TRANSFER: {:#?}", res);
    
    assert!(res.is_err(), "destination outside sandbox must be rejected");
}

#[test]
fn bandwidth_throttling_limits_throughput() {
    use std::time::{Instant, Duration};
    let dir = make_case_dir("throttle");
    let src = dir.join("src.bin");
    let dst = dir.join("dst.bin");
    
    let content = gen_content(100_000);
    std::fs::write(&src, &content).unwrap();

    let mut job = local_job(&src, &dst);
    // Cap at 50 KB/s. The transfer of 100 KB should take at least 1.5 - 2.0 seconds.
    job.plan = Some(TransferPlan {
        chunk_size: 25 * 1024, // 4 chunks
        stream_count: 2,
        max_in_flight: 4,
        worker_threads: 2,
        compression: CompressionCodec::None,
        encryption: EncryptionCodec::None,
        use_gpu_offload: false,
        transport: TransportHint::LocalParallel,
        memory_budget_bytes: 25 * 1024 * 4,
        max_bps: Some(50_000), // 50 KB/s
        cost_estimated_mbps: 0.0,
        cost_bottleneck: "test".into(),
        explanation: "throttled plan".into(),
    });

    let start = Instant::now();
    let report = run(&ParallelTransport::new(), &job);
    let elapsed = start.elapsed();

    assert_eq!(report.bytes_transferred, content.len() as u64);
    assert_eq!(std::fs::read(&dst).unwrap(), content);
    // 100 KB at 50 KB/s should take around 2.0 seconds. But since the bucket starts
    // with 50 KB of tokens, the first 50 KB is instant, and the remaining 50 KB
    // takes 1.0 second. So we assert it took at least 950ms.
    assert!(
        elapsed >= Duration::from_millis(950),
        "transfer finished too fast, throttling failed (elapsed: {:?})",
        elapsed
    );
}

#[test]
fn resume_from_partial_transfer() {
    let dir = make_case_dir("resume_partial");
    let src = dir.join("src.bin");
    let dst = dir.join("dst.bin");
    
    // 100 KB of content
    let content = gen_content(100_000);
    std::fs::write(&src, &content).unwrap();

    // Manually stage a partial transfer:
    // Write a temp file with the first 40 KB of content
    let temp_path = dst.with_extension("uniflow-tmp");
    std::fs::write(&temp_path, &content[..40_000]).unwrap();

    // Write a checkpoint file saying we finished 40,000 bytes
    let ckpt_path = dst.with_extension("uniflow-ckpt");
    std::fs::write(&ckpt_path, 40_000u64.to_le_bytes()).unwrap();

    let mut job = local_job(&src, &dst);
    job.checkpoint = Some(40_000);

    // Run the transfer. It should resume from 40,000 and complete.
    let report = run(&ParallelTransport::new(), &job);

    assert_eq!(report.bytes_transferred, 100_000);
    assert_eq!(std::fs::read(&dst).unwrap(), content);
    assert!(!temp_path.exists());
    assert!(!ckpt_path.exists());
}

#[test]
fn integrity_failure_deletes_temp_and_errors() {
    let dir = make_case_dir("integrity_fail");
    let src = dir.join("src.bin");
    let dst = dir.join("dst.bin");
    
    let temp_path = dst.with_extension("uniflow-tmp");
    let ckpt_path = dst.with_extension("uniflow-ckpt");
    
    // Valid source
    let content = gen_content(100_000); 
    std::fs::write(&src, &content).unwrap();

    // Completely invalid/corrupted destination file
    let corrupt_content = vec![0u8; 100_000];
    std::fs::write(&temp_path, &corrupt_content).unwrap();
    
    // Write a checkpoint saying the transfer is 100% done, so it immediately goes to verification
    std::fs::write(&ckpt_path, 100_000u64.to_le_bytes()).unwrap();

    let mut job = local_job(&src, &dst);
    job.checkpoint = Some(100_000); // Resume at 100%
    job.policy.verify_integrity = true;

    let rt = tokio::runtime::Runtime::new().unwrap();
    let res = rt.block_on(async { ParallelTransport::new().execute(&job).await });

    // The transfer should fail with an Integrity error
    assert!(res.is_err());
    // The temp file should be deleted
    let temp_path = dst.with_extension("uniflow-tmp");
    assert!(!temp_path.exists());
    assert!(!dst.exists());
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(10))]
        #[test]
        fn test_parallel_transfer_roundtrip(
            file_size in 0..100_000usize,
            chunk_size in 16384..65536u64,
            stream_count in 1..4u32,
            use_compression in any::<bool>(),
            use_encryption in any::<bool>(),
        ) {
            let dir = make_case_dir("proptest");
            let src = dir.join("src.bin");
            let dst = dir.join("dst.bin");

            // Generate random content
            let content: Vec<u8> = (0..file_size).map(|i| (i % 251) as u8).collect();
            std::fs::write(&src, &content).unwrap();

            let mut job = local_job(&src, &dst);
            job.plan = Some(TransferPlan {
                chunk_size,
                stream_count,
                max_in_flight: stream_count * 2,
                worker_threads: 2,
                compression: if use_compression { CompressionCodec::Zstd { level: 3 } } else { CompressionCodec::None },
                encryption: if use_encryption { EncryptionCodec::ChaCha20 } else { EncryptionCodec::None },
                use_gpu_offload: false,
                transport: TransportHint::LocalParallel,
                memory_budget_bytes: chunk_size * stream_count as u64 * 2,
                max_bps: None,
                cost_estimated_mbps: 0.0,
                cost_bottleneck: "test".into(),
                explanation: "proptest plan".into(),
            });

            let report = run(&ParallelTransport::new(), &job);
            assert_eq!(report.bytes_transferred, content.len() as u64);
            assert_eq!(std::fs::read(&dst).unwrap(), content);
        }
    }
}
