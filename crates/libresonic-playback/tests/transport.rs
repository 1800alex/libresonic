//! Decoder/transport regressions with generated audio and a bounded fake output.

use libresonic_core::*;
use libresonic_playback::*;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

const MP3: &[u8] = include_bytes!("fixtures/tone.mp3");
const FLAC: &[u8] = include_bytes!("fixtures/tone.flac");
const AAC_FAST: &[u8] = include_bytes!("fixtures/tone-aac-fast.m4a");
const AAC_TAIL: &[u8] = include_bytes!("fixtures/tone-aac-tail.m4a");
const ALAC_FAST: &[u8] = include_bytes!("fixtures/tone-alac-fast.m4a");
const ALAC_TAIL: &[u8] = include_bytes!("fixtures/tone-alac-tail.m4a");

struct FixtureStream {
    bytes: &'static [u8],
    offset: usize,
    total: usize,
    seekable: bool,
    chunk_size: usize,
}
impl AudioStream for FixtureStream {
    fn byte_len(&self) -> Option<u64> {
        Some(self.total as u64)
    }
    fn is_seekable(&self) -> bool {
        self.seekable
    }
    fn seek_to(&mut self, offset: u64) -> ServiceFuture<'_, ()> {
        Box::pin(async move {
            if !self.seekable {
                return Err(Error::Unsupported);
            }
            if offset > self.total as u64 {
                return Err(Error::InvalidResponse);
            }
            self.offset = offset as usize;
            Ok(())
        })
    }
    fn next(&mut self) -> ServiceFuture<'_, Option<Vec<u8>>> {
        Box::pin(async move {
            if self.offset == self.total {
                return Ok(None);
            }
            let length = self.chunk_size.min(self.total - self.offset);
            let chunk = (0..length)
                .map(|i| self.bytes[(self.offset + i) % self.bytes.len()])
                .collect();
            self.offset += length;
            Ok(Some(chunk))
        })
    }
}
struct StalledRead(FixtureStream);
impl AudioStream for StalledRead {
    fn next(&mut self) -> ServiceFuture<'_, Option<Vec<u8>>> {
        Box::pin(async move {
            match self.0.next().await? {
                Some(bytes) => Ok(Some(bytes)),
                None => std::future::pending().await,
            }
        })
    }
}
struct Provider;
impl StreamProvider for Provider {
    fn open<'a>(&'a self, id: &'a MediaId) -> ServiceFuture<'a, Box<dyn AudioStream>> {
        Box::pin(async move {
            if id.item() == "stall" {
                std::future::pending::<()>().await;
            }
            if id.item() == "auth" {
                return Err(Error::Authentication);
            }
            if id.item() == "read-stall" {
                return Ok(Box::new(StalledRead(FixtureStream {
                    bytes: MP3,
                    offset: 0,
                    total: MP3.len(),
                    seekable: false,
                    chunk_size: STREAM_CHUNK_BYTES,
                })) as Box<dyn AudioStream>);
            }
            let bytes = match id.item() {
                "flac" => FLAC,
                "aac-fast" | "aac-stream" | "aac-tiny" => AAC_FAST,
                "aac-tail" | "aac-tail-stream" => AAC_TAIL,
                "alac-fast" | "alac-stream" => ALAC_FAST,
                "alac-tail" => ALAC_TAIL,
                "bad" => b"not audio",
                "truncated" => &MP3[..MP3.len() / 2],
                _ => MP3,
            };
            let repeats = if id.item() == "large" { 200 } else { 1 };
            Ok(Box::new(FixtureStream {
                bytes,
                offset: 0,
                total: bytes.len() * repeats,
                seekable: (id.item().starts_with("aac") || id.item().starts_with("alac"))
                    && !id.item().ends_with("stream"),
                chunk_size: if id.item() == "aac-tiny" {
                    3
                } else {
                    STREAM_CHUNK_BYTES
                },
            }) as Box<dyn AudioStream>)
        })
    }
}

