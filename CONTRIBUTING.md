# Contributing to pinray

Thanks for helping. This page covers how to set up the workspace, run the checks CI runs, and test capture on each platform. It applies to people and coding agents alike.

## Before you start

- **Bugs**: search the [open issues](https://github.com/Itz-Agasta/pinray/issues) first. If yours is new, use the bug report form; it asks for the details needed to reproduce a backend problem
- **Features or larger changes**: open an issue first and wait for a reply before writing code. Some issues need a maintainer decision before work starts, and the issue says so
- **Small fixes** (typos, broken links, doc corrections): open a pull request directly
- **Good first issues**: look for the [`good first issue`](https://github.com/Itz-Agasta/pinray/labels/good%20first%20issue) label

## Find your way around

pinray is a Cargo workspace with five crates:

- **`crates/core`** (`pinray-core`): the frame model, session builder, errors, and the `VideoBackend`/`AudioBackend` traits. It has no platform code
- **`crates/platform-linux`**: Wayland (portal + PipeWire), X11, and PipeWire audio
- **`crates/platform-macos`**: ScreenCaptureKit
- **`crates/platform-windows`**: DXGI, Windows Graphics Capture, and WASAPI
- **`crates/pinray`**: the public facade that re-exports `pinray-core` and picks the platform backend. The runnable examples live in `crates/pinray/examples`

Each platform crate's `src/lib.rs` explains its backend selection rules. Start reading there, then [docs/platforms.md](docs/platforms.md) for each backend's behavior and limits.

## Set up

Install Rust 1.88 or later with [rustup](https://rustup.rs). On Linux you also need the PipeWire headers and `clang`:

```console
sudo apt install libpipewire-0.3-dev clang   # Debian/Ubuntu
sudo pacman -S pipewire clang                # Arch
sudo dnf install pipewire-devel clang        # Fedora
```

macOS needs nothing beyond Rust. On Windows, the default MSVC toolchain also needs the Visual Studio C++ Build Tools and the Windows SDK; see [rustup's Windows prerequisites](https://rust-lang.github.io/rustup/installation/windows-msvc.html).

## Run the checks

CI runs these on every pull request, with warnings treated as errors. Run them before you push:

```console
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

`cargo test` also runs the doc examples, including the ones in the README and the getting-started guide.

CI also checks the macOS and Windows crates from Linux. You can run the same checks if you have the targets installed (`rustup target add x86_64-pc-windows-msvc aarch64-apple-darwin`):

```console
cargo check -p pinray-platform-windows --all-targets --target x86_64-pc-windows-msvc
cargo check -p pinray --examples --target x86_64-pc-windows-msvc
cargo check -p pinray-platform-macos --target aarch64-apple-darwin
cargo check -p pinray --examples --target aarch64-apple-darwin
```

If you change doc comments, build the docs with warnings as errors:

```console
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
```

## Test capture on a real machine

CI can't grant screen-recording permission or open a desktop session, so it only runs a real capture test for X11 (under Xvfb). If you change a backend, run its example on real hardware and paste the output into your pull request:

```console
cargo run --example wayland_smoke    # Linux Wayland: video + audio
cargo run --example x11_smoke        # Linux X11: polling video + audio
cargo run --example audio_smoke      # Linux/Windows: audio only
cargo run --example macos_smoke      # macOS
cargo run --example windows_smoke    # Windows (PINRAY_BACKEND=wgc|dxgi to force)
```

Add `RUST_LOG=debug` to see backend logs. If you can't test on the platform you changed, say so in the pull request, and someone with the hardware can check it.

## Write the change

- **Public API**: every public item needs a doc comment (`missing_docs` warns). Follow the existing style: a one-line summary, then details, an `# Errors` section on anything that returns `Result`, and an example where it helps. Examples use `?`, not `unwrap()`
- **Accuracy**: documentation must match what the code does on every platform. If a backend behaves differently, say which one. Coding agents copy these docs as written
- **Platform docs**: when you change backend behavior, update [docs/platforms.md](docs/platforms.md), [llms.txt](llms.txt), and any affected doc comments in the same pull request
- **Changelog**: add a line under `## [Unreleased]` in [CHANGELOG.md](CHANGELOG.md) for anything a user would notice
- **`unsafe`**: keep it inside the platform crates, and add a comment wherever it isn't obvious why a block is sound

## Open a pull request

Keep the description short: what changed and why, plus the issue it fixes (`Fixes #123`). Mention anything that affects the public API, the minimum Rust version, or platform support. Wait for CI to pass before asking for review.
