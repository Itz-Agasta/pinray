//! PipeWire system-audio capture (sink monitor).
//!
//! Connects a native PipeWire capture stream with `stream.capture.sink=true`,
//! which makes the session manager route the default output's monitor ports
//! to us — i.e. the system mix. No portal round-trip is needed for audio and
//! nothing shells out to `pactl`.
//!
//! Mirrors the worker-thread pattern of the Wayland video backend: the
//! PipeWire main loop runs on a dedicated thread, frames flow through a
//! bounded queue into an mpsc channel, and start/stop/terminate arrive over
//! a control channel.

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
            audio::{AudioFormat, AudioInfoRaw},
            format::{MediaSubtype, MediaType},
        },
        pod::Pod,
        utils::{Direction, SpaTypes},
    },
    stream::{StreamFlags, StreamListener, StreamRc},
};

use pinray_core::{
    AudioBackend, AudioData, AudioFrame, BackendInfo, BackendKind, PinrayError, Result,
    SampleFormat,
};

/// Cap on frames buffered while the consumer is not draining; oldest packets
/// are dropped first (~10 ms of audio each).
const MAX_QUEUED_FRAMES: usize = 512;

pub(crate) struct PipeWireAudioBackend {
    control_tx: mpsc::Sender<ControlMessage>,
    event_rx: mpsc::Receiver<AudioFrame>,
    worker: Option<thread::JoinHandle<Result<()>>>,
}

impl PipeWireAudioBackend {
    pub(crate) fn new() -> Result<Self> {
        let (control_tx, control_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();

        let worker = thread::Builder::new()
            .name("pinray-pw-audio".into())
            .spawn(move || run_audio_loop(control_rx, event_tx))
            .map_err(|e| PinrayError::Platform(format!("failed to spawn audio thread: {e}")))?;

        Ok(Self {
            control_tx,
            event_rx,
            worker: Some(worker),
        })
    }

    pub(crate) fn backend_info() -> BackendInfo {
        BackendInfo {
            kind: BackendKind::LinuxPipeWireAudio,
            supports_audio: true,
            zero_copy: false,
            notes: "PipeWire system-mix capture via default sink monitor",
        }
    }
}

impl AudioBackend for PipeWireAudioBackend {
    fn info(&self) -> BackendInfo {
        Self::backend_info()
    }

    fn start(&mut self) -> Result<()> {
        self.control_tx
            .send(ControlMessage::Start)
            .map_err(|_| PinrayError::Platform("pipewire audio worker is not available".into()))
    }

    fn stop(&mut self) -> Result<()> {
        self.control_tx
            .send(ControlMessage::Stop)
            .map_err(|_| PinrayError::Platform("pipewire audio worker is not available".into()))
    }

    fn next_audio(&mut self, timeout: Option<Duration>) -> Result<AudioFrame> {
        match timeout {
            Some(timeout) => self
                .event_rx
                .recv_timeout(timeout)
                .map_err(|error| match error {
                    mpsc::RecvTimeoutError::Timeout => PinrayError::Timeout(timeout),
                    mpsc::RecvTimeoutError::Disconnected => {
                        PinrayError::Platform("pipewire audio channel disconnected".into())
                    }
                }),
            None => self
                .event_rx
                .recv()
                .map_err(|_| PinrayError::Platform("pipewire audio channel disconnected".into())),
        }
    }
}

impl Drop for PipeWireAudioBackend {
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
    format: AudioInfoRaw,
}

#[derive(Default)]
struct RuntimeState {
    active: bool,
    sequence: u64,
}

fn run_audio_loop(
    control_rx: mpsc::Receiver<ControlMessage>,
    event_tx: mpsc::Sender<AudioFrame>,
) -> Result<()> {
    pw::init();

    let main_loop = MainLoopRc::new(None).map_err(platform_error)?;
    let context = ContextRc::new(&main_loop, None).map_err(platform_error)?;
    let core = context.connect_rc(None).map_err(platform_error)?;

    let runtime = Arc::new(Mutex::new(RuntimeState::default()));
    let queue = Arc::new(Mutex::new(VecDeque::<AudioFrame>::new()));

    let stream_properties = properties! {
        *pw::keys::MEDIA_TYPE => "Audio",
        *pw::keys::MEDIA_CATEGORY => "Capture",
        *pw::keys::MEDIA_ROLE => "Music",
        // Capture from the default sink's monitor ports = system mix.
        *pw::keys::STREAM_CAPTURE_SINK => "true",
    };
    let stream =
        StreamRc::new(core, "pinray-system-audio", stream_properties).map_err(platform_error)?;

    let listener = stream
        .add_local_listener_with_user_data(UserData::default())
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
            if media_type != MediaType::Audio || media_subtype != MediaSubtype::Raw {
                return;
            }

            if let Err(error) = user_data.format.parse(param) {
                tracing::warn!(error = %error, "pipewire audio format parse failed");
                return;
            }
            tracing::debug!(
                rate = user_data.format.rate(),
                channels = user_data.format.channels(),
                "pipewire audio format negotiated"
            );
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
                let chunk = data.chunk();
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

                let rate = user_data.format.rate();
                let channels = user_data.format.channels();
                if rate == 0 || channels == 0 {
                    return;
                }

                let frame = AudioFrame {
                    // TODO: plumb PipeWire timing metadata, same as the video path.
                    stream_time_ns: 0,
                    sequence: state.sequence,
                    sample_rate: rate,
                    channels: channels as u16,
                    sample_format: SampleFormat::F32,
                    data: AudioData::Interleaved(raw[offset..offset + chunk_size].to_vec()),
                };
                state.sequence += 1;

                if let Ok(mut queue) = queue.lock() {
                    if queue.len() >= MAX_QUEUED_FRAMES {
                        queue.pop_front();
                    }
                    queue.push_back(frame);
                }
            }
        })
        .register()
        .map_err(platform_error)?;
    let _listener: StreamListener<UserData> = listener;

    // Offer F32 and let PipeWire pick the graph's native rate and channel
    // count (parsed back in param_changed).
    let mut audio_info = AudioInfoRaw::new();
    audio_info.set_format(AudioFormat::F32LE);
    let format_obj = pw::spa::pod::Object {
        type_: SpaTypes::ObjectParamFormat.as_raw(),
        id: ParamType::EnumFormat.as_raw(),
        properties: audio_info.into(),
    };
    let format_bytes = pw::spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &pw::spa::pod::Value::Object(format_obj),
    )
    .map_err(platform_error)?
    .0
    .into_inner();

    let mut params = [Pod::from_bytes(&format_bytes)
        .ok_or_else(|| PinrayError::Platform("failed to build audio format pod".into()))?];
    stream
        .connect(
            Direction::Input,
            None,
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
                ControlMessage::Terminate => terminate = true,
            }
        }

        if let Ok(mut queue) = queue.lock() {
            while let Some(frame) = queue.pop_front() {
                if event_tx.send(frame).is_err() {
                    return Ok(());
                }
            }
        }

        pw_loop.iterate(pw::loop_::Timeout::Finite(Duration::from_millis(10)));
    }

    Ok(())
}

fn platform_error(error: impl std::fmt::Display) -> PinrayError {
    PinrayError::Platform(error.to_string())
}
