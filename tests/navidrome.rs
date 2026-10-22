//! Opt-in Rust adapter validation against the disposable synthetic fixture.

use libresonic::opensubsonic::{Client, ClientOptions, Credentials};
use libresonic::*;

#[tokio::test]
#[ignore = "requires integration/navidrome fixture and LIBRESONIC_TEST_URL"]
async fn browse_navidrome_through_the_service_boundary() {
    let url = std::env::var("LIBRESONIC_TEST_URL").expect("set the disposable fixture URL");
    let client = Client::new(
        ServerId::new("fixture").unwrap(),
        &url,
        Credentials::token("admin", "admin"),
        ClientOptions::default().allow_http(true),
    )
    .unwrap();
    let service: &dyn MusicService = &client;
    let caps = service.connect().await.unwrap();
    assert_eq!(caps.info.name.as_deref(), Some("navidrome"));
    assert_eq!(caps.authentication, AuthenticationMethod::Token);
    assert_eq!(
        caps.operation(Operation::Connection).availability,
        Availability::Available
    );
    let artists = service.artists().await.unwrap();
    assert_eq!(artists.len(), 8);
    let artist = service.artist(&artists[0].id).await.unwrap();
    assert_eq!(artist.artist.id, artists[0].id);
    assert!(!artist.albums.is_empty());
    let mut albums = Vec::new();
    let mut page = PageRequest::new(0, 10).unwrap();
    loop {
        let result = service.albums(AlbumOrder::Title, page).await.unwrap();
        albums.extend(result.items);
        match result.next {
            Some(next) => {
                assert!(next.offset() <= 30);
                page = next;
            }
            None => break,
        }
    }
    assert_eq!(albums.len(), 24);
    let album = service.album(&albums[0].id).await.unwrap();
    assert_eq!(album.tracks.len(), 11);
    let track = service.song(&album.tracks[0].id).await.unwrap();
    assert_eq!(track.id, album.tracks[0].id);
    assert!(track.duration_ms.is_some_and(|d| d > 0));
    assert_eq!(track.id.server(), &ServerId::new("fixture").unwrap());
    let results = service
        .search(&track.title, PageRequest::new(0, 10).unwrap())
        .await
        .unwrap();
    assert!(results.tracks.items.iter().any(|song| song.id == track.id));
    let art = service
        .artwork(album.album.artwork.as_ref().unwrap(), Some(64))
        .await
        .unwrap();
    assert!(!art.is_empty());
    let invalid = Client::new(
        ServerId::new("fixture").unwrap(),
        &url,
        Credentials::token("admin", "wrong-fixture-password"),
        ClientOptions::default().allow_http(true),
    )
    .unwrap();
    assert_eq!(invalid.connect().await, Err(Error::Authentication));
    println!(
        "Rust facade: artists, artist, paginated albums, album/song/search/artwork and rejected credentials passed"
    );
}