struct RangeGate {
    stream: FixtureStream,
    entered: Arc<AtomicBool>,
    error: Option<Error>,
}
impl AudioStream for RangeGate {
    fn byte_len(&self) -> Option<u64> {
        self.stream.byte_len()
    }
    fn is_seekable(&self) -> bool {
        true
    }
    fn next(&mut self) -> ServiceFuture<'_, Option<Vec<u8>>> {
        self.stream.next()
    }
    fn seek_to(&mut self, _: u64) -> ServiceFuture<'_, ()> {
        Box::pin(async move {
            self.entered.store(true, Ordering::SeqCst);
            if let Some(error) = &self.error {
                return Err(error.clone());
            }
            std::future::pending().await
        })
    }
}
struct RangeProvider {
    entered: Arc<AtomicBool>,
    error: Option<Error>,
}
impl StreamProvider for RangeProvider {
    fn open<'a>(&'a self, id: &'a MediaId) -> ServiceFuture<'a, Box<dyn AudioStream>> {
        Box::pin(async move {
            if id.item() == "flac" {
                return Provider.open(id).await;
            }
            Ok(Box::new(RangeGate {
                stream: FixtureStream {
                    bytes: AAC_TAIL,
                    offset: 0,
                    total: AAC_TAIL.len(),
                    seekable: true,
                    chunk_size: STREAM_CHUNK_BYTES,
                },
                entered: self.entered.clone(),
                error: self.error.clone(),
            }) as Box<dyn AudioStream>)
        })
    }
}

