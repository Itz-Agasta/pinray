# Troubleshooting

Fixes for common build errors and capture problems, grouped by platform. Before digging in, find out which backend you got and what it says about itself:

```rust,no_run
# fn main() -> Result<(), pinray::PinrayError> {
let session = pinray::CaptureSession::builder()
    .audio(pinray::AudioCapture::SystemMix)
    .build()?;
println!("{:?}", session.backend_info());
# Ok(())
# }
```

Then turn on logging. Every backend logs through the [`tracing`](https://crates.io/crates/tracing) crate, and the examples install a subscriber:

```console
RUST_LOG=debug cargo run --example audio_smoke
```

## Build errors

**`Package libpipewire-0.3 was not found` (Linux)**: install the PipeWire development headers and `clang`.

```console
sudo apt install libpipewire-0.3-dev clang   # Debian/Ubuntu
sudo pacman -S pipewire clang                # Arch
sudo dnf install pipewire-devel clang        # Fedora
```

**`let chains are unstable` or edition errors**: pinray needs Rust 1.88 or later. Run `rustup update stable`.

## Linux

**`BackendUnavailable: no wayland session and no DISPLAY`**: neither `WAYLAND_DISPLAY`/`XDG_SESSION_TYPE=wayland` nor `DISPLAY` is set, so you're on a headless host. Video needs a desktop session, but audio-only sessions still work if PipeWire is running.

**The portal dialog appears on every run**: save the restore token. After building the first session, read `session.restore_token()`, store it, and pass it to `.restore_token(token)` next time.

**`build()` returns `Unsupported("portal request cancelled by user")`**: the user clicked Cancel in the portal dialog. This should be a permission error rather than `Unsupported`, which is tracked in [#29](https://github.com/Itz-Agasta/pinray/issues/29).

**`GetImage on the root window failed ... Xwayland root is not readable`**: you forced `LinuxX11` inside a Wayland session, and rootless Xwayland has no readable root window. Use `Auto` (which picks the Wayland backend) or run a real Xorg session.

**An audio session builds but `next_audio` always times out**: the sink monitor only produces packets while something is playing, and some setups send nothing during silence. Play audio and try again.

**`wayland event channel disconnected` or "no more input formats"**: the PipeWire connection died. If it happens right at start, check your portal and PipeWire versions. This used to happen when the D-Bus connection closed while PipeWire still used its file descriptor. pinray keeps that connection alive now, so if you see it, file a bug with the `RUST_LOG=debug` output.

## macOS

**Permission denied, or `enumerate_sources()` returns nothing**: grant Screen Recording in System Settings → Privacy & Security, then relaunch your app. macOS only applies the grant to new processes.

**The session builds but no frames or errors arrive**: check the menu bar for the screen-recording indicator. If it never appears, the stream didn't start, most likely because permission was granted to a different binary. Cargo can build to a new path, so grant access again for the current binary.

**`next_audio()` returns `BackendNotSelected`**: on macOS, audio arrives through `next_event` as `CaptureEvent::Audio` ([#34](https://github.com/Itz-Agasta/pinray/issues/34)). Read audio from `next_event` instead.

## Windows

**`next_event` keeps returning `Timeout` (DXGI)**: this is expected. DXGI Desktop Duplication only produces a frame when the desktop changes. Move the mouse or play a video, or force `BackendPreference::WindowsWgc` to get frames at the refresh rate.

**`DuplicateOutput` fails, or the backend quietly becomes WGC**: duplication is denied on secure desktops (UAC prompts, the lock screen), in some Remote Desktop sessions, and when another app already duplicates that output. `Auto` falls back to WGC; check `backend_info().kind` to see which one you got.

**No cursor in frames**: DXGI never draws the cursor. Force `BackendPreference::WindowsWgc`.

**Audio frames are all zeros**: WASAPI loopback sends silent packets when nothing is playing, and pinray zero-fills them. Play audio.

## All platforms

**Audio drops while video is busy**: `next_event` returns pending audio before it waits for video, but audio queues are bounded and drop packets when you fall behind. If your loop spends a long time per video frame, also poll `session.next_audio(Some(Duration::ZERO))` between frames (Linux and Windows).

**Colors look swapped**: the default pixel format is `PixelFormat::Bgra8888`, not RGBA. Request `.pixel_format(PixelFormat::Rgba8888)` or read `frame.pixel_format`.

**Every row after the first is shifted (diagonal smear)**: rows are padded to `stride`. Use `frame.to_tight_bytes()` instead of copying `FrameData::Host` directly.

## Reporting a bug

If none of this helps, [open a bug report](https://github.com/Itz-Agasta/pinray/issues/new?template=bug.yml) with your OS and version, the `backend_info()` output, a `RUST_LOG=debug` log, and the example that reproduces it (`wayland_smoke`, `x11_smoke`, `macos_smoke`, `windows_smoke`, or `audio_smoke`).
