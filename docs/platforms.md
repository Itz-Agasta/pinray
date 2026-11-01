# Platform support

This page lists what each backend can capture, what it needs to build and run, and where it falls short. At runtime, `pinray::available_backends()` lists the compiled-in backends and `session.backend_info()` tells you which one a session picked.

## Feature matrix

| | Linux Wayland | Linux X11 | macOS | Windows DXGI | Windows WGC |
|---|---|---|---|---|---|
| Display capture | ✅ | ✅ | ✅ | ✅ | ✅ |
| Window capture | ❌ (planned) | ⚠️ visible windows only | ✅ | ❌ (API limit) | ✅ |
| Delivery model | streaming | polling | streaming | on change | streaming |
| System audio | ✅ PipeWire | ✅ PipeWire | ✅ ScreenCaptureKit | ✅ WASAPI | ✅ WASAPI |
| Cursor embed/hide | ✅ (portal mode) | ✅ (XFixes blend) | ✅ | ❌ never drawn | ✅ |
| Crop rect | ❌ ignored ([#30](https://github.com/Itz-Agasta/pinray/issues/30)) | ✅ | ⚠️ upscaled ([#22](https://github.com/Itz-Agasta/pinray/issues/22)) | ✅ | ✅ |
| `frame_rate(None)` | uncapped | 30 fps | 60 fps | every change | full refresh |
| `next_audio()` | ✅ | ✅ | ❌ use `next_event` ([#34](https://github.com/Itz-Agasta/pinray/issues/34)) | ✅ | ✅ |
| Timestamp epoch | process start | process start | boot | boot | boot |

Not implemented anywhere yet: microphone capture (`build()` returns `Unsupported`), zero-copy GPU frames (every frame is `FrameData::Host`), and the builder's `color_space` option ([#32](https://github.com/Itz-Agasta/pinray/issues/32)).

The builder's default `frame_rate` is `Some(60)`. The `frame_rate(None)` row shows what each backend does when you remove the cap.

## Linux

Building needs the PipeWire headers and `clang` for bindgen: `libpipewire-0.3-dev` on Debian/Ubuntu, `pipewire` on Arch, `pipewire-devel` on Fedora. X11 support is pure Rust ([x11rb](https://crates.io/crates/x11rb)) and needs no extra libraries. At runtime you need a PipeWire daemon, plus XDG Desktop Portal for Wayland video.

`Auto` picks the Wayland portal in a Wayland session and X11 when only `DISPLAY` is set. Audio-only sessions use PipeWire directly and need neither.

### Wayland portal

Building a session opens the compositor's screen-share dialog, and whatever the user picks there is what you capture. The display id you pass isn't enforced. Pass `.restore_token(...)` from a previous session to skip the dialog. Window capture through the portal isn't wired up yet.

The compositor decides when frames arrive and only sends one when the screen content changes, so capture can run below the monitor's refresh rate even with `frame_rate(None)`. `frame_rate(Some(n))` is sent to the portal as a maximum, and how strictly it's enforced depends on the portal.

### X11

X11 has no damage-driven streaming API, so this backend polls with `GetImage` at `frame_rate`. Treat it as a fast screenshot loop rather than a low-latency recorder. The cursor is alpha-blended in via XFixes, and window capture works while the window is visible.

Inside a Wayland session, Xwayland's root window isn't readable, so forcing `LinuxX11` there fails for display capture. It only works on a real Xorg session.

### Audio

The PipeWire backend captures the default sink's monitor, meaning everything the system plays. It works in any session type, shows no dialog, and pairs with either video backend or runs on its own. When you fall behind, the oldest queued packets are dropped first.

Timestamps are monotonic from process start, because current PipeWire bindings don't expose buffer timestamps. They're consistent within a session but not anchored to boot.

## macOS

pinray needs macOS 13.0 or later. ScreenCaptureKit itself starts at 12.3, but pinray's stream setup uses audio settings that only exist on 13.0. There are no build-time extras.

### Screen Recording permission

macOS asks for Screen Recording access under System Settings → Privacy & Security the first time you call `enumerate_sources()` or build a session. This applies to audio-only sessions too. After a fresh grant, relaunch the app: macOS only applies it to new processes.

### Capture behavior

Display capture, window capture, cursor toggling, and system audio all go through one `SCStream`. Because of that, audio arrives through `next_event` and `next_audio()` returns `BackendNotSelected` ([#34](https://github.com/Itz-Agasta/pinray/issues/34)). Frames are BGRA copies in host memory, and requesting `Rgba8888` swizzles on the CPU.

Window capture is sized from the window's pixel size when the session starts, rounded down to even dimensions with a 2×2 minimum. On macOS 14+ that size comes from the content filter. On macOS 13 it's the window frame times the scale of the display under the window's center, falling back to the main display, then 1×. The output size stays fixed for the whole session, even if the window is resized or moved ([#25](https://github.com/Itz-Agasta/pinray/issues/25)).

Display capture uses the content filter's pixel size on macOS 14+. On macOS 13 it divides the display mode's pixel width by its point width, falling back to 1× if that fails. Output is rounded down to even dimensions with a 2×2 minimum.

### Known issues

These need checking on real hardware:

- **Crop rect**: output is upscaled to the full display size ([#22](https://github.com/Itz-Agasta/pinray/issues/22))
- **Audio layout**: system audio may be planar rather than interleaved ([#24](https://github.com/Itz-Agasta/pinray/issues/24))
- **Source enumeration**: `scale_factor_milli` is a placeholder 1000, `is_primary` is always `false`, and display `width`/`height` are in points ([#23](https://github.com/Itz-Agasta/pinray/issues/23))

## Windows

pinray needs Windows 10 or later, and WGC needs version 1903 or later. There are no build-time extras.

`Auto` uses DXGI for displays and falls back to WGC if duplication is denied (secure desktop, session policy). Window targets always use WGC, because DXGI can't capture a single window.

### DXGI Desktop Duplication

DXGI only produces a frame when the desktop changes, so an idle desktop returns `Timeout`, which is expected. Pacing to `frame_rate` delays the next acquire until the frame is due, so a skipped frame is never copied. Pacing is a ceiling: a desktop that changes more slowly than the requested rate delivers at its own rate.

DXGI never draws the cursor into frames. The cursor arrives as metadata that pinray doesn't composite yet, so use WGC when you need it.

### Windows Graphics Capture (WGC)

WGC streams at the display's refresh rate, paced to `frame_rate`. It supports the cursor toggle and hides the yellow capture border where the OS allows. Pacing happens in the arrival callback before the GPU-to-CPU copy, so a throttled frame costs only a timestamp read. On Windows 11 builds with `IGraphicsCaptureSession5`, pinray also asks the compositor to throttle at the source with `MinUpdateInterval`.

Frames skipped by pacing aren't reported as `Gap` events, because you asked for that rate. Pacing can only select whole vblanks, so when the refresh rate isn't a multiple of the requested rate (60 fps on a 90 Hz panel, say), the spacing between frames alternates while the average stays at the requested rate.

### Audio

WASAPI captures the default output device in shared-mode loopback. The format follows the device mix, usually 48 kHz stereo `F32`. It pairs with either video backend or runs on its own. When the queue is full, new packets are dropped.

Video and audio timestamps both come from the performance counter (QPC), anchored to boot.

## Writing cross-platform consumers

Code against the frame metadata rather than platform assumptions:

- **`stride`**: rows can be padded, so use `VideoFrame::to_tight_bytes()` before handing pixels to an encoder
- **`pixel_format`**: the default is BGRA, not RGBA
- **`Timeout`**: normal on idle DXGI desktops and between X11 polls; call again
- **`Gap` events and `sequence` jumps**: your signal that frames were dropped, which matters when you mux audio and video
- **Audio draining**: read audio promptly. Every backend's queue is bounded and drops packets when you fall behind, and `next_event` only returns pending audio before it blocks on video
