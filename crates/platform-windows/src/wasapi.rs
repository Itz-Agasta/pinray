//! WASAPI loopback system-audio backend.
//!
//! A dedicated capture thread opens the default render endpoint in shared
//! loopback mode and drains packets in a polling loop (event-driven buffering
//! is unreliable for loopback pins across Windows versions). Frames are
//! pushed into a bounded channel; `next_audio` drains it. If the consumer
//! falls behind, the oldest unread packets are dropped.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, sync_channel};
use std::thread::JoinHandle;
use std::time::Duration;

use pinray_core::{
    AudioBackend, AudioData, AudioFrame, BackendInfo, BackendKind, PinrayError, Result,
    SampleFormat,
};
use tracing::{debug, warn};
use windows::Win32::Media::Audio::{
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK,
    IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator, MMDeviceEnumerator, WAVE_FORMAT_PCM,
    WAVEFORMATEX, WAVEFORMATEXTENSIBLE, eConsole, eRender,
};
use windows::Win32::Media::KernelStreaming::{KSDATAFORMAT_SUBTYPE_PCM, WAVE_FORMAT_EXTENSIBLE};
use windows::Win32::Media::Multimedia::{KSDATAFORMAT_SUBTYPE_IEEE_FLOAT, WAVE_FORMAT_IEEE_FLOAT};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
    CoUninitialize,
};

use crate::d3d::win_err;

/// How many audio packets (~10 ms each) the channel buffers before dropping.
const AUDIO_CHANNEL_CAPACITY: usize = 64;
const POLL_INTERVAL: Duration = Duration::from_millis(4);

struct MixFormat {
    sample_rate: u32,
    channels: u16,
    block_align: u16,
    sample_format: SampleFormat,
}

fn parse_mix_format(format: &WAVEFORMATEX) -> Result<MixFormat> {
    let tag = format.wFormatTag as u32;
    let bits = format.wBitsPerSample;

    let int_format = |bits: u16| match bits {
        16 => Ok(SampleFormat::I16),
        32 => Ok(SampleFormat::I32),
        other => Err(PinrayError::Unsupported(format!(
            "unsupported PCM bit depth from WASAPI mix format: {other}"
        ))),
    };

    let sample_format = if tag == WAVE_FORMAT_IEEE_FLOAT {
        SampleFormat::F32
    } else if tag == WAVE_FORMAT_PCM {
        int_format(bits)?
    } else if tag == WAVE_FORMAT_EXTENSIBLE {
        let ext = unsafe { &*(format as *const WAVEFORMATEX as *const WAVEFORMATEXTENSIBLE) };
        let sub_format = ext.SubFormat;
        if sub_format == KSDATAFORMAT_SUBTYPE_IEEE_FLOAT {
            SampleFormat::F32
        } else if sub_format == KSDATAFORMAT_SUBTYPE_PCM {
            int_format(bits)?
        } else {
            return Err(PinrayError::Unsupported(format!(
                "unsupported WASAPI mix sub-format: {sub_format:?}"
            )));
        }
    } else {
        return Err(PinrayError::Unsupported(format!(
            "unsupported WASAPI mix format tag: {tag}"
        )));
    };

    Ok(MixFormat {
        sample_rate: format.nSamplesPerSec,
        channels: format.nChannels,
        block_align: format.nBlockAlign,
        sample_format,
    })
}

