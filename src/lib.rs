//! Neutral music models and an async OpenSubsonic service for Sonicfast.
//!
//! Network DTOs, credentials and authenticated URLs never form domain models.
//! The unpublished playback API provides single-track streamed transport.

pub use libresonic_core::*;
/// OpenSubsonic connection configuration and service implementation.
pub use libresonic_opensubsonic as opensubsonic;
/// Single-track decoding, transport state and neutral PCM output contract.
pub use libresonic_playback as playback;
