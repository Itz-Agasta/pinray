use std::time::Duration;

use pinray::{
    AudioCapture, BackendPreference, CaptureEvent, CaptureSession, PixelFormat, SourceId,
    VideoCaptureTarget,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "debug".into()),
        )
        .init();

    println!("[1] building session (video + system audio)...");
    let mut session = CaptureSession::builder()
        .backend_preference(BackendPreference::LinuxWaylandPortal)
        .video_target(VideoCaptureTarget::Display(SourceId::new(
            "portal-default-display",
        )))
        .audio(AudioCapture::SystemMix)
        .pixel_format(PixelFormat::Bgra8888)
        .build()?;

    println!("[2] selected backend: {:?}", session.backend_info().kind);
    println!("[3] starting session...");
    session.start()?;
    println!("[4] session started, entering capture loop...");

    let mut videos = 0u32;
    let mut audios = 0u32;
    for idx in 0..15 {
        match session.next_event(Some(Duration::from_secs(10)))? {
            CaptureEvent::Video(frame) => {
                videos += 1;
                println!(
                    "[5] video #{idx}: {}x{} stride={} format={:?} bytes={}",
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
            CaptureEvent::Audio(frame) => {
                audios += 1;
                println!(
                    "[5] audio #{idx}: rate={} ch={} fmt={:?} bytes={}",
                    frame.sample_rate,
                    frame.channels,
                    frame.sample_format,
                    match frame.data {
                        pinray::AudioData::Interleaved(ref bytes) => bytes.len(),
                        pinray::AudioData::Planar(ref planes) => planes.iter().map(Vec::len).sum(),
                    }
                );
            }
            other => println!("[5] event #{idx}: {:?}", other),
        }
    }

    println!("[6] draining audio explicitly...");
    for idx in 0..3 {
        match session.next_audio(Some(Duration::from_secs(2))) {
            Ok(frame) => {
                audios += 1;
                println!(
                    "[7] audio #{idx}: rate={} ch={}",
                    frame.sample_rate, frame.channels
                );
            }
            Err(pinray::PinrayError::Timeout(_)) => {
                println!("[7] audio #{idx}: timeout (system silent?)")
            }
            Err(error) => return Err(error.into()),
        }
    }

    println!("[8] stopping session...");
    session.stop()?;
    assert!(videos > 0, "expected at least one video frame");
    println!("[9] done. videos={videos} audios={audios}. smoke test passed.");
    Ok(())
}
