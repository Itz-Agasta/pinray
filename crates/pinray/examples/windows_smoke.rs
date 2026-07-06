//! Windows smoke test: source enumeration, DXGI/WGC display capture with
//! WASAPI loopback audio, and a stop → start → stop lifecycle pass.
//!
//! Run on a real Windows host:
//!
//! ```text
//! cargo run --example windows_smoke            # Auto policy (DXGI first)
//! PINRAY_BACKEND=wgc cargo run --example windows_smoke
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

    let preference = match std::env::var("PINRAY_BACKEND").as_deref() {
        Ok("wgc") => BackendPreference::WindowsWgc,
        Ok("dxgi") => BackendPreference::WindowsDxgi,
        _ => BackendPreference::Auto,
    };

    println!("[1] available backends:");
    for backend in pinray::available_backends() {
        println!("    {:?}: {}", backend.kind, backend.notes);
    }

    println!("[2] enumerating sources...");
    let sources = pinray::enumerate_sources()?;
    for src in sources.iter().take(15) {
        println!("    source: {src:?}");
    }
    println!("    ({} sources total)", sources.len());

    println!("[3] building capture session (primary display + system audio)...");
    let mut session = CaptureSession::builder()
        .backend_preference(preference)
        .video_target(VideoCaptureTarget::Display(SourceId::new("auto")))
        .audio(AudioCapture::SystemMix)
        .pixel_format(PixelFormat::Bgra8888)
        .build()?;

    println!("[4] backend: {:?}", session.backend_info().kind);
    println!(
        "    supports_audio={} zero_copy={}",
        session.backend_info().supports_audio,
        session.backend_info().zero_copy,
    );

    println!("[5] starting...");
    session.start()?;

    let mut videos = 0u32;
    let mut audios = 0u32;
    for idx in 0..10 {
        // DXGI only delivers frames when the desktop changes, so a timeout
        // here is not fatal — move the mouse or play a video while testing.
        match session.next_event(Some(Duration::from_secs(5))) {
            Ok(CaptureEvent::Video(frame)) => {
                videos += 1;
                let byte_count = match &frame.data {
                    FrameData::Host(b) => b.len(),
                    _ => 0,
                };
                println!(
                    "[6] video #{idx}: {}x{} stride={} format={:?} time_ns={} bytes={}",
                    frame.width,
                    frame.height,
                    frame.stride,
                    frame.pixel_format,
                    frame.stream_time_ns,
                    byte_count,
                );
                assert!(
                    byte_count >= (frame.width * frame.height * 4) as usize,
                    "frame data smaller than expected: got {byte_count} want {}",
                    frame.width * frame.height * 4
                );
            }
            Ok(CaptureEvent::Audio(frame)) => {
                audios += 1;
                println!(
                    "[6] audio #{idx}: rate={} ch={} fmt={:?} time_ns={}",
                    frame.sample_rate, frame.channels, frame.sample_format, frame.stream_time_ns,
                );
            }
            Ok(other) => println!("[6] other event #{idx}: {other:?}"),
            Err(pinray::PinrayError::Timeout(_)) => {
                println!("[6] event #{idx}: timeout (desktop idle?)");
            }
            Err(error) => return Err(error.into()),
        }
    }

    println!("[7] draining audio explicitly...");
    for idx in 0..3 {
        match session.next_audio(Some(Duration::from_secs(2))) {
            Ok(frame) => {
                audios += 1;
                println!(
                    "[8] audio #{idx}: rate={} ch={} samples_bytes={}",
                    frame.sample_rate,
                    frame.channels,
                    match &frame.data {
                        pinray::AudioData::Interleaved(b) => b.len(),
                        pinray::AudioData::Planar(p) => p.iter().map(Vec::len).sum(),
                    }
                );
            }
            Err(pinray::PinrayError::Timeout(_)) => println!("[8] audio #{idx}: timeout"),
            Err(error) => return Err(error.into()),
        }
    }

    println!("[9] stopping...");
    session.stop()?;

    println!("[10] stop -> start -> stop (lifecycle test)...");
    session.start()?;
    match session.next_event(Some(Duration::from_secs(5))) {
        Ok(event) => println!("[11] second-run event kind: {}", event_kind(&event)),
        Err(pinray::PinrayError::Timeout(_)) => println!("[11] second-run: timeout"),
        Err(error) => return Err(error.into()),
    }
    session.stop()?;

    println!("[12] done. videos={videos} audios={audios}. smoke test passed.");
    Ok(())
}

fn event_kind(event: &CaptureEvent) -> &'static str {
    match event {
        CaptureEvent::Video(_) => "Video",
        CaptureEvent::Audio(_) => "Audio",
        CaptureEvent::Gap(_) => "Gap",
        CaptureEvent::End => "End",
    }
}
