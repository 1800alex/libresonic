use std::future::Future;
use std::pin::Pin;

use crate::*;

/// An object-safe asynchronous service request. Dropping it cancels its work.
pub type ServiceFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, Error>> + Send + 'a>>;

/// Server-independent browsing. All network work occurs while awaiting futures.
pub trait MusicService: Send + Sync {
    /// Authenticate health and discover optional extensions without downgrading credentials.
    fn connect(&self) -> ServiceFuture<'_, CapabilitySet>;
    /// Current capability evidence, without network or disk access.
    fn capabilities(&self) -> CapabilitySet;
    /// List library artists.
    fn artists(&self) -> ServiceFuture<'_, Vec<Artist>>;
    /// Read an artist and its albums.
    fn artist<'a>(&'a self, id: &'a MediaId) -> ServiceFuture<'a, ArtistDetails>;
    /// Fetch a bounded album page.
    fn albums(&self, order: AlbumOrder, page: PageRequest) -> ServiceFuture<'_, Page<Album>>;
    /// Read album metadata and ordered tracks.
    fn album<'a>(&'a self, id: &'a MediaId) -> ServiceFuture<'a, AlbumDetails>;
    /// Read one library track.
    fn song<'a>(&'a self, id: &'a MediaId) -> ServiceFuture<'a, Track>;
    /// Search each category with the same offset/size, returning separate continuation hints.
    fn search<'a>(&'a self, query: &'a str, page: PageRequest) -> ServiceFuture<'a, SearchResults>;
    /// Fetch bounded authenticated artwork bytes, optionally requesting a pixel size.
    fn artwork<'a>(&'a self, id: &'a MediaId, size: Option<u16>) -> ServiceFuture<'a, Vec<u8>>;
    /// Open original, untranscoded bytes for an explicit download.
    fn download<'a>(&'a self, _id: &'a MediaId) -> ServiceFuture<'a, Box<dyn AudioStream>> {
        Box::pin(async { Err(Error::Unsupported) })
    }
    /// Report now playing or a completed listen. No automatic retry: servers
    /// do not guarantee idempotency after an ambiguous transport failure.
    fn scrobble<'a>(
        &'a self,
        _id: &'a MediaId,
        _started_ms: u64,
        _submission: bool,
    ) -> ServiceFuture<'a, ()> {
        Box::pin(async { Err(Error::Unsupported) })
    }
    /// Read all favorite songs, bounded by the adapter's response budget.
    fn favorite_songs(&self) -> ServiceFuture<'_, Vec<Track>> {
        Box::pin(async { Err(Error::Unsupported) })
    }
    /// Set song hearts in one bounded, non-retried request.
    fn set_favorite_songs<'a>(
        &'a self,
        _ids: &'a [MediaId],
        _favorite: bool,
    ) -> ServiceFuture<'a, ()> {
        Box::pin(async { Err(Error::Unsupported) })
    }
    /// Set a personal song rating, 1 through 5; zero clears the rating.
    fn set_song_rating<'a>(&'a self, _id: &'a MediaId, _rating: u8) -> ServiceFuture<'a, ()> {
        Box::pin(async { Err(Error::Unsupported) })
    }
    /// List playlists the authenticated account can read.
    fn playlists(&self) -> ServiceFuture<'_, Vec<Playlist>> {
        Box::pin(async { Err(Error::Unsupported) })
    }
    /// Read the complete ordered playlist, including unavailable row slots.
    fn playlist<'a>(&'a self, _id: &'a MediaId) -> ServiceFuture<'a, PlaylistDetails> {
        Box::pin(async { Err(Error::Unsupported) })
    }
    /// Create a playlist, optionally with ordered duplicate song IDs.
    /// Set visibility separately when the server's default is unsuitable.
    fn create_playlist<'a>(
        &'a self,
        _name: &'a str,
        _songs: &'a [MediaId],
    ) -> ServiceFuture<'a, PlaylistDetails> {
        Box::pin(async { Err(Error::Unsupported) })
    }
    /// Update metadata, append songs or remove occurrences. No automatic retry.
    fn update_playlist<'a>(
        &'a self,
        _id: &'a MediaId,
        _changes: &'a PlaylistUpdate,
    ) -> ServiceFuture<'a, ()> {
        Box::pin(async { Err(Error::Unsupported) })
    }
    /// Replace ordered contents in one request, preserving duplicate occurrences.
    fn replace_playlist<'a>(
        &'a self,
        _id: &'a MediaId,
        _songs: &'a [MediaId],
    ) -> ServiceFuture<'a, PlaylistDetails> {
        Box::pin(async { Err(Error::Unsupported) })
    }
    /// Permanently delete a playlist. Server permission checks are authoritative.
    fn delete_playlist<'a>(&'a self, _id: &'a MediaId) -> ServiceFuture<'a, ()> {
        Box::pin(async { Err(Error::Unsupported) })
    }
}
