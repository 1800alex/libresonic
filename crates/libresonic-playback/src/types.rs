use std::fmt;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use libresonic_core::{Error, MediaId};
use tokio::sync::Notify;

/// Existing Sonicfast EQ/tap processing rate.
pub const PCM_SAMPLE_RATE: u32 = 44_100;
/// Existing Sonicfast EQ/tap channel count.
pub const PCM_CHANNELS: u16 = 2;
/// Maximum source frames submitted to an output in one call.
pub const PCM_BLOCK_FRAMES: usize = 1024;
/// Number of bounded encoded chunks queued between HTTP and the decoder.
pub const INPUT_QUEUE_CHUNKS: usize = 16;
/// Queue plus current-reader/in-flight encoded bytes (excluding HTTP/MSS buffers).
pub const INPUT_BUDGET_BYTES: usize =
    (INPUT_QUEUE_CHUNKS + 2) * libresonic_core::STREAM_CHUNK_BYTES;

/// A cancellable load/seek generation, including output backpressure waits.
#[derive(Clone, Default)]
pub struct Cancellation(Arc<CancelState>);

#[derive(Default)]
struct CancelState {
    cancelled: AtomicBool,
    notify: Notify,
}

impl Cancellation {
    /// Cancel this generation and wake async waiters.
    pub fn cancel(&self) {
        self.0.cancelled.store(true, Ordering::SeqCst);
        self.0.notify.notify_waiters();
    }
    /// Nonblocking cancellation check for decoder/output threads.
    pub fn is_cancelled(&self) -> bool {
        self.0.cancelled.load(Ordering::SeqCst)
    }
    /// Wait without losing notifications that race with registration.
    pub async fn cancelled(&self) {
        let notified = self.0.notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if !self.is_cancelled() {
            notified.await;
        }
    }
}

/// Validated interleaved source PCM, prior to app-owned rate/channel conversion.
pub struct PcmBlock<'a> {
    samples: &'a [f64],
    rate: u32,
    channels: u16,
}

impl<'a> PcmBlock<'a> {
    /// Accept finite mono/stereo PCM at 8..192 kHz, at most PCM_BLOCK_FRAMES.
    pub fn new(samples: &'a [f64], rate: u32, channels: u16) -> Result<Self, OutputError> {
        if !(8000..=192000).contains(&rate)
            || !matches!(channels, 1 | 2)
            || samples.is_empty()
            || !samples.len().is_multiple_of(usize::from(channels))
            || samples.len() / usize::from(channels) > PCM_BLOCK_FRAMES
            || samples.iter().any(|v| !v.is_finite())
        {
            return Err(OutputError::InvalidPcm);
        }
        Ok(Self {
            samples,
            rate,
            channels,
        })
    }
    /// Interleaved finite samples.
    pub fn samples(&self) -> &'a [f64] {
        self.samples
    }
    /// Source sample rate.
    pub fn sample_rate(&self) -> u32 {
        self.rate
    }
    /// Source channels (one or two).
    pub fn channels(&self) -> u16 {
        self.channels
    }
}

/// Recoverable output failures without backend-specific errors.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OutputError {
    /// No usable or explicitly selected output exists.
    NoDevice,
    /// The output failed while playing.
    DeviceLost,
    /// Invalid or unsupported PCM format/data.
    InvalidPcm,
    /// A generation was cancelled during a bounded write.
    Cancelled,
}

