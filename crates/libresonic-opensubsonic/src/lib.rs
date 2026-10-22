//! OpenSubsonic adapter with bounded reads and neutral public responses.
//!
//! Credentials stay in memory; application-owned protected storage is separate.
//! Dropping a service future cancels its HTTP work. No background retries are
//! made, and authenticated URLs/server error text are never returned in errors.

mod config;
mod feedback;
mod mapping;
mod playlists;
mod transport;
mod wire;

use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};

use libresonic_core::*;

pub use config::{ClientOptions, Credentials};
use mapping::{Mapper, collection, decode, field, list};
use transport::Transport;

/// A cloneable authenticated service for one server profile.
#[derive(Clone)]
pub struct Client {
    username: Arc<Mutex<Option<String>>>,
    server: ServerId,
    transport: Transport,
    capabilities: Arc<Mutex<CapabilitySet>>,
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenSubsonicClient")
            .field("server", &self.server)
            .finish_non_exhaustive()
    }
}

impl Client {
    /// Create a service without making requests. Plain HTTP requires explicit opt-in.
    pub fn new(
        server: ServerId,
        base_url: &str,
        credentials: Credentials,
        options: ClientOptions,
    ) -> Result<Self, Error> {
        let capabilities = CapabilitySet::new(server.clone(), credentials.method());
        let username = credentials.username().map(str::to_owned);
        let transport = Transport::new(base_url, credentials, options)?;
        Ok(Self {
            username: Arc::new(Mutex::new(username)),
            server,
            transport,
            capabilities: Arc::new(Mutex::new(capabilities)),
        })
    }

    fn track<T>(&self, operation: Operation, result: Result<T, Error>) -> Result<T, Error> {
        self.capabilities
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .observe(operation, &result);
        result
    }

    fn check_id(&self, id: &MediaId, kind: MediaKind) -> Result<(), Error> {
        if id.server() != &self.server {
            return Err(Error::WrongServer);
        }
        if id.kind() != kind {
            return Err(Error::InvalidIdentifier);
        }
        Ok(())
    }
}

