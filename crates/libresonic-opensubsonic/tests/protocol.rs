//! Real HTTP regressions for the public service boundary and its transport policy.

use std::collections::HashMap;
use std::error::Error as _;
use std::time::Duration;

use libresonic_core::*;
use libresonic_opensubsonic::{Client, ClientOptions, Credentials};
use md5::{Digest, Md5};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn ok(data: Value) -> ResponseTemplate {
    let mut response =
        json!({"status":"ok", "version":"1.16.1", "type":"mock", "serverVersion":"test"});
    response
        .as_object_mut()
        .unwrap()
        .extend(data.as_object().unwrap().clone());
    ResponseTemplate::new(200).set_body_json(json!({"subsonic-response":response}))
}

fn failure(code: i32, message: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(
        json!({"subsonic-response":{"status":"failed", "error":{"code":code,"message":message}}}),
    )
}

fn client(server: &MockServer) -> Client {
    configured(
        &server.uri(),
        Credentials::token("alice", "secret-password"),
        ClientOptions::default().allow_http(true),
    )
}

fn configured(url: &str, credentials: Credentials, options: ClientOptions) -> Client {
    Client::new(ServerId::new("one").unwrap(), url, credentials, options).unwrap()
}

fn id(kind: MediaKind, item: &str) -> MediaId {
    MediaId::new(ServerId::new("one").unwrap(), kind, item).unwrap()
}

async fn endpoint(server: &MockServer, name: &str, response: ResponseTemplate) {
    Mock::given(method("GET"))
        .and(path(format!("/rest/{name}.view")))
        .respond_with(response)
        .mount(server)
        .await;
}

