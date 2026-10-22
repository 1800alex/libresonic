use std::collections::BTreeMap;

use crate::{Error, ServerId};

/// Authentication method selected by the caller, never inferred by downgrade.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthenticationMethod {
    /// Username/password salt-token requests.
    Token,
    /// Server-issued API key.
    ApiKey,
}

/// Server software identity from an authenticated response.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServerInfo {
    /// Implementation name, if advertised.
    pub name: Option<String>,
    /// Implementation version, if advertised.
    pub version: Option<String>,
    /// Reported protocol version, if advertised.
    pub protocol_version: Option<String>,
}

/// Service operations with independently observed availability.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Operation {
    /// Authenticated health check.
    Connection,
    /// Optional extension discovery.
    Extensions,
    /// Optional verified API-key account identity.
    AccountIdentity,
    /// List artists.
    Artists,
    /// Read one artist with its albums.
    Artist,
    /// Browse album pages.
    Albums,
    /// Read an album with tracks.
    Album,
    /// Search library metadata.
    Search,
    /// Read a single track.
    Song,
    /// Fetch artwork bytes.
    Artwork,
    /// Open a direct-play encoded audio stream.
    Stream,
    /// Download original bytes.
    Download,
    /// Report listening.
    Scrobble,
    /// Read favorite songs.
    FavoriteSongs,
    /// Set song hearts.
    SetFavoriteSongs,
    /// Set a personal song rating.
    SetSongRating,
    /// List readable playlists.
    Playlists,
    /// Read playlist metadata and ordered contents.
    Playlist,
    /// Create a playlist.
    CreatePlaylist,
    /// Edit playlist metadata or occurrences.
    UpdatePlaylist,
    /// Replace ordered playlist contents.
    ReplacePlaylist,
    /// Delete a playlist.
    DeletePlaylist,
}

/// Evidence of operation support, independent of transient request failures.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Availability {
    /// No successful or explicitly unsupported request has established support.
    #[default]
    Unknown,
    /// At least one request succeeded.
    Available,
    /// The server explicitly reported an unsupported operation.
    Unavailable,
}

/// Last safe outcome and established support for an operation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OperationState {
    /// Observed availability, not a promise about permissions or future uptime.
    pub availability: Availability,
    /// Last failure; successful requests clear this without leaking raw errors.
    pub last_error: Option<Error>,
}

/// Capability snapshot owned by the service, with no protocol DTOs or secrets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapabilitySet {
    /// Local server profile namespace.
    pub server: ServerId,
    /// The configured authentication mechanism.
    pub authentication: AuthenticationMethod,
    /// Server identity learned from ping.
    pub info: ServerInfo,
    /// Advertised extension names and integer versions. Absence does not disable core operations.
    pub extensions: BTreeMap<String, Vec<u32>>,
    /// Observed support and runtime errors by operation.
    pub operations: BTreeMap<Operation, OperationState>,
}

impl CapabilitySet {
    /// Start with unknown support; connecting does not assume every endpoint exists.
    pub fn new(server: ServerId, authentication: AuthenticationMethod) -> Self {
        Self {
            server,
            authentication,
            info: ServerInfo::default(),
            extensions: BTreeMap::new(),
            operations: BTreeMap::new(),
        }
    }

    /// Current evidence for an operation; unobserved operations remain unknown.
    pub fn operation(&self, operation: Operation) -> OperationState {
        self.operations.get(&operation).cloned().unwrap_or_default()
    }

    /// Record a safe request result, preserving support across transient failures.
    pub fn observe<T>(&mut self, operation: Operation, result: &Result<T, Error>) {
        let state = self.operations.entry(operation).or_default();
        match result {
            Ok(_) => {
                state.availability = Availability::Available;
                state.last_error = None;
            }
            Err(error) => {
                if *error == Error::Unsupported {
                    state.availability = Availability::Unavailable;
                }
                state.last_error = Some(error.clone());
            }
        }
    }
}
