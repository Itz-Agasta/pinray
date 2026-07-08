# Getting Started

pinray captures screens, windows, and system audio through each OS's native API and hands you raw frames. This guide walks from install to your first frames. Full API reference: [docs.rs/pinray](https://docs.rs/pinray).

> docs.rs builds the `pinray` and `pinray-platform-*` crates for a fixed target per platform (docs.rs can't link the native libraries for every OS at once, e.g. no `libpipewire` on their Linux builders) - the badge you land on may say Windows or macOS even if you're targeting Linux. The type definitions are identical on every platform regardless of which one the page was built for. For a build that's never redirected, see [docs.rs/pinray-core](https://docs.rs/pinray-core) - it has no native dependencies and always builds on the plain default target.

## Install

```console
cargo add pinray
```

Linux needs build-time system libraries (see [platforms.md](https://github.com/Itz-Agasta/pinray/blob/main/docs/platforms.md#linux)); macOS and Windows need nothing extra.

## Capture your first frames

```rust,no_run
use std::time::Duration;
use pinray::{CaptureEvent, CaptureSession, SourceId, VideoCaptureTarget};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut session = CaptureSession::builder()
        .video_target(VideoCaptureTarget::Display(SourceId::new("auto")))
        .build()?;

    session.start()?;
    for _ in 0..60 {
        match session.next_event(Some(Duration::from_secs(5)))? {
            CaptureEvent::Video(frame) => {
                // frame.data is FrameData::Host(Vec<u8>) - packed rows of
                // frame.stride bytes, frame.pixel_format (BGRA by default)
                println!("{}x{} at t={}ns", frame.width, frame.height, frame.stream_time_ns);
            }
            CaptureEvent::Gap(gap) => eprintln!("dropped frames: {:?}", gap),
            _ => {}
        }
    }
    session.stop()?;
    Ok(())
}
```

`SourceId::new("auto")` targets the primary display. On Wayland the compositor shows a permission dialog instead - whatever the user picks there is what you capture.

## Pick a specific display or window

```rust,no_run
use pinray::{CaptureSource, CaptureSession, VideoCaptureTarget};

let sources = pinray::enumerate_sources().unwrap();
let window = sources.iter().find_map(|s| match s {
    CaptureSource::Window(w) if w.title.contains("Firefox") => Some(w.id.clone()),
    _ => None,
});
if let Some(id) = window {
    let session = CaptureSession::builder()
        .video_target(VideoCaptureTarget::Window(id))
        .build();
}
```

Window ids die with the window - enumerate right before building the session.

## Add system audio

```rust,no_run
use std::time::Duration;
use pinray::{AudioCapture, CaptureSession, SourceId, VideoCaptureTarget};

let mut session = CaptureSession::builder()
    .video_target(VideoCaptureTarget::Display(SourceId::new("auto")))
    .audio(AudioCapture::SystemMix)
    .build()
    .unwrap();
session.start().unwrap();
```

Audio arrives as `CaptureEvent::Audio` interleaved with video from `next_event`, or drain it directly with `session.next_audio(timeout)`.

### Audio only, no video

You don't need a `video_target` at all - an audio-only session is a first-class use case, not a side effect of the video API, and it never shows a permission dialog on Linux/Windows:

```rust,no_run
use std::time::Duration;
use pinray::{AudioCapture, CaptureSession};

let mut session = CaptureSession::builder()
    .audio(AudioCapture::SystemMix)
    .build()?;

session.start()?;
let frame = session.next_audio(Some(Duration::from_secs(3)))?;
println!("{} Hz, {} ch", frame.sample_rate, frame.channels);
session.stop()?;
# Ok::<(), pinray::PinrayError>(())
```

Runnable version: `cargo run --example audio_smoke`.

**Only `AudioCapture::SystemMix` (loopback / sink monitor - "everything the system plays") is implemented today.** `AudioCapture::Microphone(SourceId)` exists in the enum but no backend implements it yet - `.build()` itself fails with `PinrayError::Unsupported`, before a session is ever created. If you need mic input, pinray can't do that yet; track backend support before building on this path.

## Tuning

```rust,no_run
use pinray::{BackendPreference, CaptureSession, CursorMode, PixelFormat, Rect};

let builder = CaptureSession::builder()
    .backend_preference(BackendPreference::Auto) // or force WindowsWgc, LinuxX11, ...
    .pixel_format(PixelFormat::Rgba8888)         // BGRA is native everywhere; RGBA costs a swizzle
    .cursor_mode(CursorMode::Hidden)
    .crop_rect(Some(Rect { x: 0, y: 0, width: 1280, height: 720 }))
    .frame_rate(Some(30))
    .queue_depth(4);                             // frames buffered before drops
```

Check what you actually got:

```rust,no_run
# let session = pinray::CaptureSession::builder().audio(pinray::AudioCapture::SystemMix).build().unwrap();
let info = session.backend_info();
println!("{:?}: audio={} notes={}", info.kind, info.supports_audio, info.notes);
```

Include that output in bug reports - backend selection differs per machine.

## Timestamps, sequences, gaps

- `stream_time_ns` is monotonic and comparable between a session's audio and video streams. The epoch differs per platform (boot time on macOS/Windows, process-relative on Linux) - compute deltas, don't compare across machines.
- `sequence` increments once per delivered frame per stream; a jump means the consumer fell behind and frames were dropped.
- `CaptureEvent::Gap` reports drops and backend restarts explicitly.

## Muxing frames into a video file

pinray deliberately doesn't encode (see the crate docs) - piping `VideoFrame`/`AudioFrame` bytes into ffmpeg (or another encoder) is on you. Two mistakes are easy to make here and both produce a file that opens and plays fine while being silently wrong:

**1. Row padding.** `stride` can be wider than `width * bytes_per_pixel` (platform row alignment). Copying `FrameData::Host` bytes straight into a `rawvideo` pipe skews every row after the first into a diagonal smear whenever that happens. Always go through [`VideoFrame::to_tight_bytes`] instead of touching `data` directly:

```rust,no_run
# use pinray::{CaptureEvent, CaptureSession};
# fn handle(session: &mut CaptureSession) -> Result<(), Box<dyn std::error::Error>> {
if let CaptureEvent::Video(frame) = session.next_event(None)? {
    let tight = frame.to_tight_bytes().expect("Host frame, packed pixel format");
    // write `tight`, not `frame.data`, to your rawvideo pipe
}
# Ok(())
# }
```

**2. Frame rate.** `frame_rate` on the builder is a request, not a guarantee - Wayland portals and other push-driven backends deliver frames when the compositor damages the screen, not on a fixed clock. If you declare a constant `-framerate` to ffmpeg's `rawvideo` demuxer and just stream whatever arrives, a real-time recording plays back sped up or slowed down by however far the actual delivery rate was from what you declared, and `-shortest` will silently truncate whichever stream (usually audio) is longer. `stream_time_ns` is exactly what you need to fix this: track how many output frames are "due" by wall-clock delta and duplicate the last frame to fill the gap, instead of assuming one input frame equals one output frame:

```rust,no_run
# use pinray::{CaptureEvent, CaptureSession};
# fn write_frame_to_pipe(_bytes: &[u8]) {}
# fn pace(session: &mut CaptureSession) -> Result<(), Box<dyn std::error::Error>> {
let target_fps = 30i64;
let frame_interval_ns = 1_000_000_000 / target_fps;
let mut next_due_ns: Option<i64> = None;
let mut last_frame: Vec<u8> = Vec::new();

loop {
    let CaptureEvent::Video(frame) = session.next_event(None)? else { continue };
    let tight = frame.to_tight_bytes().expect("Host frame, packed pixel format");
    let due = next_due_ns.get_or_insert(frame.stream_time_ns + frame_interval_ns);
    while frame.stream_time_ns >= *due {
        write_frame_to_pipe(if last_frame.is_empty() { &tight } else { &last_frame });
        *due += frame_interval_ns;
    }
    last_frame = tight;
}
# }
```

This duplicates frames to hit a constant declared rate rather than letting a variable capture cadence desync from wall-clock time. It's the minimum fix, not a full VFR-aware muxer - for anything beyond a quick recording, prefer a muxing library or encoder invocation that accepts explicit per-frame PTS derived from `stream_time_ns` instead of a declared constant rate.

## Runnable examples

```console
cargo run --example wayland_smoke    # Linux Wayland: video + audio
cargo run --example x11_smoke        # Linux X11: polling video + audio
cargo run --example audio_smoke      # Linux/Windows: audio only, no dialogs
cargo run --example macos_smoke      # macOS
cargo run --example windows_smoke    # Windows (PINRAY_BACKEND=wgc|dxgi to force)
```
