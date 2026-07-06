use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, mpsc},
    thread,
    time::Duration,
};

use pipewire::{
    self as pw,
    context::ContextRc,
    main_loop::MainLoopRc,
    properties::properties,
    spa::{
        param::{
            ParamType,
            format::{FormatProperties, MediaSubtype, MediaType},
            video::{VideoFormat, VideoInfoRaw},
        },
        pod::{Pod, Property, Value},
        utils::{Direction, Id, SpaTypes},
    },
    stream::{StreamFlags, StreamListener, StreamRc, StreamState},
};

use pinray_core::{
    AudioBackend, AudioCapture, BackendBundle, BackendInfo, BackendKind, CaptureEvent, ColorSpace,
    CursorMode, FrameData::Host, PinrayError, PixelFormat, Result, SessionConfig, VideoBackend,
    VideoFrame,
};

use crate::{audio::PipeWireAudioBackend, portal::PortalClient};

/// Returns `true` if the current session is a Wayland session.
///
/// Checks `XDG_SESSION_TYPE` and `WAYLAND_DISPLAY` environment variables.
pub fn is_wayland_session() -> bool {
    std::env::var_os("XDG_SESSION_TYPE")
        .and_then(|value| value.into_string().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("wayland"))
        || std::env::var_os("WAYLAND_DISPLAY").is_some()
}

/// Resolves the Wayland video and/or PipeWire audio backends for the given
/// session configuration.
///
/// System audio is captured natively from the default sink monitor and does
/// not need a portal round-trip, so audio-only sessions skip the portal
/// dialog entirely.
pub fn resolve_wayland_backend(config: &SessionConfig) -> Result<BackendBundle> {
    let audio: Option<Box<dyn AudioBackend>> = match &config.audio_capture {
        None => None,
        Some(AudioCapture::SystemMix) => Some(Box::new(PipeWireAudioBackend::new()?)),
        Some(AudioCapture::Microphone(_)) => {
            return Err(PinrayError::Unsupported(
                "linux microphone capture is not implemented yet".into(),
            ));
        }
    };

    let video: Option<Box<dyn VideoBackend>> = if config.video_target.is_some() {
        Some(Box::new(WaylandVideoBackend::new(config.clone())?))
    } else {
        None
    };

    let info = match (&video, &audio) {
        (Some(video), _) => {
            let mut info = video.info();
            info.supports_audio = audio.is_some();
            info
        }
        (None, Some(audio)) => audio.info(),
        (None, None) => {
            return Err(PinrayError::InvalidConfig(
                "at least one of video_target or audio_capture must be set".into(),
            ));
        }
    };

    Ok(BackendBundle { info, video, audio })
}

struct WaylandVideoBackend {
    info: BackendInfo,
    control_tx: mpsc::Sender<ControlMessage>,
    event_rx: mpsc::Receiver<CaptureEvent>,
    worker: Option<thread::JoinHandle<Result<()>>>,
}

impl WaylandVideoBackend {
    /// Creates a new Wayland capture backend.
    ///
    /// Opens a portal session, obtains the PipeWire fd, and spawns a worker
    /// thread that runs the PipeWire main loop and frame capture.
    fn new(config: SessionConfig) -> Result<Self> {
        let portal = PortalClient::new()?;
        let cast = portal.start_screen_cast(
            matches!(config.cursor_mode, CursorMode::Embedded),
            config.restore_token.as_deref(),
        )?;
        let (fd, streams, restore_token) = cast.into_parts();
        let stream = streams
            .into_iter()
            .next()
            .ok_or_else(|| PinrayError::Platform("portal returned no screencast stream".into()))?;

        if let Some(token) = restore_token {
            tracing::info!(restore_token = %token, "portal returned restore token");
        }

        let (control_tx, control_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let desired_format = config.pixel_format;
        let frame_rate = config.frame_rate.unwrap_or(60);
        let stream_size = stream
            .width
            .zip(stream.height)
            .map(|(width, height)| VideoSize { width, height });

        // CRITICAL: The portal (D-Bus connection) must stay alive while PipeWire
        // uses the fd obtained from it. Dropping the PortalClient closes the D-Bus
        // socket, which invalidates the fd and causes "no more input formats" errors.
        let worker = thread::spawn(move || {
            let _portal_keepalive = portal;
            run_video_loop(
                fd,
                stream.node_id,
                stream_size,
                desired_format,
                frame_rate,
                control_rx,
                event_tx,
            )
        });

        Ok(Self {
            info: BackendInfo {
                kind: BackendKind::LinuxWaylandPortal,
                supports_audio: false,
                zero_copy: false,
                notes: "Wayland video via XDG Desktop Portal + PipeWire",
            },
            control_tx,
            event_rx,
            worker: Some(worker),
        })
    }
}

impl VideoBackend for WaylandVideoBackend {
    fn info(&self) -> BackendInfo {
        self.info.clone()
    }

