use std::io::{self, Read, Seek, SeekFrom};
use std::sync::{
    Arc, Mutex, PoisonError,
    atomic::{AtomicU64, AtomicUsize, Ordering},
};
use std::thread;
use std::time::{Duration, Instant};

use libresonic_core::{AudioStream, Error, STREAM_CHUNK_BYTES, StreamProvider};
use symphonia::core::{
    audio::SampleBuffer,
    codecs::{CODEC_TYPE_AAC, CODEC_TYPE_ALAC, CODEC_TYPE_FLAC, CODEC_TYPE_MP3, DecoderOptions},
    errors::Error as DecodeError,
    formats::FormatOptions,
    io::{MediaSource, MediaSourceStream, MediaSourceStreamOptions},
    meta::{Limit, MetadataOptions},
    probe::Hint,
};
use tokio::runtime::Handle;
use tokio::sync::mpsc;

use crate::player::{Job, Progress, Shared};
use crate::*;

mod input;

#[derive(Default)]
struct InputStats {
    bytes: AtomicU64,
    buffered: AtomicUsize,
    peak: AtomicUsize,
    error: Mutex<Option<Error>>,
}

struct Pipe {
    receiver: mpsc::Receiver<Vec<u8>>,
    current: std::io::Cursor<Vec<u8>>,
    stats: Arc<InputStats>,
    shared: Arc<Shared>,
    job: Job,
}

impl Read for Pipe {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        let waiting = Instant::now();
        loop {
            if self.job.cancel.is_cancelled() {
                return Err(io::Error::new(
                    io::ErrorKind::ConnectionAborted,
                    "playback cancelled",
                ));
            }
            let count = self.current.read(out)?;
            if count > 0 {
                self.stats.buffered.fetch_sub(count, Ordering::Relaxed);
                return Ok(count);
            }
            match self.receiver.try_recv() {
                Ok(bytes) => self.current = io::Cursor::new(bytes),
                Err(mpsc::error::TryRecvError::Disconnected) => return Ok(0),
                Err(mpsc::error::TryRecvError::Empty) => {
                    if waiting.elapsed() > Duration::from_millis(150) {
                        self.shared.phase(&self.job, Phase::Buffering);
                    }
                    thread::sleep(Duration::from_millis(5));
                }
            }
        }
    }
}
impl Seek for Pipe {
    fn seek(&mut self, _: SeekFrom) -> io::Result<u64> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "stream is not seekable",
        ))
    }
}
impl MediaSource for Pipe {
    fn is_seekable(&self) -> bool {
        false
    }
    fn byte_len(&self) -> Option<u64> {
        None
    }
}

async fn pump(
    mut stream: Box<dyn AudioStream>,
    sender: mpsc::Sender<Vec<u8>>,
    stats: Arc<InputStats>,
    cancel: Cancellation,
) {
    loop {
        let next =
            tokio::select! {biased;_=cancel.cancelled()=>return,result=stream.next()=>result};
        match next {
            Ok(Some(bytes)) if bytes.len() <= STREAM_CHUNK_BYTES => {
                if bytes.is_empty() {
                    tokio::task::yield_now().await;
                    continue;
                }
                stats.bytes.fetch_add(bytes.len() as u64, Ordering::Relaxed);
                let buffered =
                    stats.buffered.fetch_add(bytes.len(), Ordering::Relaxed) + bytes.len();
                stats.peak.fetch_max(buffered, Ordering::Relaxed);
                tokio::select! {biased;_=cancel.cancelled()=>return,result=sender.send(bytes)=>if result.is_err(){return}}
            }
            Ok(None) => return,
            other => {
                let error = match other {
                    Err(error) => error,
                    _ => Error::ResponseTooLarge,
                };
                *stats.error.lock().unwrap_or_else(PoisonError::into_inner) = Some(error);
                return;
            }
        }
    }
}

pub(crate) fn run(
    runtime: &Handle,
    provider: Arc<dyn StreamProvider>,
    output: &mut dyn AudioOutput,
    shared: Arc<Shared>,
    job: &Job,
) -> Result<(), PlaybackError> {
    let started = Instant::now();
    shared.checkpoint(job, output)?;
    shared.phase(
        job,
        if job.start_ms > 0 {
            Phase::Seeking
        } else {
            Phase::Loading
        },
    );
    let stream=runtime.block_on(async{
        tokio::select!{biased;_=job.cancel.cancelled()=>Err(PlaybackError::Output(OutputError::Cancelled)),result=provider.open(&job.id)=>result.map_err(PlaybackError::Service)}
    })?;
    let (sender, receiver) = mpsc::channel(INPUT_QUEUE_CHUNKS);
    let stats = Arc::new(InputStats::default());
    let (stream, seekable_mp4) = input::inspect(stream, runtime, job)?;
    let (source, producer): (Box<dyn MediaSource>, _) = if seekable_mp4 {
        (
            Box::new(input::Seekable::new(
                stream,
                runtime.clone(),
                job.clone(),
                Arc::clone(&shared),
                Arc::clone(&stats),
            )?),
            None,
        )
    } else {
        let producer = runtime.spawn(pump(stream, sender, Arc::clone(&stats), job.cancel.clone()));
        (
            Box::new(Pipe {
                receiver,
                current: io::Cursor::new(vec![]),
                stats: Arc::clone(&stats),
                shared: Arc::clone(&shared),
                job: job.clone(),
            }),
            Some(producer),
        )
    };
    let result = decode(source, output, &shared, job, &stats, started);
    if let Some(producer) = producer {
        producer.abort();
    }
    if job.cancel.is_cancelled() {
        return Err(PlaybackError::Output(OutputError::Cancelled));
    }
    if let Some(error) = stats
        .error
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take()
    {
        return Err(PlaybackError::Service(error));
    }
    result
}