#[tokio::test]
async fn token_signing_path_prefix_and_capabilities_are_server_generic() {
    let server = MockServer::start().await;
    for (name, response) in [
        ("ping", ok(json!({}))),
        (
            "getOpenSubsonicExtensions",
            ok(
                json!({"openSubsonicExtensions":[{"name":"custom","versions":[2,1,2]},{"name":"custom","versions":[3]}]}),
            ),
        ),
    ] {
        Mock::given(method("GET"))
            .and(path(format!("/music/rest/{name}.view")))
            .respond_with(response)
            .expect(1)
            .mount(&server)
            .await;
    }
    let client = configured(
        &format!("{}/music", server.uri()),
        Credentials::token("alice", "secret-password"),
        ClientOptions::default().allow_http(true),
    );
    let service: &dyn MusicService = &client;
    let caps = service.connect().await.unwrap();
    assert_eq!(caps.info.name.as_deref(), Some("mock"));
    assert_eq!(caps.extensions["custom"], [1, 2, 3]);
    assert_eq!(
        caps.operation(Operation::Albums).availability,
        Availability::Unknown
    );
    let requests = server.received_requests().await.unwrap();
    let mut salts = vec![];
    for request in requests {
        let query: HashMap<_, _> = request.url.query_pairs().into_owned().collect();
        assert_eq!(query["u"], "alice");
        assert_eq!(query["v"], "1.16.1");
        assert_eq!(query["c"], "sonicfast");
        assert_eq!(query["f"], "json");
        assert!(!query.contains_key("p"));
        assert!(!query.contains_key("apiKey"));
        let expected: String = Md5::digest(format!("secret-password{}", query["s"]).as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        assert_eq!(query["t"], expected);
        salts.push(query["s"].clone());
    }
    assert_ne!(salts[0], salts[1]);
    assert!(!format!("{client:?}").contains("secret-password"));
}

#[tokio::test]
async fn api_key_auth_does_not_send_username_or_legacy_credentials() {
    let server = MockServer::start().await;
    endpoint(&server, "ping", ok(json!({}))).await;
    endpoint(&server, "getOpenSubsonicExtensions", ok(json!({}))).await;
    let credentials = Credentials::api_key("secret-api-key");
    assert!(!format!("{credentials:?}").contains("secret-api-key"));
    let client = configured(
        &server.uri(),
        credentials,
        ClientOptions::default().allow_http(true),
    );
    assert_eq!(
        client.connect().await.unwrap().authentication,
        AuthenticationMethod::ApiKey
    );
    for request in server.received_requests().await.unwrap() {
        let query: HashMap<_, _> = request.url.query_pairs().into_owned().collect();
        assert_eq!(query["apiKey"], "secret-api-key");
        for name in ["u", "t", "s", "p"] {
            assert!(!query.contains_key(name));
        }
    }
}

#[tokio::test]
async fn legacy_extension_absence_preserves_baseline_browsing() {
    for response in [
        ResponseTemplate::new(404),
        ResponseTemplate::new(501),
        failure(70, "not found"),
        failure(0, "Not implemented"),
    ] {
        let server = MockServer::start().await;
        endpoint(&server, "ping", ok(json!({}))).await;
        endpoint(&server, "getOpenSubsonicExtensions", response).await;
        endpoint(
            &server,
            "getArtists",
            ok(json!({"artists":{"index":[{"artist":[{"id":"artist"}]}]}})),
        )
        .await;
        let client = client(&server);
        let caps = client.connect().await.unwrap();
        assert_eq!(
            caps.operation(Operation::Connection).availability,
            Availability::Available
        );
        assert_eq!(
            caps.operation(Operation::Extensions).availability,
            Availability::Unavailable
        );
        assert_eq!(
            caps.operation(Operation::Artists).availability,
            Availability::Unknown
        );
        let artists = client.artists().await.unwrap();
        assert_eq!(artists[0].name, "Unknown artist");
        assert_eq!(
            client
                .capabilities()
                .operation(Operation::Artists)
                .availability,
            Availability::Available
        );
    }
}

#[tokio::test]
async fn discovery_does_not_hide_auth_permission_rate_limit_or_server_errors() {
    for (response, expected) in [
        (failure(44, "secret-api-key"), Error::Authentication),
        (failure(50, "secret-password"), Error::PermissionDenied),
        (
            failure(0, "database is unavailable"),
            Error::Protocol { code: 0 },
        ),
        (
            ResponseTemplate::new(429).insert_header("Retry-After", "7"),
            Error::RateLimited {
                retry_after: Some(Duration::from_secs(7)),
            },
        ),
        (ResponseTemplate::new(503), Error::Http { status: 503 }),
        (
            ResponseTemplate::new(200).set_body_string("secret-password not JSON"),
            Error::InvalidResponse,
        ),
    ] {
        let server = MockServer::start().await;
        endpoint(&server, "ping", ok(json!({}))).await;
        endpoint(&server, "getOpenSubsonicExtensions", response).await;
        let client = client(&server);
        assert_eq!(client.connect().await, Err(expected.clone()));
        let caps = client.capabilities();
        assert_eq!(
            caps.operation(Operation::Connection).availability,
            Availability::Available
        );
        assert_eq!(
            caps.operation(Operation::Extensions).last_error,
            Some(expected)
        );
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }
}

#[tokio::test]
async fn failed_ping_never_attempts_discovery_and_errors_have_no_secret_source() {
    for (response, expected) in [
        (
            failure(40, "secret-password in echoed error"),
            Error::Authentication,
        ),
        (failure(42, "unsupported auth"), Error::Authentication),
        (
            failure(123, "https://host/?apiKey=secret-api-key"),
            Error::Protocol { code: 123 },
        ),
        (ResponseTemplate::new(401), Error::Authentication),
        (ResponseTemplate::new(403), Error::PermissionDenied),
        (
            ResponseTemplate::new(500).set_body_string("secret-password"),
            Error::Http { status: 500 },
        ),
        (
            ResponseTemplate::new(200).set_body_json(json!({"secret-password":"bad shape"})),
            Error::InvalidResponse,
        ),
    ] {
        let server = MockServer::start().await;
        endpoint(&server, "ping", response).await;
        let error = client(&server).connect().await.unwrap_err();
        assert_eq!(error, expected);
        let shown = format!("{error:?} {error}");
        for secret in ["secret-password", "secret-api-key", "http", "apiKey"] {
            assert!(!shown.contains(secret));
        }
        assert!(error.source().is_none());
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }
}

#[tokio::test]
async fn sparse_album_song_artist_and_search_metadata_map_to_neutral_models() {
    let server = MockServer::start().await;
    endpoint(&server, "getAlbum", ok(json!({"album":{"id":"album", "name":null, "artist":null, "duration":-1, "song":[null,{"id":"song", "title":null, "duration":3, "albumId":"album", "coverArt":"cover", "artistId":"artist"},{"id":"song"}]}}))).await;
    endpoint(
        &server,
        "getSong",
        ok(json!({"song":{"id":"song","track":0,"discNumber":-1,"duration":null}})),
    )
    .await;
    endpoint(
        &server,
        "getArtist",
        ok(json!({"artist":{"id":"artist","name":null,"album":null}})),
    )
    .await;
    endpoint(&server, "search3", ok(json!({"searchResult3":{"artist":null,"album":[{"id":"album"}],"song":[{"id":"song"}]}}))).await;
    let client = client(&server);
    let album = client.album(&id(MediaKind::Album, "album")).await.unwrap();
    assert_eq!(album.album.title, "Untitled album");
    assert_eq!(album.album.duration_ms, None);
    assert!(album.album.artists.is_empty());
    assert_eq!(album.tracks.len(), 2);
    assert_eq!(album.tracks[0].id, album.tracks[1].id); // repeated occurrence is retained
    assert_eq!(album.tracks[0].duration_ms, Some(3000));
    assert_eq!(
        album.tracks[0].artists[0].id,
        Some(id(MediaKind::Artist, "artist"))
    );
    assert_eq!(
        album.tracks[0].artwork,
        Some(id(MediaKind::Artwork, "cover"))
    );
    let song = client.song(&id(MediaKind::Track, "song")).await.unwrap();
    assert_eq!(song.duration_ms, None);
    assert_eq!(song.track_number, None);
    assert_eq!(song.disc_number, None);
    let artist = client
        .artist(&id(MediaKind::Artist, "artist"))
        .await
        .unwrap();
    assert_eq!(artist.artist.name, "Unknown artist");
    assert!(artist.albums.is_empty());
    let results = client
        .search("a&query=encoded", PageRequest::new(4, 2).unwrap())
        .await
        .unwrap();
    assert!(results.artists.items.is_empty());
    assert_eq!(results.albums.items[0].title, "Untitled album");
    assert_eq!(results.tracks.items[0].title, "Untitled track");
    let request = server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.url.path().contains("search3"))
        .unwrap();
    let query: HashMap<_, _> = request.url.query_pairs().into_owned().collect();
    assert_eq!(query["query"], "a&query=encoded");
    for name in ["artistOffset", "albumOffset", "songOffset"] {
        assert_eq!(query[name], "4");
    }
}

