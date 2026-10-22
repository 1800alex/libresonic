//! Private DTOs for the Phase 1 endpoint subset. No wire types are public API.

use serde::{Deserialize, Deserializer};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Playlist {
    pub id: String,
    pub name: Option<String>,
    pub owner: Option<String>,
    pub comment: Option<String>,
    pub public: Option<bool>,
    pub song_count: Option<i64>,
    pub cover_art: Option<String>,
    pub changed: Option<String>,
    pub readonly: Option<bool>,
    #[serde(default)]
    pub entry: Option<Vec<Option<Child>>>,
}

fn nullable_list<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    let values = Option::<Vec<Option<T>>>::deserialize(deserializer)?;
    Ok(values.unwrap_or_default().into_iter().flatten().collect())
}

#[derive(Deserialize)]
pub(crate) struct OpenSubsonicExtension {
    pub name: String,
    pub versions: Vec<i32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ArtistId3 {
    pub id: String,
    pub name: Option<String>,
    pub cover_art: Option<String>,
    pub album_count: Option<i64>,
}

#[derive(Deserialize)]
pub(crate) struct ArtistWithAlbumsId3 {
    #[serde(flatten)]
    pub artist: ArtistId3,
    #[serde(default, deserialize_with = "nullable_list")]
    pub album: Vec<AlbumId3>,
}

#[derive(Deserialize)]
pub(crate) struct ArtistsId3 {
    #[serde(default, deserialize_with = "nullable_list")]
    pub index: Vec<IndexId3>,
}

#[derive(Deserialize)]
pub(crate) struct IndexId3 {
    #[serde(default, deserialize_with = "nullable_list")]
    pub artist: Vec<ArtistId3>,
}

#[derive(Deserialize)]
pub(crate) struct ArtistCredit {
    pub id: Option<String>,
    pub name: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AlbumId3 {
    pub id: String,
    pub name: Option<String>,
    pub artist: Option<String>,
    pub artist_id: Option<String>,
    #[serde(default, deserialize_with = "nullable_list")]
    pub artists: Vec<ArtistCredit>,
    pub cover_art: Option<String>,
    pub song_count: Option<i64>,
    pub duration: Option<i64>,
    pub year: Option<i32>,
    pub starred: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Child {
    pub id: String,
    #[serde(default)]
    pub is_dir: bool,
    pub title: Option<String>,
    pub album: Option<String>,
    pub album_id: Option<String>,
    pub artist: Option<String>,
    pub artist_id: Option<String>,
    #[serde(default, deserialize_with = "nullable_list")]
    pub artists: Vec<ArtistCredit>,
    pub cover_art: Option<String>,
    pub duration: Option<i64>,
    pub track: Option<i32>,
    pub disc_number: Option<i32>,
    pub content_type: Option<String>,
    pub suffix: Option<String>,
    pub starred: Option<String>,
    pub user_rating: Option<i64>,
    pub path: Option<String>,
    pub size: Option<i64>,
    pub play_count: Option<i64>,
}
