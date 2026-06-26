use std::time::Duration;

use pinray::{
    BackendPreference, CaptureEvent, CaptureSession, FrameData, PixelFormat, SourceId,
    VideoCaptureTarget,
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
    for src in &sources {
        println!("    source: {:?}", src);
    }

    println!("[2] building capture session (first display)...");
    let mut session = CaptureSession::builder()
        .backend_preference(BackendPreference::MacScreenCaptureKit)
        .video_target(VideoCaptureTarget::Display(SourceId::new("auto")))
        .pixel_format(PixelFormat::Bgra8888)
        .frame_rate(Some(30))
        .build()?;

    println!("[3] backend: {:?}", session.backend_info().kind);
    println!(
        "    supports_audio={} zero_copy={}",
        session.backend_info().supports_audio,
        session.backend_info().zero_copy,
    );

    println!("[4] starting...");
    session.start()?;

    for idx in 0..5 {
        println!("[5] waiting for event #{idx}...");
        match session.next_event(Some(Duration::from_secs(10)))? {
            CaptureEvent::Video(frame) => {
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

                // Sanity check: data must be non-empty and at least width*height*4 bytes
                assert!(
                    byte_count >= (frame.width * frame.height * 4) as usize,
                    "frame data smaller than expected: got {byte_count} want {}",
                    frame.width * frame.height * 4
                );
            }
            CaptureEvent::Audio(frame) => {
                println!(
                    "[6] audio #{idx}: rate={} ch={} fmt={:?} time_ns={}",
                    frame.sample_rate, frame.channels, frame.sample_format, frame.stream_time_ns,
                );
            }
            other => println!("[6] other event #{idx}: {:?}", other),
        }
    }

    println!("[7] stopping...");
    session.stop()?;
    println!("[8] stop -> start -> stop (lifecycle test)...");

    session.start()?;
    let ev = session.next_event(Some(Duration::from_secs(10)))?;
    println!("[9] second-run event: {:?}", ev.is_video_or_audio());
    session.stop()?;

    println!("[10] done. smoke test passed.");
    Ok(())
}

trait EventKind {
    fn is_video_or_audio(&self) -> &'static str;
}

impl EventKind for CaptureEvent {
    fn is_video_or_audio(&self) -> &'static str {
        match self {
            CaptureEvent::Video(_) => "Video",
            CaptureEvent::Audio(_) => "Audio",
            CaptureEvent::Gap(_) => "Gap",
            CaptureEvent::End => "End",
        }
    }
}
