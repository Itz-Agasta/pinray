# Platform Support

What each backend can do, what it needs, and where it's honest about limits. `pinray::available_backends()` reports this at runtime; `session.backend_info()` tells you which backend a session actually picked.

## Feature matrix

| | Linux Wayland | Linux X11 | macOS | Windows DXGI | Windows WGC |
|---|---|---|---|---|---|
| Display capture | ✅ | ✅ | ✅ | ✅ | ✅ |
| Window capture | ❌ (planned) | ⚠️ best effort | ✅ | ❌ (API limit) | ✅ |
| Delivery model | streaming | polling | streaming | on-change | streaming |
| System audio | ✅ PipeWire | ✅ PipeWire | ✅ SCKit | ✅ WASAPI | ✅ WASAPI |
| Cursor embed/hide | ✅ (portal mode) | ✅ (XFixes blend) | ✅ | ❌ cursor never drawn | ✅ |
| Crop rect | ❌ | ✅ | ⚠️ known issues | ✅ | ✅ |
| Frame-rate control | negotiated | ✅ paced | ✅ | n/a (on-change) | refresh rate |
| Timestamp epoch | process-relative | process-relative | boot | boot | boot |

Microphone capture and zero-copy GPU frames are not implemented on any platform yet (`Unsupported` / always host memory).

## Linux

**Build requirements:** `libpipewire-0.3-dev` (Debian/Ubuntu) or `pipewire` headers (Arch/Fedora), plus `clang` for bindgen. X11 support is pure Rust (x11rb), no extra libraries.

**Runtime:** PipeWire daemon (any modern distro). Wayland video additionally needs XDG Desktop Portal.

- **Wayland (default in Wayland sessions):** the compositor shows a permission dialog on session build; the user's choice there decides what's captured. Pass `.restore_token(...)` from a previous session to skip the dialog. Window capture through the portal is not wired up yet.
- **X11 (default in Xorg sessions, or forced via `BackendPreference::LinuxX11`):** a polling `GetImage` loop paced to `frame_rate` (default 30) - X11 has no damage-driven streaming API, so treat this as a capable screenshot loop, not a low-latency recorder. Cursor is alpha-blended via XFixes. Window capture works while the window is visible. Note: inside a Wayland session, Xwayland's root window is not readable - X11 display capture only works on real Xorg.
- **Audio:** captures the default sink's monitor (system mix) natively via PipeWire. Works in any session type, no dialogs, pairs with either video backend or runs standalone.
- **Timestamps** are process-relative monotonic (PipeWire buffer metadata isn't exposed by current bindings) - consistent within a session, not boot-anchored.

## macOS

**Requirements:** macOS 12.3+ (ScreenCaptureKit). No build-time extras.

**Permission:** Screen Recording under System Settings → Privacy & Security. The first `enumerate_sources()` or session build triggers the prompt; a fresh grant requires relaunching the app (macOS behavior, not pinray's).

- Display and window capture, cursor toggle, system audio - all through one `SCStream` (audio arrives via `next_event`, `BackendBundle`-level audio is unified with video).
- Frames are BGRA host copies; requesting `Rgba8888` swizzles on the CPU (SCKit streams don't do RGBA natively).
- **Known issues** (tracked in `docs2/macos.md`, need real-hardware verification): window capture output is currently display-sized (letterboxed), crop rect has a points-vs-pixels mismatch on retina, audio channel layout may be planar rather than interleaved, HiDPI scale is assumed 2×.

## Windows

**Requirements:** Windows 10+; WGC needs 10 1903+. No build-time extras.

- **DXGI Desktop Duplication (default for displays):** frames only when the desktop *changes* - an idle desktop yields `Timeout`, which is expected. The cursor is never drawn into DXGI frames (arrives as metadata pinray doesn't composite yet); use WGC when you need the cursor. On duplication denial (secure desktop, session policy) `Auto` falls back to WGC at build time.
- **WGC (default for windows, fallback for displays):** streams at display refresh, cursor toggle supported, the yellow capture border is suppressed where the OS allows.
- **Audio:** WASAPI shared-mode loopback of the default render endpoint; format follows the device mix (typically 48 kHz stereo F32). Pairs with either backend or runs standalone.
- Timestamps are QPC-based (boot-relative) across video and audio.

## Consuming frames across platforms

Write against the metadata, not platform assumptions:

- Respect `stride` - rows may be padded on some paths even though current backends deliver packed rows.
- Respect `pixel_format` - default BGRA; don't assume RGBA.
- Handle `Timeout` as normal flow (DXGI idle desktops, X11 pacing), not as an error.
- Handle `Gap` events if you mux audio/video - they're your drop signal.
- Drain audio with `next_audio()` if your video consumer is slow; bounded queues drop oldest audio otherwise.
