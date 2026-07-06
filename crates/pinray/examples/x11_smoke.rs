//! X11 smoke test: source enumeration, polling display capture with system
//! audio, and a stop → start → stop lifecycle pass.
//!
//! Run in an X11 session (or under Xwayland for mechanics-only testing —
//! rootless Xwayland roots usually produce black frames):
//!
//! ```text
//! cargo run --example x11_smoke
//! ```

use std::time::Duration;

use pinray::{
    AudioCapture, BackendPreference, CaptureEvent, CaptureSession, FrameData, PixelFormat,
    SourceId, VideoCaptureTarget,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "debug".into()),
        )
        .init();

    println!("[1] enumerating sources...");
    let sources = pinray::enumerate_sources()?;
    for src in sources.iter().take(12) {
        println!("    source: {src:?}");
    }
    println!("    ({} sources total)", sources.len());

    // PINRAY_NO_AUDIO=1 skips audio for hosts without a PipeWire daemon
    // (e.g. CI runners under Xvfb).
    let with_audio = std::env::var_os("PINRAY_NO_AUDIO").is_none();

    println!("[2] building capture session (primary monitor, audio={with_audio})...");
    let mut builder = CaptureSession::builder()
        .backend_preference(BackendPreference::LinuxX11)
        .video_target(VideoCaptureTarget::Display(SourceId::new("auto")))
        .pixel_format(PixelFormat::Bgra8888)
        .frame_rate(Some(15));
    if with_audio {
        builder = builder.audio(AudioCapture::SystemMix);
    }
    let mut session = builder.build()?;

    println!("[3] backend: {:?}", session.backend_info().kind);
    println!("[4] starting...");
    session.start()?;

    let mut videos = 0u32;
    let mut audios = 0u32;
    for idx in 0..15 {
        match session.next_event(Some(Duration::from_secs(5)))? {
            CaptureEvent::Video(frame) => {
                videos += 1;
                let byte_count = match &frame.data {
                    FrameData::Host(b) => b.len(),
                    _ => 0,
                };
                println!(
                    "[5] video #{idx}: {}x{} stride={} seq={} time_ns={} bytes={}",
                    frame.width,
                    frame.height,
                    frame.stride,
                    frame.sequence,
                    frame.stream_time_ns,
                    byte_count,
                );
                assert_eq!(byte_count, (frame.width * frame.height * 4) as usize);
            }
            CaptureEvent::Audio(frame) => {
                audios += 1;
                println!(
                    "[5] audio #{idx}: rate={} ch={} fmt={:?}",
                    frame.sample_rate, frame.channels, frame.sample_format,
                );
            }
            other => println!("[5] event #{idx}: {other:?}"),
        }
    }

    println!("[6] stop -> start -> stop (lifecycle test)...");
    session.stop()?;
    session.start()?;
    let event = session.next_event(Some(Duration::from_secs(5)))?;
    println!(
        "[7] second-run event: {}",
        match event {
            CaptureEvent::Video(_) => "Video",
            CaptureEvent::Audio(_) => "Audio",
            CaptureEvent::Gap(_) => "Gap",
            CaptureEvent::End => "End",
        }
    );
    session.stop()?;

    assert!(videos > 0, "expected at least one video frame");
    println!("[8] done. videos={videos} audios={audios}. smoke test passed.");
    Ok(())
}
