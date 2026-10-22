//! Bounded, single-track MP3/FLAC/M4A playback without an audio-device dependency.
//!
//! A feeder thread owns decoding and output. HTTP is supplied through the core
//! StreamProvider. Seek reopens the stream and decodes forward; no range or
//! server-offset accuracy is assumed. Drop cancels; shutdown waits asynchronously.

mod decoder;
mod player;
pub mod queue;
pub mod scrobble;
mod types;

pub use player::Player;
pub use types::*;
