# Getting started

This guide takes you from `cargo add` to your first captured frames, then covers picking a source, adding audio, tuning the session, and writing frames to a video file. The full API reference is on [docs.rs/pinray](https://docs.rs/pinray).

> docs.rs can't link every OS's native libraries in one build (its Linux builders have no `libpipewire`), so it builds the `pinray` and `pinray-platform-*` crates for one fixed target each. The page you land on may say Windows or macOS even if you target Linux, but the types are identical on every platform. [docs.rs/pinray-core](https://docs.rs/pinray-core) has no native dependencies and always builds on the default target.

## Install

```console
cargo add pinray
```

Linux needs the PipeWire headers and `clang` to build (see [Platform support](https://github.com/Itz-Agasta/pinray/blob/main/docs/platforms.md#linux)). macOS and Windows need nothing extra.

## Capture your first frames

This program captures 60 events from the primary display and prints each frame's size and timestamp:

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
            CaptureEvent::Video(frame) => println!(
                "{}x{} at t={}ns",
                frame.width, frame.height, frame.stream_time_ns
            ),
            CaptureEvent::Gap(gap) => eprintln!("dropped frames: {gap:?}"),
            _ => {}
        }
    }
    session.stop()?;
    Ok(())
}
```

`SourceId::new("auto")` targets the primary display. On Wayland, the compositor shows a permission dialog instead, and you capture whatever the user picks there. Each frame's pixels are in `frame.data` as `FrameData::Host(Vec<u8>)`, laid out as rows of `frame.stride` bytes in `frame.pixel_format` (BGRA by default).

## Pick a specific display or window

`pinray::enumerate_sources()` lists displays, windows, and audio sources. This finds a Firefox window and captures it:

```rust,no_run
use pinray::{CaptureSource, CaptureSession, VideoCaptureTarget};

let sources = pinray::enumerate_sources()?;
let window = sources.iter().find_map(|s| match s {
    CaptureSource::Window(w) if w.title.contains("Firefox") => Some(w.id.clone()),
    _ => None,
});
if let Some(id) = window {
    let session = CaptureSession::builder()
        .video_target(VideoCaptureTarget::Window(id))
        .build()?;
}
# Ok::<(), pinray::PinrayError>(())
```

Window ids stop working when the window closes, so enumerate right before you build the session. Wayland doesn't list windows, and its display ids aren't enforced, because the portal dialog decides what you capture.

## Add system audio

Add `.audio(AudioCapture::SystemMix)` to capture everything the system plays alongside the video:

```rust,no_run
use pinray::{AudioCapture, CaptureSession, SourceId, VideoCaptureTarget};

let mut session = CaptureSession::builder()
    .video_target(VideoCaptureTarget::Display(SourceId::new("auto")))
    .audio(AudioCapture::SystemMix)
    .build()?;
session.start()?;
# Ok::<(), pinray::PinrayError>(())
```

Audio arrives from `next_event` as `CaptureEvent::Audio`, mixed in with video. On Linux and Windows you can also read audio on its own with `session.next_audio(timeout)`. On macOS, audio only comes through `next_event`, and `next_audio` returns `BackendNotSelected` ([#34](https://github.com/Itz-Agasta/pinray/issues/34)).

### Capture audio without video

Leave out `video_target` to capture audio only. On Linux and Windows this never shows a permission dialog. macOS still needs Screen Recording permission, because ScreenCaptureKit captures audio.

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

On macOS, call `next_event` and match `CaptureEvent::Audio` instead of `next_audio`. The runnable version is `cargo run --example audio_smoke` (Linux and Windows).

Only `AudioCapture::SystemMix` is implemented, meaning loopback of everything the system plays. `AudioCapture::Microphone` exists in the enum, but no backend supports it yet, and `build()` returns `PinrayError::Unsupported` before a session is created.

## Tune the session

Every setting has a default, so set only the ones you need:

```rust,no_run
use pinray::{BackendPreference, CaptureSession, CursorMode, PixelFormat, Rect};

let crop = Rect { x: 0, y: 0, width: 1280, height: 720 };
let builder = CaptureSession::builder()
    .backend_preference(BackendPreference::Auto)
    .pixel_format(PixelFormat::Rgba8888)
    .cursor_mode(CursorMode::Hidden)
    .crop_rect(Some(crop))
    .frame_rate(Some(30))
    .queue_depth(4);
```

What each setting does:

- **`backend_preference`**: `Auto` picks per platform, or force a backend such as `WindowsWgc` or `LinuxX11`
- **`pixel_format`**: BGRA is native everywhere, and `Rgba8888` costs a CPU swizzle
- **`cursor_mode`**: DXGI never draws the cursor, whatever you set
- **`crop_rect`**: ignored on Wayland ([#30](https://github.com/Itz-Agasta/pinray/issues/30))
- **`frame_rate`**: a maximum, not a guarantee (see [Frame rate](#frame-rate))
- **`queue_depth`**: how many frames are buffered before new ones are dropped

Backend selection differs per machine, so check what you got and include it in bug reports:

```rust,no_run
# let session = pinray::CaptureSession::builder().audio(pinray::AudioCapture::SystemMix).build()?;
let info = session.backend_info();
println!("{:?}: audio={} notes={}", info.kind, info.supports_audio, info.notes);
# Ok::<(), pinray::PinrayError>(())
```

## Timestamps, sequence numbers, and gaps

Each frame carries timing data you can use to detect drops and line up audio with video:

- **`stream_time_ns`**: monotonic, and comparable between a session's audio and video. The epoch differs per platform (boot time on macOS and Windows, process start on Linux), so compute deltas rather than comparing across machines
- **`sequence`**: goes up by one per captured frame, per stream, including frames dropped because your queue was full. A jump means you fell behind. Frames skipped by `frame_rate` pacing aren't counted
- **`CaptureEvent::Gap`**: reports drops and backend restarts explicitly

## Write frames to a video file

pinray doesn't encode, so piping `VideoFrame` and `AudioFrame` bytes into ffmpeg or another encoder is up to you. Two mistakes here produce a file that plays fine but is wrong.

### Row padding

`stride` can be wider than `width * bytes_per_pixel` because of platform row alignment. If you copy `FrameData::Host` bytes straight into a `rawvideo` pipe, every row after the first shifts into a diagonal smear. Use [`VideoFrame::to_tight_bytes`] instead, which strips the padding:

```rust,no_run
# use pinray::{CaptureEvent, CaptureSession};
# fn handle(session: &mut CaptureSession) -> Result<(), Box<dyn std::error::Error>> {
if let CaptureEvent::Video(frame) = session.next_event(None)? {
    let tight = frame
        .to_tight_bytes()
        .expect("Host frame, packed pixel format");
    // write `tight`, not `frame.data`, to your rawvideo pipe
}
# Ok(())
# }
```

### Frame rate

`frame_rate` is a maximum, not a guarantee. Wayland, DXGI, and WGC deliver frames when the screen changes, not on a fixed clock. If you declare a constant `-framerate` to ffmpeg's `rawvideo` demuxer and stream whatever arrives, the recording plays back too fast or too slow. `-shortest` then cuts off the longer stream, usually audio, without a warning.

Fix this with `stream_time_ns`. Work out how many output frames are due from the elapsed time, and repeat the last frame to fill the gap:

```rust,no_run
# use pinray::{CaptureEvent, CaptureSession};
# fn write_frame_to_pipe(_bytes: &[u8]) {}
# fn pace(session: &mut CaptureSession) -> Result<(), Box<dyn std::error::Error>> {
let frame_interval_ns = 1_000_000_000 / 30; // 30 fps output
let mut next_due_ns: Option<i64> = None;
let mut last_frame: Vec<u8> = Vec::new();

loop {
    let CaptureEvent::Video(frame) = session.next_event(None)? else {
        continue;
    };
    let tight = frame.to_tight_bytes().expect("packed Host frame");
    let due = next_due_ns.get_or_insert(frame.stream_time_ns);
    while frame.stream_time_ns >= *due {
        let out = if last_frame.is_empty() { &tight } else { &last_frame };
        write_frame_to_pipe(out);
        *due += frame_interval_ns;
    }
    last_frame = tight;
}
# }
```

This holds a constant output rate by duplicating frames. It's the minimum fix, not a variable-frame-rate muxer. For anything longer than a short recording, use a muxing library or encoder that accepts a timestamp per frame, and derive it from `stream_time_ns`.

## Runnable examples

Each example builds a session, reads a short burst of events, and prints what it receives:

```console
cargo run --example wayland_smoke    # Linux Wayland: video + audio
cargo run --example x11_smoke        # Linux X11: polling video + audio
cargo run --example audio_smoke      # Linux/Windows: audio only, no dialogs
cargo run --example macos_smoke      # macOS
cargo run --example windows_smoke    # Windows (PINRAY_BACKEND=wgc|dxgi to force)
```
