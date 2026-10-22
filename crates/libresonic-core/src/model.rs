use serde::{Deserialize, Serialize};

use crate::{Error, MediaId};

/// Artist attribution, including display-only names without an artist ID.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtistRef {
    /// Resolvable artist identity, when supplied.
    pub id: Option<MediaId>,
    /// Display name.
    pub name: String,
}

/// A library artist.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artist {
    /// Scoped artist identity.
    pub id: MediaId,
    /// Display name with a fallback for missing metadata.
    pub name: String,
    /// Scoped cover identity; never a public or authenticated URL.
    pub artwork: Option<MediaId>,
    /// Number of albums when known.
    pub album_count: Option<u64>,
}

/// A library album.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Album {
    /// Scoped album identity.
    pub id: MediaId,
    /// Album title.
    pub title: String,
    /// Artist attributions; empty when unknown.
    pub artists: Vec<ArtistRef>,
    /// Scoped cover identity.
    pub artwork: Option<MediaId>,
    /// Duration in milliseconds, if known.
    pub duration_ms: Option<u64>,
    /// Track count, if known.
    pub track_count: Option<u64>,
    /// Release year, if known.
    pub year: Option<u32>,
    /// Whether the server marked the album as a favorite.
    pub favorite: bool,
}

/// A track's album attribution.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlbumRef {
    /// Album identity, if resolvable.
    pub id: Option<MediaId>,
    /// Album title.
    pub title: String,
}

/// A library track without protocol or player-engine fields.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Track {
    /// Scoped track identity.
    pub id: MediaId,
    /// Display title.
    pub title: String,
    /// Artist attributions; empty when unknown.
    pub artists: Vec<ArtistRef>,
    /// Album attribution when supplied.
    pub album: Option<AlbumRef>,
    /// Duration in milliseconds, if known.
    pub duration_ms: Option<u64>,
    /// Track number when known.
    pub track_number: Option<u32>,
    /// Disc number when known.
    pub disc_number: Option<u32>,
    /// Declared MIME type, not a guarantee of decoder support.
    pub content_type: Option<String>,
    /// Declared filename suffix, without an audio URL.
    pub suffix: Option<String>,
    /// Scoped cover identity.
    pub artwork: Option<MediaId>,
    /// Whether the server marked the track as a favorite.
    pub favorite: bool,
    /// Personal rating from 1 through 5; absent means unrated/unknown.
    #[serde(default)]
    pub rating: Option<u8>,
    /// Server-provided path (possibly virtual), never a trusted local absolute path.
    #[serde(default)]
    pub file_path: Option<String>,
    /// Original encoded byte length, when supplied.
    #[serde(default)]
    pub file_size: Option<u64>,
    /// Server play count, when supplied.
    #[serde(default)]
    pub play_count: Option<u64>,
}

/// Album metadata with tracks in server order, including repeated occurrences.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AlbumDetails {
    /// Album metadata.
    pub album: Album,
    /// Album tracks in server order.
    pub tracks: Vec<Track>,
}

/// Artist metadata with its albums.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtistDetails {
    /// Artist metadata.
    pub artist: Artist,
    /// Albums supplied by the artist endpoint.
    pub albums: Vec<Album>,
}

/// A playlist summary, with no assumed editing capability.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Playlist {
    /// Scoped playlist identity.
    pub id: MediaId,
    /// Playlist title.
    pub title: String,
    /// Number of tracks when supplied.
    pub track_count: Option<u64>,
    /// Scoped cover identity.
    pub artwork: Option<MediaId>,
    /// Editing availability; None means it has not been established.
    pub editable: Option<bool>,
    /// Whether the authenticated identity is known to own this playlist.
    pub owned: Option<bool>,
    /// Owner's server account name, when supplied.
    pub owner: Option<String>,
    /// Server playlist comment.
    pub description: Option<String>,
    /// Visibility to other server users, when supplied.
    pub public: Option<bool>,
}

/// Complete ordered playlist, preserving unavailable/null entry positions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaylistDetails {
    /// Playlist metadata.
    pub playlist: Playlist,
    /// Ordered occurrences; None preserves an unavailable server row.
    pub entries: Vec<Option<Track>>,
    /// Client-computed content fingerprint, not a server transaction token.
    pub revision: String,
}

/// One playlist update; removals refer to the original zero-based positions.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PlaylistUpdate {
    /// New title, or leave unchanged.
    pub name: Option<String>,
    /// New comment; an empty string clears it.
    pub description: Option<String>,
    /// New visibility, or leave unchanged.
    pub public: Option<bool>,
    /// Append these song occurrences, including duplicates.
    pub add: Vec<MediaId>,
    /// Remove exactly these positions, without collapsing duplicate identities.
    pub remove: Vec<u32>,
}

/// Bounded offset pagination with no invented server total.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageRequest {
    offset: u32,
    size: u16,
}

impl PageRequest {
    /// Construct a page of 1 through 500 items.
    pub fn new(offset: u32, size: u16) -> Result<Self, Error> {
        if !(1..=500).contains(&size) {
            return Err(Error::InvalidConfiguration(
                "page size must be between 1 and 500",
            ));
        }
        Ok(Self { offset, size })
    }

    /// Requested starting offset.
    pub fn offset(self) -> u32 {
        self.offset
    }

    /// Requested maximum number of items.
    pub fn size(self) -> u16 {
        self.size
    }

    /// Next page after the raw response count, before dropping optional null rows.
    pub fn next(self, returned: usize) -> Result<Option<Self>, Error> {
        if returned > usize::from(self.size) {
            return Err(Error::InvalidResponse);
        }
        if returned < usize::from(self.size) {
            return Ok(None);
        }
        let offset = self
            .offset
            .checked_add(u32::from(self.size))
            .ok_or(Error::InvalidResponse)?;
        Ok(Some(Self { offset, ..self }))
    }
}

/// One bounded response page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Page<T> {
    /// Items in server order.
    pub items: Vec<T>,
    /// Next page hint; a full last page may require one final empty request.
    pub next: Option<PageRequest>,
}

/// Album browse ordering with no server-specific recommendations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlbumOrder {
    /// Alphabetical album title.
    Title,
    /// Alphabetical artist.
    Artist,
    /// Recently added.
    Newest,
    /// Recently played.
    Recent,
    /// Random selection; pagination cannot promise stable random order.
    Random,
    /// Favorites.
    Favorites,
}

/// Search categories with independent continuation hints.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchResults {
    /// Matching artists.
    pub artists: Page<Artist>,
    /// Matching albums.
    pub albums: Page<Album>,
    /// Matching tracks.
    pub tracks: Page<Track>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paging_uses_raw_counts_and_checks_overflow() {
        let page = PageRequest::new(10, 2).unwrap();
        assert_eq!(page.next(2).unwrap().unwrap().offset(), 12);
        assert_eq!(page.next(1).unwrap(), None);
        assert_eq!(page.next(3), Err(Error::InvalidResponse));
        assert_eq!(
            PageRequest::new(u32::MAX, 1).unwrap().next(1),
            Err(Error::InvalidResponse)
        );
        assert!(PageRequest::new(0, 0).is_err());
        assert!(PageRequest::new(0, 501).is_err());
    }
}