    fn start(&mut self) -> Result<()> {
        self.control_tx
            .send(ControlMessage::Start)
            .map_err(|_| PinrayError::Platform("wayland capture worker is not available".into()))
    }

    fn stop(&mut self) -> Result<()> {
        self.control_tx
            .send(ControlMessage::Stop)
            .map_err(|_| PinrayError::Platform("wayland capture worker is not available".into()))
    }

    fn next_event(&mut self, timeout: Option<Duration>) -> Result<CaptureEvent> {
        match timeout {
            Some(timeout) => self
                .event_rx
                .recv_timeout(timeout)
                .map_err(|error| match error {
                    mpsc::RecvTimeoutError::Timeout => PinrayError::Timeout(timeout),
                    mpsc::RecvTimeoutError::Disconnected => {
                        PinrayError::Platform("wayland event channel disconnected".into())
                    }
                }),
            None => self
                .event_rx
                .recv()
                .map_err(|_| PinrayError::Platform("wayland event channel disconnected".into())),
        }
    }
}

impl Drop for WaylandVideoBackend {
    fn drop(&mut self) {
        let _ = self.control_tx.send(ControlMessage::Terminate);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[derive(Debug)]
enum ControlMessage {
    Start,
    Stop,
    Terminate,
}

#[derive(Default)]
struct UserData {
    video_format: VideoInfoRaw,
    buffer_params_sent: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct VideoSize {
    width: u32,
    height: u32,
}

#[derive(Default)]
struct RuntimeState {
    active: bool,
    sequence: u64,
}

/// Runs the PipeWire main loop on the worker thread.
///
/// This function:
/// 1. Connects to PipeWire using the portal-provided fd
/// 2. Creates a stream with the negotiated format
/// 3. Enters the main loop, forwarding frames to the event channel
/// 4. Handles start/stop/terminate control messages
///
/// We build a simple EnumFormat param (BGRA/BGRx/RGBA/RGBx with size/framerate
/// ranges). We do NOT try to discover the node's formats via registry enumeration
/// -- that causes "unknown resource" errors because the screencast node lives on
/// the portal's PipeWire remote and the enumeration races with the daemon.
///
/// See `build_stream_params` for the format details.
fn run_video_loop(
    fd: std::os::fd::OwnedFd,
    node_id: u32,
    portal_size: Option<VideoSize>,
    desired_format: PixelFormat,
    frame_rate: u32,
    control_rx: mpsc::Receiver<ControlMessage>,
    event_tx: mpsc::Sender<CaptureEvent>,
) -> Result<()> {
    pw::init();

    let main_loop = MainLoopRc::new(None).map_err(platform_error)?;
    let context = ContextRc::new(&main_loop, None).map_err(platform_error)?;
    let core = context.connect_fd_rc(fd, None).map_err(platform_error)?;

    // Register a core listener so PipeWire processes info/error/done events.
    // Without this, the core may not drive the event loop correctly and stream
    // negotiation can silently fail.
    let _core_listener = core
        .clone()
        .add_listener_local()
        .info(|info| tracing::debug!(?info, "pipewire core info"))
        .error(|id, seq, res, message| {
            tracing::error!(id, seq, res, message, "pipewire core error");
        })
        .done(|id, _seq| {
            tracing::trace!(id, "pipewire core done");
        })
        .register();

    let runtime = Arc::new(Mutex::new(RuntimeState::default()));
    let queue = Arc::new(Mutex::new(VecDeque::<CaptureEvent>::new()));

    let stream_properties = properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
    };
    let stream =
        StreamRc::new(core, "pinray-wayland-video", stream_properties).map_err(platform_error)?;

    let listener = stream
        .add_local_listener_with_user_data(UserData::default())
        .state_changed(|_, _, _, new| match new {
            StreamState::Error(msg) => {
                tracing::error!(error = %msg, "pipewire stream entered error state");
            }
            StreamState::Unconnected => tracing::debug!("pipewire stream: unconnected"),
            StreamState::Connecting => tracing::debug!("pipewire stream: connecting"),
            StreamState::Paused => tracing::debug!("pipewire stream: paused"),
            StreamState::Streaming => tracing::debug!("pipewire stream: streaming"),
        })
        .param_changed(|_, user_data, id, param| {
            let Some(param) = param else {
                return;
            };

            if id != ParamType::Format.as_raw() {
                return;
            }

            let Ok((media_type, media_subtype)) = pw::spa::param::format_utils::parse_format(param)
            else {
                return;
            };

            if media_type != MediaType::Video || media_subtype != MediaSubtype::Raw {
                return;
            }

            if let Err(error) = user_data.video_format.parse(param) {
                tracing::warn!(error = %error, "pipewire stream format parse failed");
                return;
            }

            user_data.buffer_params_sent = true;
        })
        .process({
            let runtime = Arc::clone(&runtime);
            let queue = Arc::clone(&queue);
            move |stream, user_data| {
                let Ok(mut state) = runtime.lock() else {
                    return;
                };

                if !state.active {
                    return;
                }

                let Some(mut buffer) = stream.dequeue_buffer() else {
                    return;
                };

                let datas = buffer.datas_mut();
                if datas.is_empty() {
                    return;
                }

                let data = &mut datas[0];
                let size = user_data.video_format.size();
                let chunk = data.chunk();
                let stride = chunk.stride().max(0) as u32;
                let offset = chunk.offset() as usize;
                let chunk_size = chunk.size() as usize;
                let Some(raw) = data.data() else {
                    return;
                };
                if offset
                    .checked_add(chunk_size)
                    .is_none_or(|end| end > raw.len())
                {
                    return;
                }
                let raw = &raw[offset..offset + chunk_size];
                let (pixel_format, bytes) =
                    normalize_frame(user_data.video_format.format(), raw, desired_format);
                let frame = VideoFrame {
                    // TODO: plumb native PipeWire timing metadata once we add a
                    // lower-level buffer inspection path for 0.9.
                    stream_time_ns: 0,
                    sequence: state.sequence,
                    width: size.width,
                    height: size.height,
                    stride,
                    pixel_format,
                    color_space: Some(ColorSpace::Srgb),
                    data: Host(bytes),
                    damage: None,
                };
                state.sequence += 1;

                if let Ok(mut queue) = queue.lock() {
                    queue.push_back(CaptureEvent::Video(frame));
                }
            }
        })
        .register()
        .map_err(platform_error)?;

    let _listener: StreamListener<UserData> = listener;

    let connect_params = build_stream_params(frame_rate, portal_size)?;

    // Request MetaHeader so we can read presentation timestamps from buffer metadata.
    let metas_obj = pw::spa::pod::object!(
        SpaTypes::ObjectParamMeta,
        ParamType::Meta,
        Property::new(
            pw::spa::sys::SPA_PARAM_META_type,
            Value::Id(Id(pw::spa::sys::SPA_META_Header))
        ),
        Property::new(
            pw::spa::sys::SPA_PARAM_META_size,
            Value::Int(size_of::<pw::spa::sys::spa_meta_header>() as i32)
        ),
    );
    let metas_bytes = pw::spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &Value::Object(metas_obj),
    )
    .map_err(platform_error)?
    .0
    .into_inner();

    let mut params = connect_params
        .iter()
        .filter_map(|bytes| Pod::from_bytes(bytes))
        .chain(Pod::from_bytes(&metas_bytes))
        .collect::<Vec<_>>();
    stream
        .connect(
            Direction::Input,
            Some(node_id),
            StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS | StreamFlags::RT_PROCESS,
            &mut params,
        )
        .map_err(platform_error)?;

    let pw_loop = main_loop.loop_();
    let mut terminate = false;
    while !terminate {
        while let Ok(message) = control_rx.try_recv() {
            match message {
                ControlMessage::Start => {
                    if let Ok(mut state) = runtime.lock() {
                        state.active = true;
                    }
                }
                ControlMessage::Stop => {
                    if let Ok(mut state) = runtime.lock() {
                        state.active = false;
                    }
                }
                ControlMessage::Terminate => {
                    terminate = true;
                }
            }
        }

        if let Ok(mut queue) = queue.lock() {
            while let Some(event) = queue.pop_front() {
                if event_tx.send(event).is_err() {
                    return Ok(());
                }
            }
        }

        pw_loop.iterate(pw::loop_::Timeout::Finite(Duration::from_millis(20)));
    }

    Ok(())
}

/// Build PipeWire stream format parameters for the video capture stream.
///
/// We offer a single `EnumFormat` param with BGRA/BGRx/RGBA/RGBx as a Choice.
/// The compositor's screencast node intersects this with its own advertised
/// formats and picks one. No modifier property -- the compositor handles format
/// conversion internally.
fn build_stream_params(frame_rate: u32, source_size: Option<VideoSize>) -> Result<Vec<Vec<u8>>> {
    let default_size = source_size.unwrap_or(VideoSize {
        width: 1920,
        height: 1080,
    });

    let format = pw::spa::pod::object!(
        SpaTypes::ObjectParamFormat,
        ParamType::EnumFormat,
        pw::spa::pod::property!(FormatProperties::MediaType, Id, MediaType::Video),
        pw::spa::pod::property!(FormatProperties::MediaSubtype, Id, MediaSubtype::Raw),
        pw::spa::pod::property!(
            FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            VideoFormat::BGRA,
            VideoFormat::BGRx,
            VideoFormat::RGBA,
            VideoFormat::RGBx,
        ),
        pw::spa::pod::property!(
            FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            pw::spa::utils::Rectangle {
                width: default_size.width,
                height: default_size.height
            },
            pw::spa::utils::Rectangle {
                width: 1,
                height: 1
            },
            pw::spa::utils::Rectangle {
                width: 7680,
                height: 4320
            }
        ),
        pw::spa::pod::property!(
            FormatProperties::VideoFramerate,
            Choice,
            Range,
            Fraction,
            pw::spa::utils::Fraction {
                num: frame_rate,
                denom: 1
            },
            pw::spa::utils::Fraction { num: 0, denom: 1 },
            pw::spa::utils::Fraction {
                num: frame_rate,
                denom: 1
            }
        ),
    );

    let format_bytes = pw::spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &pw::spa::pod::Value::Object(format),
    )
    .map_err(platform_error)?
    .0
    .into_inner();

