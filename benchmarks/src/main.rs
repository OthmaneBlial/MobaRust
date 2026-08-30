use mobarust_core::{OutputBatcher, SessionRecord};
use std::hint::black_box;
use std::time::{Duration, Instant};

const ITERATIONS: usize = 5;
const BATCH_BYTES: usize = 32 * 1024;

fn main() {
    println!("# MobaRust synthetic benchmarks");
    println!("environment.os={}", std::env::consts::OS);
    println!("environment.arch={}", std::env::consts::ARCH);
    println!(
        "environment.parallelism={}",
        std::thread::available_parallelism()
            .map(|count| count.get())
            .unwrap_or(1)
    );
    println!("iterations={ITERATIONS}");
    println!("terminal_batch_bytes={BATCH_BYTES}");
    println!();

    for lines in [10_000_usize, 100_000, 1_000_000] {
        let payload = synthetic_terminal_output(lines);
        let (elapsed, chunks) = measure(|| batch_terminal_output(&payload));
        let bytes_per_second = (payload.len() as f64 * ITERATIONS as f64)
            / elapsed.as_secs_f64().max(f64::MIN_POSITIVE);
        println!(
            "terminal_output lines={lines} bytes={} chunks={} mean_ms={:.3} throughput_mib_s={:.2}",
            payload.len(),
            chunks,
            elapsed.as_secs_f64() * 1000.0 / ITERATIONS as f64,
            bytes_per_second / (1024.0 * 1024.0)
        );
    }

    let profiles = synthetic_profiles(10_000);
    let (elapsed, matches) = measure(|| search_profiles(&profiles, "fixture-09999"));
    println!(
        "session_search profiles={} matches={} mean_us={:.3}",
        profiles.len(),
        matches,
        elapsed.as_secs_f64() * 1_000_000.0 / ITERATIONS as f64
    );

    let session = SessionRecord::local_terminal("benchmark fixture");
    let (elapsed, serialized_bytes) = measure(|| {
        serde_json::to_vec(black_box(&session))
            .expect("local session serialization")
            .len()
    });
    println!(
        "session_serialization fields=secret-free bytes={} mean_us={:.3}",
        serialized_bytes,
        elapsed.as_secs_f64() * 1_000_000.0 / ITERATIONS as f64
    );
}

fn measure<F, T>(mut operation: F) -> (Duration, T)
where
    F: FnMut() -> T,
    T: Copy,
{
    let started = Instant::now();
    let mut value = operation();
    for _ in 1..ITERATIONS {
        value = operation();
    }
    (started.elapsed(), black_box(value))
}

fn synthetic_terminal_output(lines: usize) -> Vec<u8> {
    let mut output = Vec::with_capacity(lines * 64);
    for index in 0..lines {
        output.extend_from_slice(b"\x1b[32mfixture\x1b[0m ");
        output.extend_from_slice(index.to_string().as_bytes());
        output.extend_from_slice(b" unicode=\xe2\x9c\x93 ansi=\x1b[2K\r\n");
    }
    output
}

fn batch_terminal_output(payload: &[u8]) -> usize {
    let mut batcher = OutputBatcher::new(BATCH_BYTES);
    let mut chunks = 0;
    for read in payload.chunks(4096) {
        chunks += batcher.push(black_box(read)).len();
    }
    if batcher.flush().is_some() {
        chunks += 1;
    }
    black_box(chunks)
}

fn synthetic_profiles(count: usize) -> Vec<String> {
    (0..count)
        .map(|index| format!("ops@fixture-{index:05}.local · tag=staging"))
        .collect()
}

fn search_profiles(profiles: &[String], query: &str) -> usize {
    let query = query.to_ascii_lowercase();
    let matches = profiles
        .iter()
        .filter(|profile| profile.to_ascii_lowercase().contains(&query))
        .count();
    black_box(matches)
}