#[tokio::test]
async fn multiple_artist_credits_keep_unlinked_names_without_fabricated_ids() {
    let server = MockServer::start().await;
    endpoint(&server, "getSong", ok(json!({"song":{"id":"song", "artist":"Legacy name", "artists":[{"id":"artist","name":"First"},null,{"name":"Unlinked credit"}]}}))).await;
    let track = client(&server)
        .song(&id(MediaKind::Track, "song"))
        .await
        .unwrap();
    assert_eq!(track.artists.len(), 2);
    assert_eq!(track.artists[0].id, Some(id(MediaKind::Artist, "artist")));
    assert_eq!(track.artists[1].id, None);
    assert_eq!(track.artists[1].name, "Unlinked credit");
}

#[tokio::test]
async fn pages_advance_by_raw_slots_and_empty_collections_are_usable() {
    let server = MockServer::start().await;
    Mock::given(path("/rest/getAlbumList2.view"))
        .and(query_param("offset", "0"))
        .respond_with(ok(json!({"albumList2":{"album":[null,{"id":"a"}]}})))
        .mount(&server)
        .await;
    Mock::given(path("/rest/getAlbumList2.view"))
        .and(query_param("offset", "2"))
        .respond_with(ok(json!({"albumList2":{}})))
        .mount(&server)
        .await;
    endpoint(&server, "search3", ok(json!({}))).await;
    endpoint(&server, "getArtists", ok(json!({"artists":null}))).await;
    let client = client(&server);
    let first = client
        .albums(AlbumOrder::Title, PageRequest::new(0, 2).unwrap())
        .await
        .unwrap();
    assert_eq!(first.items.len(), 1);
    let second = client
        .albums(AlbumOrder::Title, first.next.unwrap())
        .await
        .unwrap();
    assert!(second.items.is_empty());
    assert_eq!(second.next, None);
    assert!(client.artists().await.unwrap().is_empty());
    let results = client
        .search("no matches", PageRequest::new(0, 2).unwrap())
        .await
        .unwrap();
    assert!(results.tracks.items.is_empty());
    assert_eq!(results.tracks.next, None);
}

