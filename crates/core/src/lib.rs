//! Platform-independent types and traits behind [pinray].
//!
//! This crate defines the data model every pinray backend speaks: session
//! configuration ([`SessionConfig`], [`SessionBuilder`]), captured data
//! ([`VideoFrame`], [`AudioFrame`], [`CaptureEvent`]), source enumeration
//! results ([`CaptureSource`]), errors ([`PinrayError`]), and the
//! [`VideoBackend`] / [`AudioBackend`] traits that platform crates implement.
//!
//! **Most applications should depend on [pinray] instead**, which re-exports
//! these types and wires in the right platform backend automatically. Depend
//! on `pinray-core` directly when you are:
//!
//! - writing your own capture backend (implement [`VideoBackend`] /
//!   [`AudioBackend`] and a [`BackendResolver`]),
//! - processing pinray frames in a crate that should not pull in native
//!   platform dependencies, or
//! - testing frame-handling code against mock backends.
//!
//! # Examples
//!
//! Drive a [`CaptureSession`] from a custom backend:
//!
//! ```
//! use std::time::Duration;
//! use pinray_core::{
//!     AudioCapture, AudioData, AudioFrame, AudioBackend, BackendBundle, BackendInfo,
//!     BackendKind, BackendResolver, CaptureEvent, Result, SampleFormat, SessionBuilder,
//!     SessionConfig,
//! };
//!
//! /// An audio backend that produces silence forever.
//! struct Silence(u64);
//!
//! impl AudioBackend for Silence {
//!     fn info(&self) -> BackendInfo {
//!         BackendInfo {
//!             kind: BackendKind::LinuxPipeWireAudio,
//!             supports_audio: true,
//!             zero_copy: false,
//!             notes: "silence generator",
//!         }
//!     }
//!     fn start(&mut self) -> Result<()> { Ok(()) }
//!     fn stop(&mut self) -> Result<()> { Ok(()) }
//!     fn next_audio(&mut self, _timeout: Option<Duration>) -> Result<AudioFrame> {
//!         self.0 += 1;
//!         Ok(AudioFrame {
//!             stream_time_ns: self.0 as i64 * 10_000_000,
//!             sequence: self.0,
//!             sample_rate: 48_000,
//!             channels: 2,
//!             sample_format: SampleFormat::F32,
//!             data: AudioData::Interleaved(vec![0; 480 * 2 * 4]),
//!         })
//!     }
//! }
//!
//! struct SilenceResolver;
//!
//! impl BackendResolver for SilenceResolver {
//!     fn resolve(&self, _config: &SessionConfig) -> Result<BackendBundle> {
//!         let audio = Silence(0);
//!         Ok(BackendBundle { info: audio.info(), video: None, audio: Some(Box::new(audio)) })
//!     }
//! }
//!
//! # fn main() -> Result<()> {
//! let mut session = SessionBuilder::new()
//!     .audio(AudioCapture::SystemMix)
//!     .build_with_resolver(&SilenceResolver)?;
//!
//! session.start()?;
//! let CaptureEvent::Audio(frame) = session.next_event(Some(Duration::from_secs(1)))? else {
//!     unreachable!("audio-only session yields audio events");
//! };
//! assert_eq!(frame.sample_rate, 48_000);
//! session.stop()?;
//! # Ok(())
//! # }
//! ```
//!
//! [pinray]: https://docs.rs/pinray
#![warn(missing_docs)]

pub mod audio;
pub mod backend;
pub mod config;
pub mod error;
pub mod frame;
pub mod session;
pub mod source;

pub use audio::{AudioData, AudioFrame, SampleFormat};
pub use backend::{
    AudioBackend, BackendBundle, BackendInfo, BackendKind, BackendPreference, BackendResolver,
    VideoBackend,
};
pub use config::{AudioCapture, CursorMode, SessionConfig, VideoCaptureTarget};
pub use error::{PinrayError, Result};
pub use frame::{
    CaptureEvent, ColorSpace, CvPixelBufferHandle, D3D11TextureHandle, DmabufFrame, FrameData,
    GapEvent, GapReason, PixelFormat, Rect, VideoFrame,
};
pub use session::{CaptureSession, SessionBuilder};
pub use source::{AudioDeviceSource, CaptureSource, DisplaySource, SourceId, WindowSource};
