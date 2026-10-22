//! Playlist adaptation and bounded, non-retried writes.
use super::*;
use crate::wire;
use md5::{Digest, Md5};

impl Client {
    fn playlist_summary(&self, value: wire::Playlist) -> Result<Playlist, Error> {
        let username = self
            .username
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let owned = username
            .as_deref()
            .zip(value.owner.as_deref())
            .map(|(user, owner)| user == owner);
        Ok(Playlist {
            id: Mapper(&self.server).id(MediaKind::Playlist, value.id)?,
            title: value
                .name
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| "Untitled playlist".into()),
            track_count: value.song_count.and_then(|n| u64::try_from(n).ok()),
            artwork: value
                .cover_art
                .filter(|s| !s.is_empty())
                .map(|s| Mapper(&self.server).id(MediaKind::Artwork, s))
                .transpose()?,
            editable: value.readonly.map(|v| !v).or(owned),
            owned,
            owner: value.owner,
            description: value.comment,
            public: value.public,
        })
    }
    fn playlist_details(&self, mut value: wire::Playlist) -> Result<PlaylistDetails, Error> {
        if value.entry.is_none() && value.song_count.is_some_and(|n| n > 0) {
            return Err(Error::InvalidResponse);
        }
        let changed = value.changed.take();
        let entries = value
            .entry
            .take()
            .unwrap_or_default()
            .into_iter()
            .map(|entry| {
                entry
                    .filter(|t| !t.is_dir)
                    .map(|t| Mapper(&self.server).track(t))
                    .transpose()
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut playlist = self.playlist_summary(value)?;
        playlist.track_count = Some(entries.len() as u64);
        let bytes = serde_json::to_vec(&(&playlist, &entries, changed))
            .map_err(|_| Error::InvalidResponse)?;
        Ok(PlaylistDetails {
            playlist,
            entries,
            revision: format!("{:x}", Md5::digest(bytes)),
        })
    }
    fn playlist_songs(&self, songs: &[MediaId]) -> Result<(), Error> {
        if songs.len() > 2000 {
            return Err(Error::InvalidConfiguration(
                "playlist writes are limited to 2000 songs",
            ));
        }
        let bytes = songs
            .iter()
            .try_fold(0usize, |total, id| total.checked_add(id.item().len()));
        if bytes.is_none_or(|n| n > 1024 * 1024) {
            return Err(Error::InvalidConfiguration(
                "playlist song identities exceed 1 MiB",
            ));
        }
        for id in songs {
            self.check_id(id, MediaKind::Track)?;
        }
        Ok(())
    }
    fn playlist_text(name: Option<&str>, description: Option<&str>) -> Result<(), Error> {
        if name.is_some_and(|s| s.trim().is_empty() || s.len() > 1024)
            || description.is_some_and(|s| s.len() > 16 * 1024)
        {
            return Err(Error::InvalidConfiguration(
                "playlist name/comment is empty or exceeds its limit",
            ));
        }
        Ok(())
    }
    async fn playlist_write(
        &self,
        endpoint: &'static str,
        params: &[(&str, String)],
    ) -> Result<transport::Envelope, Error> {
        let post = self
            .capabilities()
            .extensions
            .get("formPost")
            .is_some_and(|v| v.contains(&1));
        self.transport.json_write(endpoint, params, post).await
    }
    pub(super) fn playlist_list(&self) -> ServiceFuture<'_, Vec<Playlist>> {
        Box::pin(async move {
            let result = async {
                let mut env = self.transport.json("getPlaylists", &[]).await?;
                let mut value = collection(&mut env.data, "playlists")?;
                let value = value.as_object_mut().ok_or(Error::InvalidResponse)?;
                let (items, _) = list::<wire::Playlist>(value.remove("playlist"))?;
                items
                    .into_iter()
                    .map(|p| self.playlist_summary(p))
                    .collect()
            }
            .await;
            self.track(Operation::Playlists, result)
        })
    }
    pub(super) fn playlist_read<'a>(
        &'a self,
        id: &'a MediaId,
    ) -> ServiceFuture<'a, PlaylistDetails> {
        Box::pin(async move {
            self.check_id(id, MediaKind::Playlist)?;
            let result = async {
                let mut env = self
                    .transport
                    .json("getPlaylist", &[("id", id.item().to_owned())])
                    .await?;
                let details = self.playlist_details(decode(field(&mut env.data, "playlist")?)?)?;
                if details.playlist.id != *id {
                    return Err(Error::InvalidResponse);
                }
                Ok(details)
            }
            .await;
            self.track(Operation::Playlist, result)
        })
    }
    pub(super) fn playlist_create<'a>(
        &'a self,
        name: &'a str,
        songs: &'a [MediaId],
    ) -> ServiceFuture<'a, PlaylistDetails> {
        Box::pin(async move {
            Self::playlist_text(Some(name), None)?;
            self.playlist_songs(songs)?;
            let result = async {
                let mut params = vec![("name", name.to_owned())];
                params.extend(songs.iter().map(|id| ("songId", id.item().to_owned())));
                let mut env = self.playlist_write("createPlaylist", &params).await?;
                let mut details =
                    self.playlist_details(decode(field(&mut env.data, "playlist")?)?)?;
                details.playlist.editable = Some(true);
                details.playlist.owned = Some(true);
                Ok(details)
            }
            .await;
            self.track(Operation::CreatePlaylist, result)
        })
    }
    pub(super) fn playlist_update<'a>(
        &'a self,
        id: &'a MediaId,
        changes: &'a PlaylistUpdate,
    ) -> ServiceFuture<'a, ()> {
        Box::pin(async move {
            self.check_id(id, MediaKind::Playlist)?;
            Self::playlist_text(changes.name.as_deref(), changes.description.as_deref())?;
            self.playlist_songs(&changes.add)?;
            if changes.remove.len() > 2000 {
                return Err(Error::InvalidConfiguration(
                    "playlist writes are limited to 2000 rows",
                ));
            }
            let mut params = vec![("playlistId", id.item().to_owned())];
            if let Some(name) = &changes.name {
                params.push(("name", name.clone()));
            }
            if let Some(comment) = &changes.description {
                params.push(("comment", comment.clone()));
            }
            if let Some(public) = changes.public {
                params.push(("public", public.to_string()));
            }
            params.extend(
                changes
                    .add
                    .iter()
                    .map(|id| ("songIdToAdd", id.item().to_owned())),
            );
            let rows: std::collections::BTreeSet<_> = changes.remove.iter().copied().collect();
            params.extend(
                rows.into_iter()
                    .rev()
                    .map(|i| ("songIndexToRemove", i.to_string())),
            );
            let result = self
                .playlist_write("updatePlaylist", &params)
                .await
                .map(|_| ());
            self.track(Operation::UpdatePlaylist, result)
        })
    }
    pub(super) fn playlist_replace<'a>(
        &'a self,
        id: &'a MediaId,
        songs: &'a [MediaId],
    ) -> ServiceFuture<'a, PlaylistDetails> {
        Box::pin(async move {
            self.check_id(id, MediaKind::Playlist)?;
            self.playlist_songs(songs)?;
            let result = async {
                let mut params = vec![("playlistId", id.item().to_owned())];
                params.extend(songs.iter().map(|id| ("songId", id.item().to_owned())));
                let mut env = self.playlist_write("createPlaylist", &params).await?;
                let details = self.playlist_details(decode(field(&mut env.data, "playlist")?)?)?;
                if details.playlist.id != *id {
                    return Err(Error::InvalidResponse);
                }
                Ok(details)
            }
            .await;
            self.track(Operation::ReplacePlaylist, result)
        })
    }
    pub(super) fn playlist_delete<'a>(&'a self, id: &'a MediaId) -> ServiceFuture<'a, ()> {
        Box::pin(async move {
            self.check_id(id, MediaKind::Playlist)?;
            let result = self
                .playlist_write("deletePlaylist", &[("id", id.item().to_owned())])
                .await
                .map(|_| ());
            self.track(Operation::DeletePlaylist, result)
        })
    }
}