#[tokio::test]
async fn malformed_identity_shapes_and_oversized_pages_fail_safely() {
    for song in [
        json!({"title":"no id"}),
        json!({"id":""}),
        json!({"id":5}),
        json!({"id":"s","isDir":true}),
    ] {
        let server = MockServer::start().await;
        endpoint(&server, "getSong", ok(json!({"song":song}))).await;
        assert_eq!(
            client(&server).song(&id(MediaKind::Track, "s")).await,
            Err(Error::InvalidResponse)
        );
    }
    let server = MockServer::start().await;
    endpoint(
        &server,
        "getAlbumList2",
        ok(json!({"albumList2":{"album":[{"id":"1"},{"id":"2"}]}})),
    )
    .await;
    assert_eq!(
        client(&server)
            .albums(AlbumOrder::Title, PageRequest::new(0, 1).unwrap())
            .await,
        Err(Error::InvalidResponse)
    );
}

#[tokio::test]
async fn artwork_is_authenticated_bytes_and_json_errors_are_not_images() {
    let server = MockServer::start().await;
    Mock::given(path("/rest/getCoverArt.view"))
        .and(query_param("id", "cover&x=1"))
        .and(query_param("size", "64"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "image/png")
                .set_body_bytes(vec![137, 80, 78, 71]),
        )
        .mount(&server)
        .await;
    let client = client(&server);
    let bytes = client
        .artwork(&id(MediaKind::Artwork, "cover&x=1"), Some(64))
        .await
        .unwrap();
    assert_eq!(bytes, [137, 80, 78, 71]);
    server.reset().await;
    endpoint(&server, "getCoverArt", failure(40, "secret-password")).await;
    assert_eq!(
        client.artwork(&id(MediaKind::Artwork, "cover"), None).await,
        Err(Error::Authentication)
    );
    server.reset().await;
    endpoint(&server, "getCoverArt", ok(json!({}))).await;
    assert_eq!(
        client.artwork(&id(MediaKind::Artwork, "cover"), None).await,
        Err(Error::InvalidResponse)
    );
}

