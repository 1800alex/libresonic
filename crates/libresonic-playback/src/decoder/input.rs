//! Bounded sniffing and byte-seek input for MP4's non-linear atom layout.
use super::*;
use std::collections::VecDeque;

struct Prefixed {
    prefix: VecDeque<Vec<u8>>,
    inner: Box<dyn AudioStream>,
}
impl AudioStream for Prefixed {
    fn byte_len(&self) -> Option<u64> {
        self.inner.byte_len()
    }
    fn is_seekable(&self) -> bool {
        self.inner.is_seekable()
    }
    fn seek_to(&mut self, offset: u64) -> libresonic_core::ServiceFuture<'_, ()> {
        Box::pin(async move {
            self.inner.seek_to(offset).await?;
            self.prefix.clear();
            Ok(())
        })
    }
    fn next(&mut self) -> libresonic_core::ServiceFuture<'_, Option<Vec<u8>>> {
        Box::pin(async move {
            if let Some(bytes) = self.prefix.pop_front() {
                Ok(Some(bytes))
            } else {
                self.inner.next().await
            }
        })
    }
}

pub(super) fn inspect(
    mut stream: Box<dyn AudioStream>,
    runtime: &Handle,
    job: &Job,
) -> Result<(Box<dyn AudioStream>, bool), PlaybackError> {
    if !stream.is_seekable() {
        return Ok((stream, false));
    }
    // Probe only a fixed header. Even if each read returns a single byte,
    // retained prefix data is at most one public chunk plus seven bytes.
    let mut prefix = VecDeque::new();
    let mut header = Vec::new();
    while header.len() < 8 {
        let next = runtime.block_on(async {
            tokio::select! { biased;
                _ = job.cancel.cancelled() => Err(PlaybackError::Output(OutputError::Cancelled)),
                result = stream.next() => result.map_err(PlaybackError::Service),
            }
        })?;
        let Some(bytes) = next else { break };
        if bytes.len() > STREAM_CHUNK_BYTES {
            return Err(PlaybackError::Service(Error::ResponseTooLarge));
        }
        if bytes.is_empty() {
            thread::yield_now();
            continue;
        }
        header.extend_from_slice(&bytes[..(8 - header.len()).min(bytes.len())]);
        prefix.push_back(bytes);
    }
    let mp4 = header.get(4..8) == Some(b"ftyp");
    Ok((
        Box::new(Prefixed {
            prefix,
            inner: stream,
        }),
        mp4,
    ))
}

pub(super) struct Seekable {
    stream: Mutex<Box<dyn AudioStream>>,
    runtime: Handle,
    job: Job,
    shared: Arc<Shared>,
    stats: Arc<InputStats>,
    current: io::Cursor<Vec<u8>>,
    position: u64,
    length: u64,
}
impl Seekable {
    pub(super) fn new(
        stream: Box<dyn AudioStream>,
        runtime: Handle,
        job: Job,
        shared: Arc<Shared>,
        stats: Arc<InputStats>,
    ) -> Result<Self, PlaybackError> {
        let length = stream.byte_len().ok_or(PlaybackError::UnsupportedMedia)?;
        Ok(Self {
            stream: Mutex::new(stream),
            runtime,
            job,
            shared,
            stats,
            current: io::Cursor::new(vec![]),
            position: 0,
            length,
        })
    }
    fn failure(&self, error: Error) -> io::Error {
        *self
            .stats
            .error
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(error);
        io::Error::other("encoded stream operation failed")
    }
}
impl Read for Seekable {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        loop {
            if self.job.cancel.is_cancelled() {
                return Err(io::Error::new(
                    io::ErrorKind::ConnectionAborted,
                    "playback cancelled",
                ));
            }
            let count = self.current.read(out)?;
            if count > 0 {
                self.position += count as u64;
                self.stats.buffered.fetch_sub(count, Ordering::Relaxed);
                return Ok(count);
            }
            let next = self
                .runtime
                .block_on(async {
                    let next = self
                        .stream
                        .get_mut()
                        .unwrap_or_else(PoisonError::into_inner)
                        .next();
                    tokio::pin!(next);
                    tokio::select! { biased;
                        _ = self.job.cancel.cancelled() => return Err(Error::Network),
                        result = &mut next => return result,
                        _ = tokio::time::sleep(Duration::from_millis(150)) => {},
                    }
                    self.shared.phase(&self.job, Phase::Buffering);
                    tokio::select! { biased;
                        _ = self.job.cancel.cancelled() => Err(Error::Network),
                        result = next => result,
                    }
                })
                .map_err(|e| self.failure(e))?;
            match next {
                None => return Ok(0),
                Some(bytes) if bytes.len() <= STREAM_CHUNK_BYTES => {
                    self.stats
                        .bytes
                        .fetch_add(bytes.len() as u64, Ordering::Relaxed);
                    self.stats.buffered.store(bytes.len(), Ordering::Relaxed);
                    self.stats.peak.fetch_max(bytes.len(), Ordering::Relaxed);
                    self.current = io::Cursor::new(bytes);
                }
                _ => return Err(self.failure(Error::ResponseTooLarge)),
            }
        }
    }
}
impl Seek for Seekable {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let position = match from {
            SeekFrom::Start(n) => Some(n),
            SeekFrom::Current(n) => self.position.checked_add_signed(n),
            SeekFrom::End(n) => self.length.checked_add_signed(n),
        }
        .filter(|n| *n <= self.length)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid byte offset"))?;
        if position == self.position {
            return Ok(position);
        }
        self.runtime
            .block_on(async {
                tokio::select! { biased;
                    _ = self.job.cancel.cancelled() => Err(Error::Network),
                result = self.stream.get_mut().unwrap_or_else(PoisonError::into_inner).seek_to(position) => result,
                }
            })
            .map_err(|e| self.failure(e))?;
        self.current = io::Cursor::new(vec![]);
        self.stats.buffered.store(0, Ordering::Relaxed);
        self.position = position;
        Ok(position)
    }
}
impl MediaSource for Seekable {
    fn is_seekable(&self) -> bool {
        true
    }
    fn byte_len(&self) -> Option<u64> {
        Some(self.length)
    }
}