    Ok(vec![format_bytes])
}

fn normalize_frame(
    source_format: VideoFormat,
    raw: &[u8],
    desired_format: PixelFormat,
) -> (PixelFormat, Vec<u8>) {
    match (source_format, desired_format) {
        (VideoFormat::BGRA | VideoFormat::BGRx, PixelFormat::Bgra8888) => {
            (PixelFormat::Bgra8888, raw.to_vec())
        }
        (VideoFormat::BGRA, PixelFormat::Rgba8888) => {
            let mut data = raw.to_vec();
            for pixel in data.chunks_exact_mut(4) {
                pixel.swap(0, 2);
            }
            (PixelFormat::Rgba8888, data)
        }
        (VideoFormat::RGBx, PixelFormat::Rgba8888) => (PixelFormat::Rgba8888, raw.to_vec()),
        (VideoFormat::RGBA, PixelFormat::Rgba8888) => (PixelFormat::Rgba8888, raw.to_vec()),
        (VideoFormat::RGB, PixelFormat::Rgb888) => (PixelFormat::Rgb888, raw.to_vec()),
        (VideoFormat::BGRx, PixelFormat::Rgba8888) => {
            let mut data = raw.to_vec();
            for pixel in data.chunks_exact_mut(4) {
                pixel.swap(0, 2);
            }
            (PixelFormat::Rgba8888, data)
        }
        (VideoFormat::RGBx, PixelFormat::Bgra8888) => {
            let mut data = raw.to_vec();
            for pixel in data.chunks_exact_mut(4) {
                pixel.swap(0, 2);
            }
            (PixelFormat::Bgra8888, data)
        }
        (VideoFormat::RGB, _) => (PixelFormat::Rgb888, raw.to_vec()),
        _ => (PixelFormat::Bgra8888, raw.to_vec()),
    }
}

fn platform_error(error: impl std::fmt::Display) -> PinrayError {
    PinrayError::Platform(error.to_string())
}