#[tokio::test]
async fn foreign_server_wrong_kind_and_zero_art_size_never_send_requests() {
    let server = MockServer::start().await;
    let client = client(&server);
    let foreign = MediaId::new(ServerId::new("other").unwrap(), MediaKind::Track, "s").unwrap();
    assert_eq!(client.song(&foreign).await, Err(Error::WrongServer));
    assert_eq!(
        client.song(&id(MediaKind::Album, "s")).await,
        Err(Error::InvalidIdentifier)
    );
    assert_eq!(
        client.artwork(&id(MediaKind::Artwork, "c"), Some(0)).await,
        Err(Error::InvalidConfiguration("artwork size must be nonzero"))
    );
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn redirects_never_reach_the_target_for_metadata_or_artwork() {
    let target = MockServer::start().await;
    let server = MockServer::start().await;
    let location = format!("{}/stolen?apiKey=secret-api-key", target.uri());
    for name in ["ping", "getCoverArt"] {
        endpoint(
            &server,
            name,
            ResponseTemplate::new(302).insert_header("Location", location.as_str()),
        )
        .await;
    }
    let client = client(&server);
    assert_eq!(client.connect().await, Err(Error::RedirectBlocked));
    assert_eq!(
        client.artwork(&id(MediaKind::Artwork, "c"), None).await,
        Err(Error::RedirectBlocked)
    );
    assert!(target.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn content_length_limits_apply_to_metadata_and_artwork() {
    let server = MockServer::start().await;
    endpoint(&server, "ping", ok(json!({}))).await;
    endpoint(
        &server,
        "getCoverArt",
        ResponseTemplate::new(200)
            .insert_header("Content-Type", "image/png")
            .set_body_bytes(vec![1; 17]),
    )
    .await;
    let client = configured(
        &server.uri(),
        Credentials::api_key("secret-api-key"),
        ClientOptions::default()
            .allow_http(true)
            .response_limits(16, 16),
    );
    assert_eq!(
        client.connect().await,
        Err(Error::ResponseLimitExceeded {
            resource: "ping",
            limit_bytes: 16
        })
    );
    assert_eq!(
        client.artwork(&id(MediaKind::Artwork, "c"), None).await,
        Err(Error::ResponseLimitExceeded {
            resource: "getCoverArt",
            limit_bytes: 16
        })
    );
}

#[tokio::test]
async fn full_artist_index_has_its_own_bounded_budget_and_safe_diagnostics() {
    let server = MockServer::start().await;
    // Unlike album/search pages, getArtists cannot be paginated. A large real
    // library can exceed the ordinary 4 MiB budget with legitimate metadata.
    let artists: Vec<_> = (0..20_000).map(|i| json!({
        "id": format!("artist-{i}"), "name": "A fairly long artist name from a large library",
        "albumCount": 25, "coverArt": format!("ar-{i}"),
        "musicBrainzId": "00000000-0000-0000-0000-000000000000",
        "sortName": "An artist sort name", "roles": ["artist", "albumartist"]
    })).collect();
    let body =
        json!({"subsonic-response": {"status":"ok", "artists": {"index": [{"artist": artists}]}}})
            .to_string();
    assert!(body.len() > 4 * 1024 * 1024 && body.len() < 32 * 1024 * 1024);
    endpoint(
        &server,
        "getArtists",
        ResponseTemplate::new(200).set_body_string(body),
    )
    .await;
    assert_eq!(client(&server).artists().await.unwrap().len(), 20_000);
    let capped = configured(
        &server.uri(),
        Credentials::api_key("secret-api-key"),
        ClientOptions::default()
            .allow_http(true)
            .artist_index_limit(1024),
    );
    let error = capped.artists().await.unwrap_err();
    assert_eq!(
        error,
        Error::ResponseLimitExceeded {
            resource: "getArtists",
            limit_bytes: 1024
        }
    );
    assert_eq!(
        error.to_string(),
        "The getArtists response exceeded its 1024-byte limit"
    );
    assert!(error.source().is_none());
    assert!(!format!("{error:?} {error}").contains("secret"));
    let capped = configured(
        &server.uri(),
        Credentials::api_key("secret-api-key"),
        ClientOptions::default()
            .allow_http(true)
            .response_limits(1024, 1024),
    );
    assert_eq!(capped.artists().await.unwrap_err(), error);
}

async fn raw_server(response: Vec<u8>, stall_body: bool) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = vec![];
        while !request.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            if socket.read_exact(&mut byte).await.is_err() {
                return;
            }
            request.push(byte[0]);
            assert!(request.len() < 8192);
        }
        socket.write_all(&response).await.unwrap();
        if stall_body {
            let mut byte = [0];
            let _ = socket.read(&mut byte).await;
        }
    });
    (url, task)
}

