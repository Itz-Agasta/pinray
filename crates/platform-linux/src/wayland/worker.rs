//! PipeWire main-loop worker thread for Wayland video capture.
//!
//! Connects to PipeWire over the portal-provided fd, negotiates a raw video
//! stream on the screencast node, and forwards frames through a queue into
//! the backend's event channel. Start/stop/terminate arrive over a control
//! channel and are polled between loop iterations.

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, mpsc},
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
            format::{MediaSubtype, MediaType},
            video::VideoInfoRaw,
        },
        pod::{Pod, Property, Value},
        utils::{Direction, Id, SpaTypes},
    },
    stream::{StreamFlags, StreamListener, StreamRc, StreamState},
};

use pinray_core::{CaptureEvent, ColorSpace, FrameData::Host, PixelFormat, Result, VideoFrame};

use super::format::{build_stream_params, normalize_frame};
use super::{ControlMessage, VideoSize, platform_error};
use crate::clock::monotonic_time_ns;

#[derive(Default)]
struct UserData {
    video_format: VideoInfoRaw,
    buffer_params_sent: bool,
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
pub(super) fn run_video_loop(
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
                    // Dequeue-time monotonic stamp; see crate::clock for why
                    // native PipeWire timing metadata is not used yet.
                    stream_time_ns: monotonic_time_ns(),
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
