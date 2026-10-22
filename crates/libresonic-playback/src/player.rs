use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;

use libresonic_core::{MediaId, MediaKind, StreamProvider};
use tokio::runtime::Handle;
use tokio::sync::watch;

use crate::{AudioOutput, Cancellation, OutputError, Phase, PlaybackError, PlaybackState};

#[derive(Clone)]
pub(crate) struct Job {
    pub id: MediaId,
    pub generation: u64,
    pub start_ms: u64,
    pub cancel: Cancellation,
}

struct Control {
    state: PlaybackState,
    pending: Option<Job>,
    cancel: Cancellation,
    paused: bool,
    active: bool,
    stop: bool,
    closed: bool,
}

pub(crate) struct Progress {
    pub position: u64,
    pub duration: Option<u64>,
    pub frames: u64,
    pub bytes: u64,
    pub peak: usize,
    pub startup: Option<u64>,
}

pub(crate) struct Shared {
    control: Mutex<Control>,
    wake: Condvar,
    updates: watch::Sender<PlaybackState>,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Control> {
        self.control.lock().unwrap_or_else(PoisonError::into_inner)
    }
    fn publish(&self, control: &Control) {
        self.updates.send_replace(control.state.clone());
    }

    pub fn phase(&self, job: &Job, phase: Phase) {
        let mut control = self.lock();
        if control.state.generation != job.generation || job.cancel.is_cancelled() {
            return;
        }
        control.state.phase = if control.paused { Phase::Paused } else { phase };
        self.publish(&control);
    }

    pub fn checkpoint(&self, job: &Job, output: &mut dyn AudioOutput) -> Result<(), PlaybackError> {
        loop {
            let control = self.lock();
            if control.closed
                || control.state.generation != job.generation
                || job.cancel.is_cancelled()
            {
                return Err(PlaybackError::Output(OutputError::Cancelled));
            }
            let paused = control.paused;
            let volume = control.state.volume;
            drop(control);
            output.set_volume(volume);
            if !paused {
                return output.resume().map_err(PlaybackError::Output);
            }
            output.pause();
            let control = self.lock();
            if control.paused
                && control.state.volume == volume
                && !control.closed
                && !job.cancel.is_cancelled()
            {
                drop(
                    self.wake
                        .wait(control)
                        .unwrap_or_else(PoisonError::into_inner),
                );
            }
        }
    }

    pub fn progress(&self, job: &Job, progress: Progress) {
        let Progress {
            position,
            duration,
            frames,
            bytes,
            peak,
            startup,
        } = progress;
        let mut control = self.lock();
        if control.state.generation != job.generation || job.cancel.is_cancelled() {
            return;
        }
        control.state.position_ms = duration.map_or(position, |d| position.min(d));
        control.state.duration_ms = duration;
        control.state.decoded_frames = frames;
        control.state.encoded_bytes = bytes;
        control.state.peak_input_bytes = peak;
        control.state.startup_ms = startup;
        control.state.phase = if control.paused {
            Phase::Paused
        } else if startup.is_some() {
            Phase::Playing
        } else if job.start_ms > 0 {
            Phase::Seeking
        } else {
            Phase::Buffering
        };
        self.publish(&control);
    }

    pub fn ended(&self, job: &Job, position: u64) -> bool {
        let mut control = self.lock();
        if control.state.generation != job.generation || job.cancel.is_cancelled() {
            return true;
        }
        if control.paused {
            return false;
        }
        control.state.phase = Phase::Ended;
        control.state.position_ms = control.state.duration_ms.unwrap_or(position);
        control.active = false;
        self.publish(&control);
        true
    }

    fn fail(&self, job: &Job, error: PlaybackError) {
        let mut control = self.lock();
        if control.state.generation != job.generation || job.cancel.is_cancelled() {
            return;
        }
        control.state.phase = Phase::Error;
        control.state.error = Some(error);
        control.active = false;
        self.publish(&control);
    }
}