impl MusicService for Client {
    fn download<'a>(&'a self, id: &'a MediaId) -> ServiceFuture<'a, Box<dyn AudioStream>> {
        Box::pin(async move {
            self.check_id(id, MediaKind::Track)?;
            self.track(
                Operation::Download,
                self.transport.download(id.item()).await,
            )
        })
    }
    fn scrobble<'a>(
        &'a self,
        id: &'a MediaId,
        started_ms: u64,
        submission: bool,
    ) -> ServiceFuture<'a, ()> {
        Box::pin(async move {
            self.check_id(id, MediaKind::Track)?;
            let post = self
                .capabilities()
                .extensions
                .get("formPost")
                .is_some_and(|v| v.contains(&1));
            let result = self
                .transport
                .json_write(
                    "scrobble",
                    &[
                        ("id", id.item().to_owned()),
                        ("time", started_ms.to_string()),
                        ("submission", submission.to_string()),
                    ],
                    post,
                )
                .await
                .map(|_| ());
            self.track(Operation::Scrobble, result)
        })
    }

    fn favorite_songs(&self) -> ServiceFuture<'_, Vec<Track>> {
        self.favorite_song_list()
    }
    fn set_favorite_songs<'a>(
        &'a self,
        ids: &'a [MediaId],
        favorite: bool,
    ) -> ServiceFuture<'a, ()> {
        self.favorite_song_write(ids, favorite)
    }
    fn set_song_rating<'a>(&'a self, id: &'a MediaId, rating: u8) -> ServiceFuture<'a, ()> {
        self.song_rating_write(id, rating)
    }

    fn playlists(&self) -> ServiceFuture<'_, Vec<Playlist>> {
        self.playlist_list()
    }
    fn playlist<'a>(&'a self, id: &'a MediaId) -> ServiceFuture<'a, PlaylistDetails> {
        self.playlist_read(id)
    }
    fn create_playlist<'a>(
        &'a self,
        name: &'a str,
        songs: &'a [MediaId],
    ) -> ServiceFuture<'a, PlaylistDetails> {
        self.playlist_create(name, songs)
    }
    fn update_playlist<'a>(
        &'a self,
        id: &'a MediaId,
        changes: &'a PlaylistUpdate,
    ) -> ServiceFuture<'a, ()> {
        self.playlist_update(id, changes)
    }
    fn replace_playlist<'a>(
        &'a self,
        id: &'a MediaId,
        songs: &'a [MediaId],
    ) -> ServiceFuture<'a, PlaylistDetails> {
        self.playlist_replace(id, songs)
    }
    fn delete_playlist<'a>(&'a self, id: &'a MediaId) -> ServiceFuture<'a, ()> {
        self.playlist_delete(id)
    }
    fn capabilities(&self) -> CapabilitySet {
        self.capabilities
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn connect(&self) -> ServiceFuture<'_, CapabilitySet> {
        Box::pin(async move {
            let ping = self.track(
                Operation::Connection,
                self.transport.json("ping", &[]).await,
            )?;
            self.capabilities
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .info = ServerInfo {
                name: ping.server_type,
                version: ping.server_version,
                protocol_version: ping.version,
            };
            let extensions = async {
                let mut env = self
                    .transport
                    .json("getOpenSubsonicExtensions", &[])
                    .await?;
                let (extensions, _) =
                    list::<wire::OpenSubsonicExtension>(env.data.remove("openSubsonicExtensions"))?;
                Ok(extensions)
            }
            .await;
            match extensions {
                Ok(extensions) => {
                    let mut caps = self
                        .capabilities
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner);
                    caps.extensions.clear();
                    for extension in extensions {
                        let versions = caps.extensions.entry(extension.name).or_default();
                        for version in extension.versions {
                            if let Ok(version) = u32::try_from(version) {
                                versions.push(version);
                            }
                        }
                        versions.sort_unstable();
                        versions.dedup();
                    }
                    caps.observe(Operation::Extensions, &Ok::<_, Error>(()));
                }
                Err(Error::NotFound | Error::Unsupported) => {
                    let mut caps = self
                        .capabilities
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner);
                    caps.extensions.clear();
                    caps.observe::<()>(Operation::Extensions, &Err(Error::Unsupported));
                }
                Err(error) => {
                    return self.track(Operation::Extensions, Err(error));
                }
            }
            if self.capabilities().authentication == AuthenticationMethod::ApiKey
                && self
                    .capabilities()
                    .extensions
                    .get("apiKeyAuthentication")
                    .is_some_and(|v| v.contains(&1))
            {
                *self.username.lock().unwrap_or_else(PoisonError::into_inner) = None;
                let result = async {
                    let mut env = self.transport.json("tokenInfo", &[]).await?;
                    let info = field(&mut env.data, "tokenInfo")?;
                    info.get("username")
                        .and_then(serde_json::Value::as_str)
                        .filter(|name| !name.trim().is_empty() && name.len() <= 4096)
                        .map(str::to_owned)
                        .ok_or(Error::InvalidResponse)
                }
                .await;
                match self.track(Operation::AccountIdentity, result) {
                    Ok(name) => {
                        *self.username.lock().unwrap_or_else(PoisonError::into_inner) = Some(name)
                    }
                    Err(Error::Authentication) => return Err(Error::Authentication),
                    Err(_) => {} // Browsing remains usable with unknown ownership.
                }
            }
            Ok(self.capabilities())
        })
    }

    fn artists(&self) -> ServiceFuture<'_, Vec<Artist>> {
        Box::pin(async move {
            let result = async {
                let mut env = self.transport.json("getArtists", &[]).await?;
                let value = collection(&mut env.data, "artists")?;
                let artists: wire::ArtistsId3 = decode(value)?;
                artists
                    .index
                    .into_iter()
                    .flat_map(|index| index.artist)
                    .map(|a| Mapper(&self.server).artist(a))
                    .collect()
            }
            .await;
            self.track(Operation::Artists, result)
        })
    }

    fn artist<'a>(&'a self, id: &'a MediaId) -> ServiceFuture<'a, ArtistDetails> {
        Box::pin(async move {
            self.check_id(id, MediaKind::Artist)?;
            let result = async {
                let mut env = self
                    .transport
                    .json("getArtist", &[("id", id.item().to_owned())])
                    .await?;
                let value = field(&mut env.data, "artist")?;
                Mapper(&self.server).artist_details(decode(value)?)
            }
            .await;
            self.track(Operation::Artist, result)
        })
    }

    fn albums(&self, order: AlbumOrder, page: PageRequest) -> ServiceFuture<'_, Page<Album>> {
        Box::pin(async move {
            let result = async {
                let order = match order {
                    AlbumOrder::Title => "alphabeticalByName",
                    AlbumOrder::Artist => "alphabeticalByArtist",
                    AlbumOrder::Newest => "newest",
                    AlbumOrder::Recent => "recent",
                    AlbumOrder::Random => "random",
                    AlbumOrder::Favorites => "starred",
                };
                let mut env = self
                    .transport
                    .json(
                        "getAlbumList2",
                        &[
                            ("type", order.into()),
                            ("size", page.size().to_string()),
                            ("offset", page.offset().to_string()),
                        ],
                    )
                    .await?;
                let mut value = collection(&mut env.data, "albumList2")?;
                let value = value.as_object_mut().ok_or(Error::InvalidResponse)?;
                let (albums, count) = list::<wire::AlbumId3>(value.remove("album"))?;
                let next = page.next(count)?;
                let next = if order == "random" { None } else { next };
                Ok(Page {
                    items: albums
                        .into_iter()
                        .map(|a| Mapper(&self.server).album(a))
                        .collect::<Result<_, _>>()?,
                    next,
                })
            }
            .await;
            self.track(Operation::Albums, result)
        })
    }

    fn album<'a>(&'a self, id: &'a MediaId) -> ServiceFuture<'a, AlbumDetails> {
        Box::pin(async move {
            self.check_id(id, MediaKind::Album)?;
            let result = async {
                let mut env = self
                    .transport
                    .json("getAlbum", &[("id", id.item().to_owned())])
                    .await?;
                Mapper(&self.server).album_details(field(&mut env.data, "album")?)
            }
            .await;
            self.track(Operation::Album, result)
        })
    }

    fn song<'a>(&'a self, id: &'a MediaId) -> ServiceFuture<'a, Track> {
        Box::pin(async move {
            self.check_id(id, MediaKind::Track)?;
            let result = async {
                let mut env = self
                    .transport
                    .json("getSong", &[("id", id.item().to_owned())])
                    .await?;
                Mapper(&self.server).track(decode(field(&mut env.data, "song")?)?)
            }
            .await;
            self.track(Operation::Song, result)
        })
    }

    fn search<'a>(&'a self, query: &'a str, page: PageRequest) -> ServiceFuture<'a, SearchResults> {
        Box::pin(async move {
            if query.len() > 4096 {
                return Err(Error::InvalidConfiguration(
                    "search query exceeds 4096 bytes",
                ));
            }
            let result = async {
                let mut env = self
                    .transport
                    .json(
                        "search3",
                        &[
                            ("query", query.into()),
                            ("artistCount", page.size().to_string()),
                            ("albumCount", page.size().to_string()),
                            ("songCount", page.size().to_string()),
                            ("artistOffset", page.offset().to_string()),
                            ("albumOffset", page.offset().to_string()),
                            ("songOffset", page.offset().to_string()),
                        ],
                    )
                    .await?;
                let mut value = collection(&mut env.data, "searchResult3")?;
                let value = value.as_object_mut().ok_or(Error::InvalidResponse)?;
                let (artists, ac) = list::<wire::ArtistId3>(value.remove("artist"))?;
                let (albums, bc) = list::<wire::AlbumId3>(value.remove("album"))?;
                let (tracks, tc) = list::<wire::Child>(value.remove("song"))?;
                let mapper = Mapper(&self.server);
                Ok(SearchResults {
                    artists: Page {
                        items: artists
                            .into_iter()
                            .map(|a| mapper.artist(a))
                            .collect::<Result<_, _>>()?,
                        next: page.next(ac)?,
                    },
                    albums: Page {
                        items: albums
                            .into_iter()
                            .map(|a| mapper.album(a))
                            .collect::<Result<_, _>>()?,
                        next: page.next(bc)?,
                    },
                    tracks: Page {
                        items: tracks
                            .into_iter()
                            .map(|t| mapper.track(t))
                            .collect::<Result<_, _>>()?,
                        next: page.next(tc)?,
                    },
                })
            }
            .await;
            self.track(Operation::Search, result)
        })
    }

    fn artwork<'a>(&'a self, id: &'a MediaId, size: Option<u16>) -> ServiceFuture<'a, Vec<u8>> {
        Box::pin(async move {
            self.check_id(id, MediaKind::Artwork)?;
            if size == Some(0) {
                return Err(Error::InvalidConfiguration("artwork size must be nonzero"));
            }
            let mut params = vec![("id", id.item().to_owned())];
            if let Some(size) = size {
                params.push(("size", size.to_string()));
            }
            self.track(Operation::Artwork, self.transport.artwork(&params).await)
        })
    }
}

impl StreamProvider for Client {
    fn open<'a>(&'a self, id: &'a MediaId) -> ServiceFuture<'a, Box<dyn AudioStream>> {
        Box::pin(async move {
            self.check_id(id, MediaKind::Track)?;
            self.track(Operation::Stream, self.transport.audio(id.item()).await)
        })
    }
}
