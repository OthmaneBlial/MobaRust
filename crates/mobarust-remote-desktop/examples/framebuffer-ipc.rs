//! In-memory native helper codec probe; no GUI, network, or remote credentials.
use mobarust_remote_desktop::{HelperEvent, decode_event_frame, encode_event_frame};
use std::hint::black_box;
use std::time::{Duration, Instant};

const SAMPLES: usize = 5;

fn main() {
    println!(
        "framebuffer_ipc os={} arch={} debug={} samples={SAMPLES}",
        std::env::consts::OS,
        std::env::consts::ARCH,
        cfg!(debug_assertions),
    );
    for (width, height) in [(320_u16, 200_u16), (640, 400), (1280, 720), (1920, 1080)] {
        let pixels: Vec<u8> = [0x23, 0x67, 0xab, 0xff]
            .into_iter()
            .cycle()
            .take(usize::from(width) * usize::from(height) * 4)
            .collect();
        let raw_bytes = pixels.len();
        let event = HelperEvent::Framebuffer {
            width,
            height,
            pixels,
        };
        let mut encode_times = Vec::with_capacity(SAMPLES);
        let mut decode_times = Vec::with_capacity(SAMPLES);
        let mut wire_bytes = 0;
        for _ in 0..SAMPLES {
            let started = Instant::now();
            let frame = encode_event_frame(black_box(&event));
            encode_times.push(started.elapsed());
            let frame = match frame {
                Ok(frame) => frame,
                Err(error) => {
                    println!(
                        "framebuffer_ipc {width}x{height} raw_bytes={raw_bytes} encode_error={error}"
                    );
                    break;
                }
            };
            wire_bytes = frame.len();
            let started = Instant::now();
            let decoded =
                decode_event_frame(black_box(&frame)).expect("decode synthetic framebuffer");
            decode_times.push(started.elapsed());
            assert_eq!(decoded, event, "synthetic framebuffer round trip");
        }
        if decode_times.len() == SAMPLES {
            println!(
                "framebuffer_ipc {width}x{height} raw_bytes={raw_bytes} wire_bytes={wire_bytes} encode_ms(min/median/max)={} decode_ms(min/median/max)={}",
                timings(&mut encode_times),
                timings(&mut decode_times),
            );
        }
    }
}

fn timings(samples: &mut [Duration]) -> String {
    samples.sort_unstable();
    format!(
        "{:.3}/{:.3}/{:.3}",
        samples[0].as_secs_f64() * 1000.0,
        samples[samples.len() / 2].as_secs_f64() * 1000.0,
        samples[samples.len() - 1].as_secs_f64() * 1000.0
    )
}