/// Thread-backed single-track controller. Commands are nonblocking local intent.
pub struct Player {
    shared: Arc<Shared>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Player {
    /// Spawn one feeder thread. Output construction must not open devices on a UI thread.
    pub fn spawn(
        provider: Arc<dyn StreamProvider>,
        mut output: Box<dyn AudioOutput>,
        runtime: Handle,
    ) -> Result<Self, PlaybackError> {
        let state = PlaybackState::default();
        let (updates, _) = watch::channel(state.clone());
        let shared = Arc::new(Shared {
            control: Mutex::new(Control {
                state,
                pending: None,
                cancel: Cancellation::default(),
                paused: false,
                active: false,
                stop: false,
                closed: false,
            }),
            wake: Condvar::new(),
            updates,
        });
        let worker_shared = Arc::clone(&shared);
        let worker = thread::Builder::new()
            .name("libresonic-feeder".into())
            .spawn(move || {
                loop {
                    let mut control = worker_shared.lock();
                    while control.pending.is_none() && !control.stop && !control.closed {
                        control = worker_shared
                            .wake
                            .wait(control)
                            .unwrap_or_else(PoisonError::into_inner);
                    }
                    if control.closed {
                        break;
                    }
                    if control.stop {
                        control.stop = false;
                        drop(control);
                        output.stop();
                        continue;
                    }
                    let Some(job) = control.pending.take() else {
                        continue;
                    };
                    drop(control);
                    let result = output
                        .reset()
                        .map_err(PlaybackError::Output)
                        .and_then(|()| {
                            crate::decoder::run(
                                &runtime,
                                Arc::clone(&provider),
                                &mut *output,
                                Arc::clone(&worker_shared),
                                &job,
                            )
                        });
                    output.stop();
                    if let Err(error) = result {
                        worker_shared.fail(&job, error);
                    }
                }
                output.stop();
            })
            .map_err(|_| PlaybackError::Worker)?;
        Ok(Self {
            shared,
            worker: Some(worker),
        })
    }

    /// Latest state; no network/decoder work.
    pub fn snapshot(&self) -> PlaybackState {
        self.shared.updates.borrow().clone()
    }
    /// Subscribe to bounded latest-state updates. Copy borrowed state before awaiting.
    pub fn subscribe(&self) -> watch::Receiver<PlaybackState> {
        self.shared.updates.subscribe()
    }

    fn request(control: &mut Control, id: MediaId, start_ms: u64, play: bool, phase: Phase) {
        control.cancel.cancel();
        control.cancel = Cancellation::default();
        let generation = control.state.generation.wrapping_add(1);
        control.state = PlaybackState {
            track: Some(id.clone()),
            generation,
            position_ms: start_ms,
            phase,
            volume: control.state.volume,
            ..PlaybackState::default()
        };
        control.paused = !play;
        if control.paused {
            control.state.phase = Phase::Paused;
        }
        control.active = true;
        control.pending = Some(Job {
            id,
            generation,
            start_ms,
            cancel: control.cancel.clone(),
        });
    }

    /// Replace the selected track; abandoned generations are cancelled.
    pub fn load(&self, id: MediaId, play: bool) -> Result<(), PlaybackError> {
        if id.kind() != MediaKind::Track {
            return Err(PlaybackError::Service(
                libresonic_core::Error::InvalidIdentifier,
            ));
        }
        let mut control = self.shared.lock();
        Self::request(&mut control, id, 0, play, Phase::Loading);
        self.shared.publish(&control);
        self.shared.wake.notify_all();
        Ok(())
    }

    /// Pause immediately in state; the feeder pauses the retained output queue.
    pub fn pause(&self) {
        let mut control = self.shared.lock();
        control.paused = true;
        if control.active {
            control.state.phase = Phase::Paused;
        }
        self.shared.publish(&control);
        self.shared.wake.notify_all();
    }

    /// Resume paused playback, or restart a stopped/ended/failed selection.
    pub fn play(&self) -> Result<(), PlaybackError> {
        let mut control = self.shared.lock();
        let id = control.state.track.clone().ok_or(PlaybackError::NoTrack)?;
        if !control.active {
            Self::request(&mut control, id, 0, true, Phase::Loading);
        } else {
            control.paused = false;
            control.state.phase = if control.state.startup_ms.is_some() {
                Phase::Playing
            } else {
                Phase::Loading
            };
        }
        self.shared.publish(&control);
        self.shared.wake.notify_all();
        Ok(())
    }

    /// Reopen/decode forward to a sample position; preserve pause intent.
    pub fn seek(&self, position_ms: u64) -> Result<(), PlaybackError> {
        let mut control = self.shared.lock();
        let id = control.state.track.clone().ok_or(PlaybackError::NoTrack)?;
        let target = control
            .state
            .duration_ms
            .map_or(position_ms, |d| position_ms.min(d));
        let play = !control.paused;
        Self::request(&mut control, id, target, play, Phase::Seeking);
        self.shared.publish(&control);
        self.shared.wake.notify_all();
        Ok(())
    }

    /// Cancel/discard transport and reset position, keeping the selected track.
    pub fn stop(&self) {
        let mut control = self.shared.lock();
        control.cancel.cancel();
        control.pending = None;
        control.active = false;
        control.stop = true;
        control.state.generation = control.state.generation.wrapping_add(1);
        control.state.phase = Phase::Stopped;
        control.state.position_ms = 0;
        control.state.error = None;
        self.shared.publish(&control);
        self.shared.wake.notify_all();
    }

    /// Change gain including during pause, with no server request.
    pub fn set_volume(&self, volume: u16) {
        let mut control = self.shared.lock();
        control.state.volume = volume;
        self.shared.publish(&control);
        self.shared.wake.notify_all();
    }

    fn close(&self) {
        let mut control = self.shared.lock();
        control.closed = true;
        control.cancel.cancel();
        control.pending = None;
        self.shared.wake.notify_all();
    }

    /// Cancel and wait for worker exit without blocking the async caller's thread.
    pub async fn shutdown(mut self) -> Result<(), PlaybackError> {
        self.close();
        if let Some(worker) = self.worker.take() {
            tokio::task::spawn_blocking(move || worker.join())
                .await
                .map_err(|_| PlaybackError::Worker)?
                .map_err(|_| PlaybackError::Worker)?;
        }
        Ok(())
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.close();
    }
}