fn decode(
    source: Box<dyn MediaSource>,
    output: &mut dyn AudioOutput,
    shared: &Shared,
    job: &Job,
    stats: &InputStats,
    started: Instant,
) -> Result<(), PlaybackError> {
    shared.phase(job, Phase::Buffering);
    let mss = MediaSourceStream::new(source, MediaSourceStreamOptions::default());
    let meta = MetadataOptions {
        limit_metadata_bytes: Limit::Maximum(1024 * 1024),
        limit_visual_bytes: Limit::Maximum(1024 * 1024),
    };
    let format = FormatOptions {
        enable_gapless: true,
        ..FormatOptions::default()
    };
    let mut reader = symphonia::default::get_probe()
        .format(&Hint::new(), mss, &format, &meta)
        .map_err(|_| PlaybackError::UnsupportedMedia)?
        .format;
    let track = reader
        .tracks()
        .iter()
        .find(|track| {
            matches!(
                track.codec_params.codec,
                CODEC_TYPE_MP3 | CODEC_TYPE_FLAC | CODEC_TYPE_AAC | CODEC_TYPE_ALAC
            )
        })
        .ok_or(PlaybackError::UnsupportedMedia)?;
    if track.codec_params.channels.is_some_and(|c| c.count() > 2) {
        return Err(PlaybackError::UnsupportedMedia);
    }
    let track_id = track.id;
    let duration = track
        .codec_params
        .time_base
        .zip(track.codec_params.n_frames)
        .map(|(base, frames)| {
            let time = base.calc_time(frames);
            time.seconds.saturating_mul(1000) + (time.frac * 1000.0) as u64
        });
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|_| PlaybackError::UnsupportedMedia)?;
    let target = duration.map_or(job.start_ms, |d| job.start_ms.min(d));
    let mut seconds = 0.0f64;
    let mut frames = 0u64;
    let mut startup = None;
    let mut wrote = false;
    loop {
        shared.checkpoint(job, output)?;
        let packet = match reader.next_packet() {
            Ok(packet) => packet,
            Err(DecodeError::IoError(error)) if error.kind() == io::ErrorKind::UnexpectedEof => {
                break;
            }
            Err(_) => return Err(PlaybackError::Decode),
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = decoder.decode(&packet).map_err(|_| PlaybackError::Decode)?;
        let spec = *decoded.spec();
        let channels = spec.channels.count();
        let count = decoded.frames();
        if channels == 0
            || channels > 2
            || !(8000..=192000).contains(&spec.rate)
            || decoded.capacity() > 65536
        {
            return Err(PlaybackError::UnsupportedMedia);
        }
        if count == 0 {
            continue;
        }
        let mut buffer = SampleBuffer::<f64>::new(decoded.capacity() as u64, spec);
        buffer.copy_interleaved_ref(decoded);
        let start = (((target as f64 / 1000.0 - seconds).max(0.0) * f64::from(spec.rate)).round()
            as usize)
            .min(count);
        frames = frames.saturating_add(count as u64);
        for (index, samples) in buffer.samples()[start * channels..]
            .chunks(PCM_BLOCK_FRAMES * channels)
            .enumerate()
        {
            shared.checkpoint(job, output)?;
            let block = PcmBlock::new(samples, spec.rate, channels as u16)
                .map_err(PlaybackError::Output)?;
            output
                .write(block, &job.cancel)
                .map_err(PlaybackError::Output)?;
            wrote = true;
            if startup.is_none() {
                startup = Some(started.elapsed().as_millis() as u64)
            }
            let played_frames = start + index * PCM_BLOCK_FRAMES + samples.len() / channels;
            let position =
                ((seconds + played_frames as f64 / f64::from(spec.rate)) * 1000.0) as u64;
            shared.progress(
                job,
                Progress {
                    position: position.saturating_sub(output.pending_ms()).max(target),
                    duration,
                    frames,
                    bytes: stats.bytes.load(Ordering::Relaxed),
                    peak: stats.peak.load(Ordering::Relaxed),
                    startup,
                },
            );
        }
        seconds += count as f64 / f64::from(spec.rate);
        if start == count {
            shared.progress(
                job,
                Progress {
                    position: target,
                    duration,
                    frames,
                    bytes: stats.bytes.load(Ordering::Relaxed),
                    peak: stats.peak.load(Ordering::Relaxed),
                    startup,
                },
            );
        }
    }
    if let Some(error) = stats
        .error
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
    {
        return Err(PlaybackError::Service(error));
    }
    let position = (seconds * 1000.0) as u64;
    if frames == 0 || duration.is_some_and(|duration| position.saturating_add(100) < duration) {
        return Err(PlaybackError::Decode);
    }
    if wrote {
        output.finish(&job.cancel).map_err(PlaybackError::Output)?;
    }
    let mut drain_time = Duration::ZERO;
    loop {
        shared.checkpoint(job, output)?;
        let at = Instant::now();
        if output.drained().map_err(PlaybackError::Output)? && shared.ended(job, position) {
            break;
        }
        shared.progress(
            job,
            Progress {
                position: position.saturating_sub(output.pending_ms()).max(target),
                duration,
                frames,
                bytes: stats.bytes.load(Ordering::Relaxed),
                peak: stats.peak.load(Ordering::Relaxed),
                startup,
            },
        );
        if drain_time > Duration::from_secs(5) {
            return Err(PlaybackError::Output(OutputError::DeviceLost));
        }
        thread::sleep(Duration::from_millis(10));
        drain_time += at.elapsed();
    }
    Ok(())
}