#[derive(Default)]
struct Captured {
    sessions: Vec<Vec<f64>>,
    rate: u32,
    channels: u16,
    paused: bool,
    volume: u16,
}
struct FakeOutput {
    captured: Arc<Mutex<Captured>>,
    block: Arc<AtomicBool>,
    entered: Arc<AtomicBool>,
    fail: Arc<AtomicBool>,
    drain: Arc<AtomicBool>,
    pace: Duration,
}
impl AudioOutput for FakeOutput {
    fn reset(&mut self) -> Result<(), OutputError> {
        self.captured.lock().unwrap().sessions.push(vec![]);
        Ok(())
    }
    fn write(&mut self, block: PcmBlock<'_>, cancel: &Cancellation) -> Result<(), OutputError> {
        self.entered.store(true, Ordering::SeqCst);
        while self.block.load(Ordering::SeqCst) {
            if cancel.is_cancelled() {
                return Err(OutputError::Cancelled);
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        if self.fail.load(Ordering::SeqCst) {
            return Err(OutputError::NoDevice);
        }
        {
            let mut c = self.captured.lock().unwrap();
            c.rate = block.sample_rate();
            c.channels = block.channels();
            c.sessions
                .last_mut()
                .unwrap()
                .extend_from_slice(block.samples());
        }
        let start = std::time::Instant::now();
        while start.elapsed() < self.pace {
            if cancel.is_cancelled() {
                return Err(OutputError::Cancelled);
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        Ok(())
    }
    fn set_volume(&mut self, volume: u16) {
        self.captured.lock().unwrap().volume = volume;
    }
    fn pause(&mut self) {
        self.captured.lock().unwrap().paused = true;
    }
    fn resume(&mut self) -> Result<(), OutputError> {
        self.captured.lock().unwrap().paused = false;
        Ok(())
    }
    fn pending_ms(&self) -> u64 {
        0
    }
    fn drained(&mut self) -> Result<bool, OutputError> {
        Ok(self.drain.load(Ordering::SeqCst))
    }
    fn stop(&mut self) {}
}

fn output(pace: Duration) -> FakeOutput {
    FakeOutput {
        captured: Arc::default(),
        block: Arc::default(),
        entered: Arc::default(),
        fail: Arc::default(),
        drain: Arc::new(AtomicBool::new(true)),
        pace,
    }
}
fn id(item: &str) -> MediaId {
    MediaId::new(ServerId::new("fixture").unwrap(), MediaKind::Track, item).unwrap()
}
fn spawn(output: FakeOutput) -> Player {
    Player::spawn(
        Arc::new(Provider),
        Box::new(output),
        tokio::runtime::Handle::current(),
    )
    .unwrap()
}

async fn wait(player: &Player, predicate: impl Fn(&PlaybackState) -> bool) -> PlaybackState {
    let mut updates = player.subscribe();
    tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            let state = updates.borrow().clone();
            if predicate(&state) {
                return state;
            }
            assert_ne!(state.phase, Phase::Error, "{:?}", state.error);
            updates.changed().await.unwrap();
        }
    })
    .await
    .expect("playback transition timed out")
}
async fn until(predicate: impl Fn() -> bool) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while !predicate() {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn mp3_and_flac_decode_to_finite_source_pcm_with_real_duration() {
    for (format, rate, channels) in [("mp3", 44100, 2), ("flac", 48000, 1)] {
        let output = output(Duration::ZERO);
        let captured = Arc::clone(&output.captured);
        let player = spawn(output);
        player.load(id(format), true).unwrap();
        let state = wait(&player, |s| s.phase == Phase::Ended).await;
        assert!((state.position_ms as i64 - 2000).abs() <= 1);
        assert_eq!(state.duration_ms, Some(2000));
        assert!(state.encoded_bytes > 0);
        assert!(state.peak_input_bytes <= INPUT_BUDGET_BYTES);
        assert!(state.startup_ms.is_some());
        {
            let c = captured.lock().unwrap();
            assert_eq!((c.rate, c.channels), (rate, channels));
            assert_eq!(
                c.sessions[0].len(),
                rate as usize * 2 * usize::from(channels)
            );
            assert!(c.sessions[0].iter().all(|x| x.is_finite()));
            assert!(c.sessions[0].iter().any(|x| x.abs() > 0.01));
        }
        player.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn m4a_aac_and_alac_play_front_and_tail_metadata_with_bounded_reads() {
    for format in [
        "aac-fast",
        "aac-tail",
        "alac-fast",
        "alac-tail",
        "aac-stream",
        "alac-stream",
        "aac-tiny",
    ] {
        let output = output(Duration::ZERO);
        let captured = Arc::clone(&output.captured);
        let player = spawn(output);
        player.load(id(format), true).unwrap();
        let state = wait(&player, |s| s.phase == Phase::Ended).await;
        assert!(
            (1950..=2100).contains(&state.position_ms),
            "{format}: {state:?}"
        );
        assert!(
            (1950..=2100).contains(&state.duration_ms.unwrap()),
            "{format}: {state:?}"
        );
        assert!(state.peak_input_bytes <= INPUT_BUDGET_BYTES);
        {
            let c = captured.lock().unwrap();
            assert_eq!((c.rate, c.channels), (44100, 2));
            assert!(c.sessions[0].iter().all(|x| x.is_finite()));
            assert!(c.sessions[0].iter().any(|x| x.abs() > 0.01));
        }
        player.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn m4a_tail_without_ranges_fails_safely_and_the_player_can_recover() {
    let player = spawn(output(Duration::ZERO));
    player.load(id("aac-tail-stream"), true).unwrap();
    let state = wait(&player, |s| s.phase == Phase::Error).await;
    assert_eq!(state.error, Some(PlaybackError::UnsupportedMedia));
    player.load(id("aac-stream"), true).unwrap();
    wait(&player, |s| s.phase == Phase::Ended).await;
    player.shutdown().await.unwrap();
}

#[tokio::test]
async fn stopping_or_loading_another_track_cancels_a_stalled_m4a_byte_seek() {
    for stop in [true, false] {
        let entered = Arc::new(AtomicBool::new(false));
        let player = Player::spawn(
            Arc::new(RangeProvider {
                entered: entered.clone(),
                error: None,
            }),
            Box::new(output(Duration::ZERO)),
            tokio::runtime::Handle::current(),
        )
        .unwrap();
        player.load(id("m4a"), true).unwrap();
        until(|| entered.load(Ordering::SeqCst)).await;
        if stop {
            player.stop();
        } else {
            player.load(id("flac"), true).unwrap();
            let state = wait(&player, |s| s.phase == Phase::Ended).await;
            assert_eq!(state.track, Some(id("flac")));
        }
        tokio::time::timeout(Duration::from_secs(1), player.shutdown())
            .await
            .unwrap()
            .unwrap();
    }
}

#[tokio::test]
async fn m4a_byte_seek_errors_keep_the_safe_service_failure_instead_of_a_probe_error() {
    let entered = Arc::new(AtomicBool::new(false));
    let player = Player::spawn(
        Arc::new(RangeProvider {
            entered: entered.clone(),
            error: Some(Error::Authentication),
        }),
        Box::new(output(Duration::ZERO)),
        tokio::runtime::Handle::current(),
    )
    .unwrap();
    player.load(id("m4a"), true).unwrap();
    let state = wait(&player, |s| s.phase == Phase::Error).await;
    assert!(entered.load(Ordering::SeqCst));
    assert_eq!(
        state.error,
        Some(PlaybackError::Service(Error::Authentication))
    );
    player.load(id("flac"), true).unwrap();
    wait(&player, |s| s.phase == Phase::Ended).await;
    player.shutdown().await.unwrap();
}

#[tokio::test]
async fn replay_seek_starts_at_the_exact_decoded_sample_for_supported_codecs() {
    for (format, rate, channels) in [
        ("mp3", 44100, 2),
        ("flac", 48000, 1),
        ("aac-fast", 44100, 2),
        ("aac-tail", 44100, 2),
        ("alac-fast", 44100, 2),
        ("alac-tail", 44100, 2),
    ] {
        let output = output(Duration::ZERO);
        let captured = Arc::clone(&output.captured);
        let player = spawn(output);
        player.load(id(format), true).unwrap();
        let original = wait(&player, |s| s.phase == Phase::Ended).await;
        player.seek(500).unwrap();
        let state = wait(&player, |s| s.phase == Phase::Ended && s.generation == 2).await;
        assert_eq!(state.position_ms, original.position_ms);
        {
            let c = captured.lock().unwrap();
            let sample = (rate as usize / 2) * channels;
            assert_eq!(&c.sessions[0][sample..sample + 256], &c.sessions[1][..256]);
        }
        player.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn pause_volume_seek_and_resume_preserve_local_intent() {
    let output = output(Duration::from_millis(5));
    let captured = Arc::clone(&output.captured);
    let player = spawn(output);
    player.load(id("mp3"), true).unwrap();
    wait(&player, |s| s.phase == Phase::Playing).await;
    player.pause();
    until(|| captured.lock().unwrap().paused).await;
    let position = player.snapshot().position_ms;
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(player.snapshot().position_ms, position);
    player.set_volume(0);
    until(|| captured.lock().unwrap().volume == 0).await;
    player.seek(1000).unwrap();
    assert_eq!(player.snapshot().phase, Phase::Paused);
    assert_eq!(player.snapshot().position_ms, 1000);
    player.play().unwrap();
    wait(&player, |s| s.phase == Phase::Playing && s.generation == 2).await;
    player.stop();
    let stopped = player.snapshot();
    assert_eq!(stopped.phase, Phase::Stopped);
    assert_eq!(stopped.position_ms, 0);
    tokio::time::sleep(Duration::from_millis(40)).await;
    assert_eq!(player.snapshot().generation, stopped.generation);
    assert_eq!(player.snapshot().phase, Phase::Stopped);
    player.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_new_load_cancels_a_stalled_open_and_stale_results_cannot_win() {
    let player = spawn(output(Duration::ZERO));
    player.load(id("stall"), true).unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    player.load(id("flac"), true).unwrap();
    let state = wait(&player, |s| s.phase == Phase::Ended).await;
    assert_eq!(state.track, Some(id("flac")));
    assert_eq!(state.generation, 2);
    player.shutdown().await.unwrap();
}

#[tokio::test]
async fn service_decode_and_device_errors_are_recoverable() {
    let output = output(Duration::ZERO);
    let fail = Arc::clone(&output.fail);
    let player = spawn(output);
    for (name, error) in [
        ("auth", PlaybackError::Service(Error::Authentication)),
        ("bad", PlaybackError::UnsupportedMedia),
        ("truncated", PlaybackError::Decode),
    ] {
        player.load(id(name), true).unwrap();
        let state = wait(&player, |s| s.phase == Phase::Error).await;
        assert_eq!(state.error, Some(error));
    }
    fail.store(true, Ordering::SeqCst);
    player.load(id("mp3"), true).unwrap();
    assert_eq!(
        wait(&player, |s| s.phase == Phase::Error).await.error,
        Some(PlaybackError::Output(OutputError::NoDevice))
    );
    fail.store(false, Ordering::SeqCst);
    player.play().unwrap();
    wait(&player, |s| s.phase == Phase::Ended).await;
    player.shutdown().await.unwrap();
}

#[tokio::test]
async fn paused_backpressure_stays_bounded_and_stop_cancels_a_full_input_queue() {
    let output = output(Duration::ZERO);
    let blocked = Arc::clone(&output.block);
    let entered = Arc::clone(&output.entered);
    let captured = Arc::clone(&output.captured);
    blocked.store(true, Ordering::SeqCst);
    let player = spawn(output);
    player.load(id("large"), true).unwrap();
    until(|| entered.load(Ordering::SeqCst)).await;
    tokio::time::sleep(Duration::from_millis(30)).await;
    player.pause();
    blocked.store(false, Ordering::SeqCst);
    until(|| captured.lock().unwrap().paused).await;
    let state = player.snapshot();
    assert!(state.encoded_bytes >= STREAM_CHUNK_BYTES as u64 * INPUT_QUEUE_CHUNKS as u64);
    assert!(state.peak_input_bytes <= INPUT_BUDGET_BYTES);
    player.stop();
    tokio::time::timeout(Duration::from_secs(1), player.shutdown())
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn pause_at_eof_prevents_an_ended_event_until_resumed() {
    let output = output(Duration::ZERO);
    let drain = Arc::clone(&output.drain);
    let captured = Arc::clone(&output.captured);
    drain.store(false, Ordering::SeqCst);
    let player = spawn(output);
    player.load(id("flac"), true).unwrap();
    wait(&player, |s| s.decoded_frames >= 96000).await;
    player.pause();
    until(|| captured.lock().unwrap().paused).await;
    drain.store(true, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(player.snapshot().phase, Phase::Paused);
    player.play().unwrap();
    wait(&player, |s| s.phase == Phase::Ended).await;
    player.shutdown().await.unwrap();
}

#[test]
fn pcm_rejects_unbounded_nonfinite_or_unsupported_blocks() {
    assert!(PcmBlock::new(&[f64::NAN; 2], 44100, 2).is_err());
    assert!(PcmBlock::new(&[0.0; 3], 44100, 2).is_err());
    assert!(PcmBlock::new(&[0.0; 8], 44100, 4).is_err());
    assert!(PcmBlock::new(&[0.0; 8], 0, 2).is_err());
    assert!(PcmBlock::new(&vec![0.0; (PCM_BLOCK_FRAMES + 1) * 2], 44100, 2).is_err());
}

#[tokio::test]
async fn stop_interrupts_a_decoder_waiting_for_more_network_bytes() {
    let player = spawn(output(Duration::from_millis(1)));
    player.load(id("read-stall"), true).unwrap();
    wait(&player, |s| s.phase == Phase::Playing).await;
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert_ne!(player.snapshot().phase, Phase::Ended);
    player.stop();
    tokio::time::timeout(Duration::from_secs(1), player.shutdown())
        .await
        .unwrap()
        .unwrap();
}