#[tokio::test]
async fn chunked_bodies_are_limited_without_content_length() {
    let mut response = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n40\r\n".to_vec();
    response.extend([b'a'; 64]);
    response.extend(b"\r\n0\r\n\r\n");
    let (url, server) = raw_server(response, false).await;
    let client = configured(
        &url,
        Credentials::api_key("secret"),
        ClientOptions::default()
            .allow_http(true)
            .response_limits(32, 32),
    );
    assert_eq!(
        client.connect().await,
        Err(Error::ResponseLimitExceeded {
            resource: "ping",
            limit_bytes: 32
        })
    );
    server.await.unwrap();
}

#[tokio::test]
async fn request_timeout_covers_a_stalled_response_body() {
    let (url, server) = raw_server(
        b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 100\r\n\r\n{"
            .to_vec(),
        true,
    )
    .await;
    let client = configured(
        &url,
        Credentials::api_key("secret"),
        ClientOptions::default()
            .allow_http(true)
            .timeouts(Duration::from_millis(150), Duration::from_millis(150)),
    );
    assert_eq!(client.connect().await, Err(Error::Timeout));
    tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn dropped_connect_future_cannot_publish_late_discovery_state() {
    let server = MockServer::start().await;
    endpoint(&server, "ping", ok(json!({}))).await;
    endpoint(
        &server,
        "getOpenSubsonicExtensions",
        ok(json!({"openSubsonicExtensions":[{"name":"late","versions":[1]}]}))
            .set_delay(Duration::from_millis(250)),
    )
    .await;
    let client = client(&server);
    let mut connection = client.connect();
    let received = async {
        loop {
            if server.received_requests().await.unwrap().len() == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    };
    tokio::select! {
        _ = &mut connection => panic!("discovery should still be pending"),
        _ = tokio::time::timeout(Duration::from_secs(2), received) => {}
    }
    drop(connection);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(client.capabilities().extensions.is_empty());
    assert_eq!(
        client
            .capabilities()
            .operation(Operation::Extensions)
            .availability,
        Availability::Unknown
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn runtime_failure_does_not_erase_successful_operation_evidence() {
    let server = MockServer::start().await;
    endpoint(&server, "getArtists", ok(json!({"artists":{}}))).await;
    let client = client(&server);
    client.artists().await.unwrap();
    server.reset().await;
    endpoint(&server, "getArtists", ResponseTemplate::new(503)).await;
    assert_eq!(client.artists().await, Err(Error::Http { status: 503 }));
    let state = client.capabilities().operation(Operation::Artists);
    assert_eq!(state.availability, Availability::Available);
    assert_eq!(state.last_error, Some(Error::Http { status: 503 }));
}

#[test]
fn connection_validation_never_echoes_raw_urls_or_secrets() {
    for url in [
        "ftp://host/path",
        "https://alice:secret-password@host",
        "https://host?apiKey=secret-api-key",
        "https://host#secret",
        "not a url",
        "http://host",
    ] {
        let error = Client::new(
            ServerId::new("one").unwrap(),
            url,
            Credentials::api_key("secret-api-key"),
            ClientOptions::default(),
        )
        .unwrap_err();
        let shown = format!("{error} {error:?}");
        assert!(!shown.contains(url));
        assert!(!shown.contains("secret"));
    }
    let error = Client::new(
        ServerId::new("one").unwrap(),
        "https://host",
        Credentials::api_key(""),
        ClientOptions::default(),
    )
    .unwrap_err();
    assert!(matches!(error, Error::InvalidConfiguration(_)));
}

#[tokio::test]
async fn unreachable_transport_errors_drop_authenticated_request_urls() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let client = configured(
        &url,
        Credentials::api_key("secret-api-key"),
        ClientOptions::default().allow_http(true),
    );
    let error = client.connect().await.unwrap_err();
    assert_eq!(error, Error::Network);
    assert!(error.source().is_none());
    assert!(!format!("{error:?} {error}").contains("secret"));
}

#[tokio::test]
async fn audio_is_authenticated_bounded_and_scoped() {
    let server = MockServer::start().await;
    Mock::given(path("/rest/stream.view"))
        .and(query_param("id", "song&extra=1"))
        .and(query_param("format", "raw"))
        .and(query_param("u", "alice"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "audio/flac")
                .set_body_bytes(vec![42; 8 * 1024 * 1024 + 7]),
        )
        .mount(&server)
        .await;
    let client = client(&server);
    let foreign = MediaId::new(ServerId::new("other").unwrap(), MediaKind::Track, "s").unwrap();
    assert!(matches!(
        client.open(&foreign).await,
        Err(Error::WrongServer)
    ));
    assert!(matches!(
        client.open(&id(MediaKind::Album, "s")).await,
        Err(Error::InvalidIdentifier)
    ));
    let mut stream = client
        .open(&id(MediaKind::Track, "song&extra=1"))
        .await
        .unwrap();
    let mut count = 0;
    while let Some(chunk) = stream.next().await.unwrap() {
        assert!(!chunk.is_empty() && chunk.len() <= STREAM_CHUNK_BYTES);
        assert!(chunk.iter().all(|b| *b == 42));
        count += chunk.len();
        // Real playback applies backpressure while decoding and feeding audio.
        // Let socket reads accumulate instead of immediately draining the body.
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    assert_eq!(count, 8 * 1024 * 1024 + 7);
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let params: HashMap<_, _> = requests[0].url.query_pairs().into_owned().collect();
    let token = format!(
        "{:x}",
        Md5::digest(format!("secret-password{}", params["s"]))
    );
    assert_eq!(params["t"], token);
    assert!(!params.contains_key("p"));
    assert_eq!(
        client
            .capabilities()
            .operation(Operation::Stream)
            .availability,
        Availability::Available
    );
}

#[tokio::test]
async fn audio_redirects_json_failures_and_invalid_mime_are_safe() {
    let target = MockServer::start().await;
    let server = MockServer::start().await;
    for (response, expected) in [
        (
            ResponseTemplate::new(302)
                .insert_header("Location", format!("{}/secret", target.uri())),
            Error::RedirectBlocked,
        ),
        (failure(40, "secret-password"), Error::Authentication),
        (ok(json!({})), Error::InvalidResponse),
        (
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/html")
                .set_body_string("secret-password"),
            Error::InvalidResponse,
        ),
    ] {
        server.reset().await;
        endpoint(&server, "stream", response).await;
        let error = client(&server)
            .open(&id(MediaKind::Track, "s"))
            .await
            .err()
            .unwrap();
        assert_eq!(error, expected);
        assert!(!format!("{error:?} {error}").contains("secret"));
    }
    assert!(target.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn audio_byte_ranges_validate_offsets_totals_and_keep_authentication_private() {
    let server = MockServer::start().await;
    let data: Vec<u8> = (0..80_000).map(|i| (i % 251) as u8).collect();
    let body = data.clone();
    Mock::given(path("/rest/stream.view"))
        .and(query_param("format", "raw"))
        .and(query_param("u", "alice"))
        .respond_with(move |request: &wiremock::Request| {
            let mut response = ResponseTemplate::new(200)
                .insert_header("Content-Type", "audio/mp4")
                .insert_header("Accept-Ranges", "bytes");
            let offset = request.headers.get("Range").map(|v| {
                let value = v.to_str().unwrap();
                value
                    .strip_prefix("bytes=")
                    .unwrap()
                    .trim_end_matches('-')
                    .parse::<usize>()
                    .unwrap()
            });
            if let Some(offset) = offset {
                response = ResponseTemplate::new(206)
                    .insert_header("Content-Type", "audio/mp4")
                    .insert_header(
                        "Content-Range",
                        format!("bytes {offset}-{}/{}", body.len() - 1, body.len()),
                    );
            }
            response.set_body_bytes(body[offset.unwrap_or(0)..].to_vec())
        })
        .mount(&server)
        .await;
    let mut stream = client(&server)
        .open(&id(MediaKind::Track, "m4a"))
        .await
        .unwrap();
    assert!(stream.is_seekable());
    assert_eq!(stream.byte_len(), Some(data.len() as u64));
    stream.next().await.unwrap().unwrap();
    stream.seek_to(37_000).await.unwrap();
    let chunk = stream.next().await.unwrap().unwrap();
    assert!(chunk.len() <= STREAM_CHUNK_BYTES);
    assert_eq!(chunk, data[37_000..37_000 + chunk.len()]);
    stream.seek_to(data.len() as u64).await.unwrap();
    assert!(stream.next().await.unwrap().is_none());
    stream.seek_to(0).await.unwrap();
    let chunk = stream.next().await.unwrap().unwrap();
    assert_eq!(chunk, data[..chunk.len()]);
    assert_eq!(server.received_requests().await.unwrap().len(), 3);
}

#[tokio::test]
async fn audio_seek_rejects_ignored_or_mismatched_ranges_without_reinterpreting_bytes() {
    let server = MockServer::start().await;
    for response in [
        ResponseTemplate::new(200).set_body_bytes(vec![0; 4]),
        ResponseTemplate::new(206)
            .insert_header("Content-Range", "bytes 0-3/4")
            .set_body_bytes(vec![0; 4]),
        ResponseTemplate::new(206)
            .insert_header("Content-Range", "bytes 2-3/5")
            .set_body_bytes(vec![0; 2]),
        ResponseTemplate::new(206)
            .insert_header("Content-Range", "bytes 2-3/4")
            .set_body_bytes(vec![0; 1]),
    ] {
        server.reset().await;
        endpoint(
            &server,
            "stream",
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "audio/mp4")
                .insert_header("Accept-Ranges", "bytes")
                .set_body_bytes(vec![0; 4]),
        )
        .await;
        let mut stream = client(&server)
            .open(&id(MediaKind::Track, "m4a"))
            .await
            .unwrap();
        server.reset().await;
        endpoint(&server, "stream", response).await;
        assert_eq!(stream.seek_to(2).await, Err(Error::InvalidResponse));
    }
    server.reset().await;
    endpoint(
        &server,
        "stream",
        ResponseTemplate::new(200)
            .insert_header("Content-Type", "audio/mp4")
            .set_body_bytes(vec![0; 4]),
    )
    .await;
    let mut stream = client(&server)
        .open(&id(MediaKind::Track, "m4a"))
        .await
        .unwrap();
    assert!(!stream.is_seekable());
    assert_eq!(stream.seek_to(2).await, Err(Error::Unsupported));
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn audio_deadlines_cover_reads_but_not_song_length_or_consumer_pause() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = vec![];
        while !request.ends_with(b"\r\n\r\n") {
            request.push(socket.read_u8().await.unwrap());
        }
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: audio/mpeg\r\nContent-Length: 5\r\n\r\n")
            .await
            .unwrap();
        for _ in 0..5 {
            socket.write_all(b"x").await.unwrap();
            tokio::time::sleep(Duration::from_millis(70)).await;
        }
    });
    let client = configured(
        &url,
        Credentials::api_key("secret"),
        ClientOptions::default()
            .allow_http(true)
            .timeouts(Duration::from_millis(200), Duration::from_millis(200)),
    );
    let mut stream = client.open(&id(MediaKind::Track, "s")).await.unwrap();
    assert_eq!(stream.next().await.unwrap().unwrap(), b"x");
    tokio::time::sleep(Duration::from_millis(450)).await;
    let mut remaining = 0;
    while let Some(bytes) = stream.next().await.unwrap() {
        remaining += bytes.len();
    }
    assert_eq!(remaining, 4);
    server.await.unwrap();

    let (url, server) = raw_server(
        b"HTTP/1.1 200 OK\r\nContent-Type: audio/flac\r\nContent-Length: 100\r\n\r\n".to_vec(),
        true,
    )
    .await;
    let client = configured(
        &url,
        Credentials::api_key("secret"),
        ClientOptions::default()
            .allow_http(true)
            .timeouts(Duration::from_millis(100), Duration::from_millis(100)),
    );
    let mut stream = client.open(&id(MediaKind::Track, "s")).await.unwrap();
    assert_eq!(stream.next().await, Err(Error::Timeout));
    drop(stream);
    tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .unwrap()
        .unwrap();
}