/// Output owned exclusively by the feeder thread. No UI or network calls here.
pub trait AudioOutput: Send {
    /// Discard queued audio and processing history for a load/seek.
    fn reset(&mut self) -> Result<(), OutputError>;
    /// Queue bounded PCM, checking cancellation while waiting for queue space.
    fn write(&mut self, block: PcmBlock<'_>, cancel: &Cancellation) -> Result<(), OutputError>;
    /// Update output gain, including already queued audio.
    fn set_volume(&mut self, volume: u16);
    /// Pause output without discarding the queued position.
    fn pause(&mut self);
    /// Resume the retained queue/device.
    fn resume(&mut self) -> Result<(), OutputError>;
    /// Estimated output backlog in milliseconds.
    fn pending_ms(&self) -> u64;
    /// Flush bounded processing tail before natural end, preserving cancellation.
    fn finish(&mut self, _cancel: &Cancellation) -> Result<(), OutputError> {
        Ok(())
    }
    /// True once submitted audio has drained; called with transport checks between polls.
    fn drained(&mut self) -> Result<bool, OutputError>;
    /// Stop/discard output. Must not wait indefinitely.
    fn stop(&mut self);
}

/// Explicit transport phase, independent of a server's playback state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// No active transport; a selected track may remain.
    Stopped,
    /// Opening a stream and probing a codec.
    Loading,
    /// Waiting for stream/decode input.
    Buffering,
    /// Submitting/playing PCM.
    Playing,
    /// Paused at local intent, including during load/seek.
    Paused,
    /// Reopening/decoding toward a new position.
    Seeking,
    /// Natural end after output drain.
    Ended,
    /// A recoverable error; another load/play may retry.
    Error,
}

/// Safe errors from service, decoder or audio output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlaybackError {
    /// Auth/network/protocol failure with sanitized details.
    Service(Error),
    /// Unsupported container/codec or source rate/channel layout.
    UnsupportedMedia,
    /// Malformed encoded audio.
    Decode,
    /// Native output failure.
    Output(OutputError),
    /// A command requires a selected track.
    NoTrack,
    /// Worker creation/shutdown failure.
    Worker,
}

impl fmt::Display for PlaybackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Service(error) => write!(f, "{error}"),
            Self::UnsupportedMedia => {
                f.write_str("Unsupported audio; use MP3, FLAC or M4A (AAC/ALAC), mono or stereo. M4A with end metadata requires byte-range support")
            }
            Self::Decode => f.write_str("Unable to decode the audio stream"),
            Self::Output(OutputError::NoDevice) => {
                f.write_str("No usable audio output device was found")
            }
            Self::Output(OutputError::DeviceLost) => {
                f.write_str("The audio output stopped working")
            }
            Self::Output(OutputError::InvalidPcm) => f.write_str("Invalid audio samples or format"),
            Self::Output(OutputError::Cancelled) => f.write_str("Playback cancelled"),
            Self::NoTrack => f.write_str("Select a track first"),
            Self::Worker => f.write_str("Playback worker stopped"),
        }
    }
}
impl std::error::Error for PlaybackError {}

/// Latest local transport snapshot. No credential-bearing URLs or wire DTOs.
#[derive(Clone, Debug)]
pub struct PlaybackState {
    /// Selected track, including when stopped/ended.
    pub track: Option<MediaId>,
    /// Local transport phase.
    pub phase: Phase,
    /// Load/seek generation; stale events cannot replace newer intent.
    pub generation: u64,
    /// Estimated heard position, excluding submitted output backlog.
    pub position_ms: u64,
    /// Decoder-reported duration, when available.
    pub duration_ms: Option<u64>,
    /// Local volume in the full u16 range.
    pub volume: u16,
    /// Current safe failure, if any.
    pub error: Option<PlaybackError>,
    /// Milliseconds until first PCM submission completed, not a DAC latency metric.
    pub startup_ms: Option<u64>,
    /// Source frames decoded in this generation, including seek discard.
    pub decoded_frames: u64,
    /// Encoded bytes received in this generation.
    pub encoded_bytes: u64,
    /// Peak encoded queue/reader/in-flight bytes; bounded by INPUT_BUDGET_BYTES.
    pub peak_input_bytes: usize,
}

impl Default for PlaybackState {
    fn default() -> Self {
        Self {
            track: None,
            phase: Phase::Stopped,
            generation: 0,
            position_ms: 0,
            duration_ms: None,
            volume: u16::MAX / 2,
            error: None,
            startup_ms: None,
            decoded_frames: 0,
            encoded_bytes: 0,
            peak_input_bytes: 0,
        }
    }
}
