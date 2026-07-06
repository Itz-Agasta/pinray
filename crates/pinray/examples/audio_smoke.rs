//! Audio-only smoke test: captures the system mix without any video target.
//! Works on Linux (PipeWire sink monitor, no portal dialog) and Windows
//! (WASAPI loopback). Play some audio while it runs.
//!
//! ```text
//! cargo run --example audio_smoke
//! ```

use std::time::Duration;

use pinray::{AudioCapture, CaptureSession};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "debug".into()),
        )
        .init();

    println!("[1] building audio-only session...");
    let mut session = CaptureSession::builder()
        .audio(AudioCapture::SystemMix)
        .build()?;

    println!("[2] backend: {:?}", session.backend_info().kind);
    println!("[3] starting...");
    session.start()?;

    let mut received = 0u32;
    for idx in 0..20 {
        match session.next_audio(Some(Duration::from_secs(3))) {
            Ok(frame) => {
                received += 1;
                let byte_count = match &frame.data {
                    pinray::AudioData::Interleaved(bytes) => bytes.len(),
                    pinray::AudioData::Planar(planes) => planes.iter().map(Vec::len).sum(),
                };
                println!(
                    "[4] audio #{idx}: seq={} rate={} ch={} fmt={:?} bytes={}",
                    frame.sequence, frame.sample_rate, frame.channels, frame.sample_format, byte_count,
                );
            }
            Err(pinray::PinrayError::Timeout(_)) => {
                println!("[4] audio #{idx}: timeout (is anything playing?)");
            }
            Err(error) => return Err(error.into()),
        }
    }

    println!("[5] stop -> start -> stop (lifecycle test)...");
    session.stop()?;
    session.start()?;
    match session.next_audio(Some(Duration::from_secs(3))) {
        Ok(_) => println!("[6] second-run audio received"),
        Err(pinray::PinrayError::Timeout(_)) => println!("[6] second-run: timeout"),
        Err(error) => return Err(error.into()),
    }
    session.stop()?;

    assert!(received > 0, "expected at least one audio frame");
    println!("[7] done. {received} audio frames. smoke test passed.");
    Ok(())
}
