use serde::{Deserialize, Serialize};

use crate::Error;

/// Opaque local profile namespace, independent of credentials and base URLs.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String")]
pub struct ServerId(String);

impl ServerId {
    /// Construct a nonempty local profile ID. Do not use credentials as an ID.
    pub fn new(value: impl Into<String>) -> Result<Self, Error> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(Error::InvalidIdentifier);
        }
        Ok(Self(value))
    }

    /// The local profile's opaque value.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ServerId {
    type Error = Error;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

/// Item namespace within a server, where raw IDs may overlap across kinds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MediaKind {
    /// An audio track.
    Track,
    /// An album.
    Album,
    /// An artist.
    Artist,
    /// A playlist.
    Playlist,
    /// Artwork fetched as authenticated bytes.
    Artwork,
}

/// A server- and kind-scoped identifier. It never contains an authenticated URL.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "RawMediaId")]
pub struct MediaId {
    server: ServerId,
    kind: MediaKind,
    item: String,
}

#[derive(Deserialize)]
struct RawMediaId {
    server: ServerId,
    kind: MediaKind,
    item: String,
}

impl TryFrom<RawMediaId> for MediaId {
    type Error = Error;

    fn try_from(raw: RawMediaId) -> Result<Self, Self::Error> {
        Self::new(raw.server, raw.kind, raw.item)
    }
}

impl MediaId {
    /// Construct an ID without normalizing its opaque item value.
    pub fn new(server: ServerId, kind: MediaKind, item: impl Into<String>) -> Result<Self, Error> {
        let item = item.into();
        if item.trim().is_empty() {
            return Err(Error::InvalidIdentifier);
        }
        Ok(Self { server, kind, item })
    }

    /// The server profile that owns this item.
    pub fn server(&self) -> &ServerId {
        &self.server
    }

    /// The item's entity kind.
    pub fn kind(&self) -> MediaKind {
        self.kind
    }

    /// The server's opaque item value, without credentials or URI rewriting.
    pub fn item(&self) -> &str {
        &self.item
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_includes_server_and_entity_kind() {
        let a = ServerId::new("a").unwrap();
        let b = ServerId::new("b").unwrap();
        let track = MediaId::new(a.clone(), MediaKind::Track, "1").unwrap();
        assert_ne!(track, MediaId::new(b, MediaKind::Track, "1").unwrap());
        assert_ne!(track, MediaId::new(a, MediaKind::Album, "1").unwrap());
        assert_eq!(track.item(), "1");
        assert_eq!(
            serde_json::from_str::<MediaId>(&serde_json::to_string(&track).unwrap()).unwrap(),
            track
        );
    }

    #[test]
    fn deserialization_cannot_bypass_identifier_validation() {
        for json in [
            r#"{"server":"","kind":"track","item":"1"}"#,
            r#"{"server":"a","kind":"track","item":" "}"#,
        ] {
            assert!(serde_json::from_str::<MediaId>(json).is_err());
        }
        assert!(ServerId::new(" ").is_err());
    }
}
