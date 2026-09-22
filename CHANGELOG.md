# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project
follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.5] - 2026-09-22

`frame_rate` is now honored on Windows. Both backends previously ignored it, so
a session asking for 30 fps was handed frames at whatever rate the display or
desktop produced.

### Fixed

- **WGC honors `frame_rate`** ([#8], reported in [#7]). The free-threaded frame
  pool delivered a frame on every DWM composition, and frames overflowing the
  bounded channel were discarded only after `texture_to_host` had already run,
  so the GPU to CPU staging copy was paid for frames no consumer ever saw.
  Pacing now happens in the arrival callback before the copy. Measured at
  1920x1080 @ 60 Hz with an encoder consuming at 30 fps, wasted copies went from
  309 to 8 over ten seconds and CPU from 29% of a core to 15%.
- **DXGI honors `frame_rate`** ([#10], tracked in [#9]). Duplication is pull
  model, so it never wasted work, but a caller who set `.frame_rate(Some(30))`
  and looped on `next_event` got frames as fast as the desktop changed. On a
  144 Hz panel that is 144 fps of full frame copies into the encoder. The
  acquire is now held off until the frame is due, so a skipped frame is never
  copied. Unthrottled to 30 fps at 1080p, that is 59.9 fps and 32% of a core
  down to 29.6 fps and 11%.
- `IGraphicsCaptureSession5::MinUpdateInterval` is set on WGC where the OS
  supports it, so the compositor throttles at the source. This is Windows 11
  only; Windows 10 returns `E_NOINTERFACE` and the software pacing carries the
  rate limit there.

### Changed

- **On a display faster than 60 Hz, the default `frame_rate: Some(60)` now caps
  delivery at 60 fps** where earlier versions ran at full refresh. This is what
  the setting has always meant on the Linux and macOS backends. Pass
  `.frame_rate(None)` to keep the old behavior and take every frame the display
  or desktop produces.
- Internal crate dependencies are pinned to the exact workspace version rather
  than the `0.2` range, so `pinray` can no longer resolve against an older
  `pinray-core` that lacks APIs it calls.
- `docs/platforms.md` records what each Windows backend does with `frame_rate`,
  and the feature matrix is corrected.
- Adopted `as_chunks_mut` across the platform crates for
  `clippy::chunks_exact_to_as_chunks`, new in Rust 1.98.

### Notes

Pacing is a ceiling, not a floor. A desktop changing more slowly than the
requested rate still delivers at its own rate.

Two details worth knowing if exact spacing matters:

- WGC can only select whole vblanks, so where the refresh rate is not an integer
  multiple of the request (60 fps on a 90 Hz panel) the gap between delivered
  frames alternates while the average holds.
- Frames skipped by pacing are not reported as `Gap` or `Dropped`, because the
  caller asked for that rate.

Verified on Windows 10 19045, 1920x1080 @ 60 Hz, Intel UHD Graphics. Both
backends deliver within 1% of the requested rate at 60, 30, 15 and 5 fps, with
CPU and staging bandwidth scaling linearly. The `MinUpdateInterval` success path
is compile verified only, since it needs a Windows 11 build exposing
`IGraphicsCaptureSession5`.

## [0.2.4] - 2026-09-19

### Added

- Wayland portal restore tokens are surfaced to callers through
  `CaptureSession::restore_token()`, so a later session can skip the permission
  dialog.

## [0.2.2] - 2026-09-18

### Added

- `VideoFrame::to_tight_bytes()` for consumers that cannot handle stride
  padding.
- `llms.txt` agent oriented crate reference, and the getting started guide is
  embedded into the crate level docs so it renders on docs.rs.

## [0.2.0] - 2026-09-17

### Added

- X11 backend, plus hardening and publish ready documentation.

## [0.1.1]

Initial published release.

[Unreleased]: https://github.com/Itz-Agasta/pinray/compare/v0.2.5...HEAD
[0.2.5]: https://github.com/Itz-Agasta/pinray/compare/v0.2.4...v0.2.5
[0.2.4]: https://github.com/Itz-Agasta/pinray/compare/v0.2.2...v0.2.4
[0.2.2]: https://github.com/Itz-Agasta/pinray/compare/v0.2.0...v0.2.2
[0.2.0]: https://github.com/Itz-Agasta/pinray/releases/tag/v0.2.0
[#7]: https://github.com/Itz-Agasta/pinray/issues/7
[#8]: https://github.com/Itz-Agasta/pinray/pull/8
[#9]: https://github.com/Itz-Agasta/pinray/issues/9
[#10]: https://github.com/Itz-Agasta/pinray/pull/10
