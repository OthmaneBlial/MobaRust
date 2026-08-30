use mobarust_core::{OutputBatcher, SessionRecord};
use std::hint::black_box;
use std::time::{Duration, Instant};

const ITERATIONS: usize = 5;
const BATCH_BYTES: usize = 32 * 1024;

#[derive(Clone, Copy, Debug)]
enum TerminalFixture {
    Mixed,
    LongUnicodeAnsi,
    ProgressRedraw,
}

impl TerminalFixture {
    const fn label(self) -> &'static str {
        match self {
            Self::Mixed => "mixed-utf8-ansi",
            Self::LongUnicodeAnsi => "long-unicode-ansi",
            Self::ProgressRedraw => "progress-redraw",
        }
    }
}

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
        benchmark_terminal_fixture(TerminalFixture::Mixed, lines);
    }
    benchmark_terminal_fixture(TerminalFixture::LongUnicodeAnsi, 10_000);
    benchmark_terminal_fixture(TerminalFixture::ProgressRedraw, 100_000);

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

fn benchmark_terminal_fixture(fixture: TerminalFixture, lines: usize) {
    let payload = synthetic_terminal_output(lines, fixture);
    let (elapsed, chunks) = measure(|| batch_terminal_output(&payload));
    let bytes_per_second =
        (payload.len() as f64 * ITERATIONS as f64) / elapsed.as_secs_f64().max(f64::MIN_POSITIVE);
    println!(
        "terminal_output fixture={} lines={lines} bytes={} chunks={} mean_ms={:.3} throughput_mib_s={:.2}",
        fixture.label(),
        payload.len(),
        chunks,
        elapsed.as_secs_f64() * 1000.0 / ITERATIONS as f64,
        bytes_per_second / (1024.0 * 1024.0)
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

fn synthetic_terminal_output(lines: usize, fixture: TerminalFixture) -> Vec<u8> {
    let estimated_bytes_per_line = match fixture {
        TerminalFixture::Mixed => 64,
        TerminalFixture::LongUnicodeAnsi => 1_000,
        TerminalFixture::ProgressRedraw => 96,
    };
    let mut output = Vec::with_capacity(lines.saturating_mul(estimated_bytes_per_line));
    for index in 0..lines {
        match fixture {
            TerminalFixture::Mixed => match index % 3 {
                0 => output.extend_from_slice(
                    format!(
                        "\x1b[32mfixture\x1b[0m line={index:07} unicode=✓ λ 🚀\n"
                    )
                    .as_bytes(),
                ),
                1 => output.extend_from_slice(
                    format!(
                        "\x1b[1;38;5;45mfixture\x1b[0m \x1b[48;5;236mline={index:07}\x1b[0m ansi=heavy\n"
                    )
                    .as_bytes(),
                ),
                _ => output.extend_from_slice(
                    format!(
                        "\r\x1b[2K\x1b[1;34mprogress {index:07}\x1b[0m 67% ✓\n"
                    )
                    .as_bytes(),
                ),
            },
            TerminalFixture::LongUnicodeAnsi => {
                output.extend_from_slice(
                    format!("\x1b[38;5;208mline={index:07} unicode=λ界🚀 ").as_bytes(),
                );
                output.extend(std::iter::repeat_n(b'x', 768));
                output.extend_from_slice(b"\x1b[0m end=\xE2\x9C\x93\n");
            }
            TerminalFixture::ProgressRedraw => output.extend_from_slice(
                format!(
                    "\r\x1b[2K\x1b[1;36mupload {index:07}\x1b[0m [##########..........] {index}%"
                )
                .as_bytes(),
            ),
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixtures_cover_unicode_ansi_and_redraw_controls() {
        let mixed = synthetic_terminal_output(3, TerminalFixture::Mixed);
        assert!(mixed.windows(3).any(|window| window == b"\xe2\x9c\x93"));
        assert!(mixed.windows(2).any(|window| window == b"\x1b["));
        assert!(mixed.contains(&b'\r'));

        let long = synthetic_terminal_output(1, TerminalFixture::LongUnicodeAnsi);
        assert!(long.len() > 768);
        assert!(long.ends_with(b"end=\xE2\x9C\x93\n"));

        let progress = synthetic_terminal_output(2, TerminalFixture::ProgressRedraw);
        assert!(progress.starts_with(b"\r\x1b[2K"));
        assert!(!progress.contains(&b'\n'));
    }

    #[test]
    fn batching_stays_bounded_for_each_fixture() {
        for fixture in [
            TerminalFixture::Mixed,
            TerminalFixture::LongUnicodeAnsi,
            TerminalFixture::ProgressRedraw,
        ] {
            let payload = synthetic_terminal_output(64, fixture);
            let chunks = batch_terminal_output(&payload);
            assert_eq!(chunks, payload.len().div_ceil(BATCH_BYTES));
        }
    }
}
