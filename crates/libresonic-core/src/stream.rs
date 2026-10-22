use crate::{MediaId, ServiceFuture};

/// Maximum encoded bytes returned by one stream read.
pub const STREAM_CHUNK_BYTES: usize = 16 * 1024;

/// Authenticated encoded audio without a public URL or transport-specific type.
pub trait AudioStream: Send {
    /// Complete encoded byte length, when known (not the remaining length).
    fn byte_len(&self) -> Option<u64> {
        None
    }
    /// Whether absolute byte seeks are supported for this representation.
    fn is_seekable(&self) -> bool {
        false
    }
    /// Reposition the next read. Adapters must validate ranged responses and
    /// never return bytes from a different offset as if the seek succeeded.
    fn seek_to(&mut self, _offset: u64) -> ServiceFuture<'_, ()> {
        Box::pin(async { Err(crate::Error::Unsupported) })
    }
    /// Next bounded chunk, or EOF. Dropping this future/stream cancels its work.
    fn next(&mut self) -> ServiceFuture<'_, Option<Vec<u8>>>;
}

/// Opens direct-play streams. Authentication and URLs remain inside the adapter.
pub trait StreamProvider: Send + Sync {
    /// Open one track; each chunk must respect STREAM_CHUNK_BYTES.
    fn open<'a>(&'a self, id: &'a MediaId) -> ServiceFuture<'a, Box<dyn AudioStream>>;
}
