# Troubleshooting

Start every investigation with two facts: what backend you got and what it says about itself.

```rust,no_run
# let session = pinray::CaptureSession::builder().audio(pinray::AudioCapture::SystemMix).build().unwrap();
println!("{:?}", session.backend_info());
```

And enable logs - every backend traces through the `tracing` crate:

```console
RUST_LOG=debug cargo run ...
```

## Build errors

**`Package libpipewire-0.3 was not found` (Linux)**
Install the PipeWire development package: `sudo apt install libpipewire-0.3-dev clang` (Debian/Ubuntu), `sudo pacman -S pipewire` (Arch), `sudo dnf install pipewire-devel clang` (Fedora).

**`let chains are unstable` or edition errors**
pinray needs Rust 1.88+. `rustup update stable`.

## Linux

**`BackendUnavailable: no wayland session and no DISPLAY`**
Neither `XDG_SESSION_TYPE=wayland`/`WAYLAND_DISPLAY` nor `DISPLAY` is set - you're on a headless host. Video needs a session; audio-only sessions still work if PipeWire runs.

**Portal dialog appears every run**
Capture the restore token: after the first session the portal returns one (logged at info level; API surfacing is on the roadmap), then pass `.restore_token(token)`.

**`GetImage on the root window failed ... Xwayland root is not readable`**
You forced `LinuxX11` inside a Wayland session. Rootless Xwayland has no readable root - use the Wayland backend (`Auto` does this), or run a real Xorg session.

**Audio session builds but `next_audio` always times out**
The sink monitor only produces packets while something plays audio. Silence = no packets on some setups. Play something and retry.

**`wayland event channel disconnected` / "no more input formats"**
The PipeWire connection died. If you see it immediately at start, check the portal + PipeWire versions; this was historically caused by dropping the D-Bus connection while PipeWire used its fd (pinray keeps it alive - if you see this, file a bug with `RUST_LOG=debug` output).

## macOS

**`permission denied` / empty enumeration**
Grant Screen Recording in System Settings → Privacy & Security, then **relaunch** - macOS applies the grant only to new processes.

**Session builds, no frames, no error**
Check the menu-bar screen-recording indicator. If it never appears, the stream didn't start - most likely permission was granted to a different binary (cargo runs a new path per build). Re-grant for the current binary.

## Windows

**`next_event` returns `Timeout` constantly (DXGI)**
Working as designed: DXGI desktop duplication delivers frames on desktop *change*. Move the mouse, play a video, or force WGC (`BackendPreference::WindowsWgc`) for refresh-rate delivery.

**`DuplicateOutput` fails / backend silently becomes WGC**
Duplication is denied on secure desktops (UAC, lock screen), in some RDP sessions, and when another app holds the output's duplication slot. `Auto` falls back to WGC; check `backend_info().kind` to confirm which you got.

**No cursor in frames**
DXGI never draws the cursor. Use WGC.

**Audio frames are all zeros**
WASAPI loopback delivers silence packets when nothing plays (pinray zero-fills `AUDCLNT_BUFFERFLAGS_SILENT` packets). Play audio.

## General

**Audio starves when video is busy**
`next_event` drains pending audio before blocking on video, but if you only ever process video, call `session.next_audio(Some(Duration::ZERO))` in your loop - audio queues are bounded and drop oldest first.

**Frames arrive but colors look swapped**
You assumed RGBA; default is `PixelFormat::Bgra8888`. Either request `.pixel_format(PixelFormat::Rgba8888)` or read `frame.pixel_format`.

**Still stuck?**
Open an issue with: OS + version, `backend_info()` output, `RUST_LOG=debug` log, and which example reproduces it (`wayland_smoke` / `x11_smoke` / `macos_smoke` / `windows_smoke` / `audio_smoke`).
