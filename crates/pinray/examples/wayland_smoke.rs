use std::time::Duration;

use pinray::{
    BackendPreference, CaptureEvent, CaptureSession, PixelFormat, SourceId, VideoCaptureTarget,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "debug".into()),
        )
        .init();

    println!("[1] building session...");
    let mut session = CaptureSession::builder()
        .backend_preference(BackendPreference::LinuxWaylandPortal)
        .video_target(VideoCaptureTarget::Display(SourceId::new(
            "portal-default-display",
        )))
        .pixel_format(PixelFormat::Bgra8888)
        .build()?;

    println!("[2] selected backend: {:?}", session.backend_info().kind);
    println!("[3] starting session...");
    session.start()?;
    println!("[4] session started, entering capture loop...");

    for idx in 0..5 {
        println!("[5] waiting for event #{idx}...");
        match session.next_event(Some(Duration::from_secs(10)))? {
            CaptureEvent::Video(frame) => {
                println!(
                    "[6] frame #{idx}: {}x{} stride={} format={:?} bytes={}",
                    frame.width,
                    frame.height,
                    frame.stride,
                    frame.pixel_format,
                    match frame.data {
                        pinray::FrameData::Host(ref bytes) => bytes.len(),
                        _ => 0,
                    }
                );
            }
            other => println!("[6] event #{idx}: {:?}", other),
        }
    }

    println!("[7] stopping session...");
    session.stop()?;
    println!("[8] done.");
    Ok(())
}
