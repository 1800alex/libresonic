//! Personal song feedback; no automatic mutation retries.
use super::*;

impl Client {
    pub(super) fn favorite_song_list(&self) -> ServiceFuture<'_, Vec<Track>> {
        Box::pin(async move {
            let result = async {
                let mut env = self.transport.json("getStarred2", &[]).await?;
                let mut data = collection(&mut env.data, "starred2")?;
                let value = data
                    .as_object_mut()
                    .ok_or(Error::InvalidResponse)?
                    .remove("song");
                let (songs, _) = list::<wire::Child>(value)?;
                songs
                    .into_iter()
                    .map(|t| {
                        let mut song = Mapper(&self.server).track(t)?;
                        song.favorite = true; // Membership is evidence even without a timestamp.
                        Ok(song)
                    })
                    .collect()
            }
            .await;
            self.track(Operation::FavoriteSongs, result)
        })
    }
    pub(super) fn favorite_song_write<'a>(
        &'a self,
        ids: &'a [MediaId],
        favorite: bool,
    ) -> ServiceFuture<'a, ()> {
        Box::pin(async move {
            if ids.is_empty() || ids.len() > 2000 {
                return Err(Error::InvalidConfiguration(
                    "select between 1 and 2000 songs",
                ));
            }
            for id in ids {
                self.check_id(id, MediaKind::Track)?;
            }
            let params: Vec<_> = ids.iter().map(|id| ("id", id.item().to_owned())).collect();
            let result = self
                .feedback_write(if favorite { "star" } else { "unstar" }, &params)
                .await;
            self.track(Operation::SetFavoriteSongs, result)
        })
    }
    pub(super) fn song_rating_write<'a>(
        &'a self,
        id: &'a MediaId,
        rating: u8,
    ) -> ServiceFuture<'a, ()> {
        Box::pin(async move {
            self.check_id(id, MediaKind::Track)?;
            if rating > 5 {
                return Err(Error::InvalidConfiguration(
                    "song rating must be between 0 and 5",
                ));
            }
            let result = self
                .feedback_write(
                    "setRating",
                    &[("id", id.item().to_owned()), ("rating", rating.to_string())],
                )
                .await;
            self.track(Operation::SetSongRating, result)
        })
    }
    async fn feedback_write(
        &self,
        endpoint: &'static str,
        params: &[(&str, String)],
    ) -> Result<(), Error> {
        let post = self
            .capabilities()
            .extensions
            .get("formPost")
            .is_some_and(|v| v.contains(&1));
        self.transport
            .json_write(endpoint, params, post)
            .await
            .map(|_| ())
    }
}