fn capture_loop(stop: &AtomicBool, tx: &SyncSender<AudioFrame>, init_tx: &SyncSender<Result<()>>) {
    let mut initialized = false;
    let result = (|| -> Result<()> {
        let enumerator: IMMDeviceEnumerator =
            unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
                .map_err(|e| win_err("CoCreateInstance(MMDeviceEnumerator)", e))?;
        let device = unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole) }
            .map_err(|e| win_err("GetDefaultAudioEndpoint", e))?;
        let client: IAudioClient = unsafe { device.Activate(CLSCTX_ALL, None) }
            .map_err(|e| win_err("IMMDevice::Activate(IAudioClient)", e))?;

        let format_ptr =
            unsafe { client.GetMixFormat() }.map_err(|e| win_err("GetMixFormat", e))?;
        let mix = {
            let parsed = parse_mix_format(unsafe { &*format_ptr });
            let init = unsafe {
                client.Initialize(
                    AUDCLNT_SHAREMODE_SHARED,
                    AUDCLNT_STREAMFLAGS_LOOPBACK,
                    // 200 ms buffer in 100-ns units; generous to survive
                    // consumer stalls between polls.
                    2_000_000,
                    0,
                    format_ptr,
                    None,
                )
            };
            unsafe { CoTaskMemFree(Some(format_ptr.cast())) };
            init.map_err(|e| win_err("IAudioClient::Initialize", e))?;
            parsed?
        };

        let capture: IAudioCaptureClient = unsafe { client.GetService() }
            .map_err(|e| win_err("GetService(IAudioCaptureClient)", e))?;
        unsafe { client.Start() }.map_err(|e| win_err("IAudioClient::Start", e))?;

        initialized = true;
        let _ = init_tx.try_send(Ok(()));
        debug!(
            rate = mix.sample_rate,
            channels = mix.channels,
            format = ?mix.sample_format,
            "wasapi loopback capture started"
        );

        let mut sequence = 0u64;
        while !stop.load(Ordering::Relaxed) {
            std::thread::sleep(POLL_INTERVAL);

            loop {
                let packet_frames = unsafe { capture.GetNextPacketSize() }
                    .map_err(|e| win_err("GetNextPacketSize", e))?;
                if packet_frames == 0 {
                    break;
                }

                let mut data_ptr: *mut u8 = std::ptr::null_mut();
                let mut frames_read = 0u32;
                let mut flags = 0u32;
                let mut qpc_position = 0u64;
                unsafe {
                    capture.GetBuffer(
                        &mut data_ptr,
                        &mut frames_read,
                        &mut flags,
                        None,
                        Some(&mut qpc_position),
                    )
                }
                .map_err(|e| win_err("IAudioCaptureClient::GetBuffer", e))?;

                let byte_count = frames_read as usize * mix.block_align as usize;
                let bytes = if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 {
                    vec![0u8; byte_count]
                } else {
                    unsafe { std::slice::from_raw_parts(data_ptr, byte_count) }.to_vec()
                };
                unsafe { capture.ReleaseBuffer(frames_read) }
                    .map_err(|e| win_err("ReleaseBuffer", e))?;

                let frame = AudioFrame {
                    // GetBuffer's QPC position is already in 100-ns units.
                    stream_time_ns: qpc_position as i64 * 100,
                    sequence,
                    sample_rate: mix.sample_rate,
                    channels: mix.channels,
                    sample_format: mix.sample_format,
                    data: AudioData::Interleaved(bytes),
                };
                sequence += 1;

                // If the consumer is behind, drop the packet.
                let _ = tx.try_send(frame);
            }
        }

        unsafe { client.Stop() }.map_err(|e| win_err("IAudioClient::Stop", e))?;
        Ok(())
    })();

    if let Err(error) = result {
        if initialized {
            warn!("wasapi capture loop terminated: {error}");
        } else {
            let _ = init_tx.try_send(Err(error));
        }
    }
}

pub(crate) struct WasapiAudioBackend {
    worker: Option<(JoinHandle<()>, Arc<AtomicBool>)>,
    rx: Option<Receiver<AudioFrame>>,
}

impl WasapiAudioBackend {
    pub(crate) fn new() -> Self {
        Self {
            worker: None,
            rx: None,
        }
    }

    pub(crate) fn backend_info() -> BackendInfo {
        BackendInfo {
            kind: BackendKind::WindowsWasapi,
            supports_audio: true,
            zero_copy: false,
            notes: "WASAPI shared-mode loopback of the default render endpoint (system mix)",
        }
    }
}

impl AudioBackend for WasapiAudioBackend {
    fn info(&self) -> BackendInfo {
        Self::backend_info()
    }

