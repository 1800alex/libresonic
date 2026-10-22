use crate::wire;
use libresonic_core::*;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

pub(crate) struct Mapper<'a>(pub &'a ServerId);

impl Mapper<'_> {
    pub fn id(&self, kind: MediaKind, raw: String) -> Result<MediaId, Error> {
        MediaId::new(self.0.clone(), kind, raw).map_err(|_| Error::InvalidResponse)
    }

    fn optional_id(&self, kind: MediaKind, raw: Option<String>) -> Result<Option<MediaId>, Error> {
        raw.filter(|v| !v.trim().is_empty())
            .map(|v| self.id(kind, v))
            .transpose()
    }

    fn artists(
        &self,
        expanded: Vec<wire::ArtistCredit>,
        id: Option<String>,
        name: Option<String>,
    ) -> Result<Vec<ArtistRef>, Error> {
        if !expanded.is_empty() {
            return expanded
                .into_iter()
                .map(|artist| {
                    Ok(ArtistRef {
                        id: self.optional_id(MediaKind::Artist, artist.id)?,
                        name: display(artist.name, "Unknown artist"),
                    })
                })
                .collect();
        }
        let id = self.optional_id(MediaKind::Artist, id)?;
        if id.is_none() && name.as_deref().is_none_or(|s| s.trim().is_empty()) {
            return Ok(Vec::new());
        }
        Ok(vec![ArtistRef {
            id,
            name: display(name, "Unknown artist"),
        }])
    }

    pub fn artist(&self, artist: wire::ArtistId3) -> Result<Artist, Error> {
        Ok(Artist {
            id: self.id(MediaKind::Artist, artist.id)?,
            name: display(artist.name, "Unknown artist"),
            artwork: self.optional_id(MediaKind::Artwork, artist.cover_art)?,
            album_count: nonnegative(artist.album_count),
        })
    }

    pub fn artist_details(
        &self,
        artist: wire::ArtistWithAlbumsId3,
    ) -> Result<ArtistDetails, Error> {
        Ok(ArtistDetails {
            artist: self.artist(artist.artist)?,
            albums: artist
                .album
                .into_iter()
                .map(|album| self.album(album))
                .collect::<Result<_, _>>()?,
        })
    }

    pub fn album(&self, album: wire::AlbumId3) -> Result<Album, Error> {
        Ok(Album {
            id: self.id(MediaKind::Album, album.id)?,
            title: display(album.name, "Untitled album"),
            artists: self.artists(album.artists, album.artist_id, album.artist)?,
            artwork: self.optional_id(MediaKind::Artwork, album.cover_art)?,
            duration_ms: milliseconds(album.duration),
            track_count: nonnegative(album.song_count),
            year: positive_u32(album.year),
            favorite: album.starred.is_some_and(|s| !s.is_empty()),
        })
    }

    pub fn album_details(&self, mut value: Value) -> Result<AlbumDetails, Error> {
        // Read the common album summary through the private wire model. Its
        // extra fields are ignored, so the same mapping serves list/detail paths.
        let songs = value.as_object_mut().and_then(|v| v.remove("song"));
        let album = self.album(decode(value)?)?;
        let (songs, _) = list::<wire::Child>(songs)?;
        let tracks = songs
            .into_iter()
            .map(|song| self.track(song))
            .collect::<Result<_, _>>()?;
        Ok(AlbumDetails { album, tracks })
    }

    pub fn track(&self, track: wire::Child) -> Result<Track, Error> {
        if track.is_dir {
            return Err(Error::InvalidResponse);
        }
        let album_id = self.optional_id(MediaKind::Album, track.album_id)?;
        let album = if album_id.is_some() || track.album.is_some() {
            Some(AlbumRef {
                id: album_id,
                title: display(track.album, "Untitled album"),
            })
        } else {
            None
        };
        Ok(Track {
            id: self.id(MediaKind::Track, track.id)?,
            title: display(track.title, "Untitled track"),
            artists: self.artists(track.artists, track.artist_id, track.artist)?,
            album,
            duration_ms: milliseconds(track.duration),
            track_number: positive_u32(track.track),
            disc_number: positive_u32(track.disc_number),
            content_type: track.content_type,
            suffix: track.suffix,
            artwork: self.optional_id(MediaKind::Artwork, track.cover_art)?,
            file_path: track.path,
            file_size: nonnegative(track.size),
            play_count: nonnegative(track.play_count),
            favorite: track.starred.is_some_and(|s| !s.is_empty()),
            rating: track
                .user_rating
                .and_then(|n| u8::try_from(n).ok())
                .filter(|n| (1..=5).contains(n)),
        })
    }
}

pub(crate) fn field(data: &mut Map<String, Value>, key: &str) -> Result<Value, Error> {
    data.remove(key)
        .filter(|v| !v.is_null())
        .ok_or(Error::InvalidResponse)
}

pub(crate) fn collection(data: &mut Map<String, Value>, key: &str) -> Result<Value, Error> {
    match data.remove(key) {
        None | Some(Value::Null) => Ok(Value::Object(Map::new())),
        Some(value @ Value::Object(_)) => Ok(value),
        _ => Err(Error::InvalidResponse),
    }
}

pub(crate) fn decode<T: DeserializeOwned>(value: Value) -> Result<T, Error> {
    serde_json::from_value(value).map_err(|_| Error::InvalidResponse)
}

pub(crate) fn list<T: DeserializeOwned>(value: Option<Value>) -> Result<(Vec<T>, usize), Error> {
    let values = match value {
        None | Some(Value::Null) => return Ok((Vec::new(), 0)),
        Some(Value::Array(values)) => values,
        _ => return Err(Error::InvalidResponse),
    };
    let raw_count = values.len();
    let items = values
        .into_iter()
        .filter(|v| !v.is_null())
        .map(decode)
        .collect::<Result<_, _>>()?;
    Ok((items, raw_count))
}

fn display(value: Option<String>, fallback: &str) -> String {
    let value = value.unwrap_or_default();
    if value.trim().is_empty() {
        fallback.to_owned()
    } else {
        value
    }
}

fn nonnegative(value: Option<i64>) -> Option<u64> {
    value.and_then(|v| u64::try_from(v).ok())
}
fn milliseconds(value: Option<i64>) -> Option<u64> {
    nonnegative(value)?.checked_mul(1000)
}
fn positive_u32(value: Option<i32>) -> Option<u32> {
    value.and_then(|v| u32::try_from(v).ok()).filter(|v| *v > 0)
}
