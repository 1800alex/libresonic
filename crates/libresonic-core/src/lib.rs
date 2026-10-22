//! Server-independent identifiers, music models, errors and async service API.

mod capability;
mod error;
mod id;
mod model;
mod service;
mod stream;

pub use capability::*;
pub use error::Error;
pub use id::{MediaId, MediaKind, ServerId};
pub use model::*;
pub use service::{MusicService, ServiceFuture};
pub use stream::{AudioStream, STREAM_CHUNK_BYTES, StreamProvider};