    fn start(&mut self) -> Result<()> {
        if self.worker.is_some() {
            return Ok(());
        }

        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = sync_channel::<AudioFrame>(AUDIO_CHANNEL_CAPACITY);
        let (init_tx, init_rx) = sync_channel::<Result<()>>(1);

        let thread_stop = Arc::clone(&stop);
        let handle = std::thread::Builder::new()
            .name("pinray-wasapi".into())
            .spawn(move || {
                let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
                capture_loop(&thread_stop, &tx, &init_tx);
                if com.is_ok() {
                    unsafe { CoUninitialize() };
                }
            })
            .map_err(|e| PinrayError::Platform(format!("failed to spawn audio thread: {e}")))?;

        match init_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => {
                self.worker = Some((handle, stop));
                self.rx = Some(rx);
                Ok(())
            }
            Ok(Err(error)) => {
                let _ = handle.join();
                Err(error)
            }
            Err(_) => {
                stop.store(true, Ordering::Relaxed);
                let _ = handle.join();
                Err(PinrayError::Platform(
                    "wasapi capture thread did not initialize within 5s".into(),
                ))
            }
        }
    }

    fn stop(&mut self) -> Result<()> {
        if let Some((handle, stop)) = self.worker.take() {
            stop.store(true, Ordering::Relaxed);
            let _ = handle.join();
        }
        self.rx = None;
        Ok(())
    }

    fn next_audio(&mut self, timeout: Option<Duration>) -> Result<AudioFrame> {
        let rx = self
            .rx
            .as_ref()
            .ok_or_else(|| PinrayError::Platform("wasapi backend not started".into()))?;

        match timeout {
            Some(timeout) => rx.recv_timeout(timeout).map_err(|error| match error {
                RecvTimeoutError::Timeout => PinrayError::Timeout(timeout),
                RecvTimeoutError::Disconnected => {
                    PinrayError::Platform("wasapi capture thread terminated".into())
                }
            }),
            None => rx
                .recv()
                .map_err(|_| PinrayError::Platform("wasapi capture thread terminated".into())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_format(tag: u32, bits: u16) -> WAVEFORMATEX {
        WAVEFORMATEX {
            wFormatTag: tag as u16,
            nChannels: 2,
            nSamplesPerSec: 48_000,
            nAvgBytesPerSec: 48_000 * 2 * (bits as u32 / 8),
            nBlockAlign: 2 * (bits / 8),
            wBitsPerSample: bits,
            cbSize: 0,
        }
    }

    #[test]
    fn parses_ieee_float_mix_format() {
        let mix = parse_mix_format(&base_format(WAVE_FORMAT_IEEE_FLOAT, 32)).unwrap();
        assert_eq!(mix.sample_format, SampleFormat::F32);
        assert_eq!(mix.sample_rate, 48_000);
        assert_eq!(mix.channels, 2);
        assert_eq!(mix.block_align, 8);
    }

    #[test]
    fn parses_pcm_16bit() {
        let mix = parse_mix_format(&base_format(WAVE_FORMAT_PCM, 16)).unwrap();
        assert_eq!(mix.sample_format, SampleFormat::I16);
    }

    #[test]
    fn rejects_odd_pcm_depth() {
        assert!(parse_mix_format(&base_format(WAVE_FORMAT_PCM, 24)).is_err());
    }

    #[test]
    fn parses_extensible_float() {
        let mut base = base_format(WAVE_FORMAT_EXTENSIBLE, 32);
        base.cbSize = 22;
        let ext = WAVEFORMATEXTENSIBLE {
            Format: base,
            SubFormat: KSDATAFORMAT_SUBTYPE_IEEE_FLOAT,
            ..Default::default()
        };
        // The struct is packed(1), so go through a raw pointer instead of a
        // field reference — this mirrors how the real GetMixFormat buffer is
        // read.
        let fmt = &ext as *const WAVEFORMATEXTENSIBLE as *const WAVEFORMATEX;
        let mix = parse_mix_format(unsafe { &*fmt }).unwrap();
        assert_eq!(mix.sample_format, SampleFormat::F32);
    }
}
