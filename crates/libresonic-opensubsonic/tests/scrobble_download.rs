//! Explicit reporting and original-file retrieval preserve authentication boundaries.
use libresonic_core::*;
use libresonic_opensubsonic::{Client, ClientOptions, Credentials};
use serde_json::{Value, json};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};
fn id() -> MediaId {
    MediaId::new(
        ServerId::new("fixture").unwrap(),
        MediaKind::Track,
        "song & 雪",
    )
    .unwrap()
}
fn client(server: &MockServer) -> Client {
    Client::new(
        ServerId::new("fixture").unwrap(),
        &server.uri(),
        Credentials::token("fixture", "test-secret"),
        ClientOptions::default().allow_http(true),
    )
    .unwrap()
}
fn ok(data: Value) -> ResponseTemplate {
    let mut response = json!({"status":"ok"});
    response
        .as_object_mut()
        .unwrap()
        .extend(data.as_object().unwrap().clone());
    ResponseTemplate::new(200).set_body_json(json!({"subsonic-response":response}))
}
#[tokio::test]
async fn original_metadata_and_download_are_not_transcoded_and_redirects_are_blocked() {
    let server = MockServer::start().await;
    Mock::given(path("/rest/getSong.view")).respond_with(ok(json!({"song":{"id":"song & 雪","path":"Artist/Album/original.m4a","size":12345,"playCount":7}}))).mount(&server).await;
    Mock::given(path("/rest/download.view"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "audio/mp4")
                .set_body_bytes(vec![1; 70_000]),
        )
        .mount(&server)
        .await;
    let c = client(&server);
    let song = c.song(&id()).await.unwrap();
    assert_eq!(song.file_path.as_deref(), Some("Artist/Album/original.m4a"));
    assert_eq!(song.file_size, Some(12345));
    assert_eq!(song.play_count, Some(7));
    let mut stream = c.download(&id()).await.unwrap();
    let mut n = 0;
    while let Some(bytes) = stream.next().await.unwrap() {
        assert!(bytes.len() <= STREAM_CHUNK_BYTES);
        n += bytes.len();
    }
    assert_eq!(n, 70000);
    let request = server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.url.path().contains("download"))
        .unwrap();
    assert!(
        !request
            .url
            .query_pairs()
            .any(|(k, _)| k == "format" || k == "maxBitRate")
    );
    server.reset().await;
    Mock::given(path("/rest/download.view"))
        .respond_with(
            ResponseTemplate::new(302).insert_header("Location", "https://example.invalid/secret"),
        )
        .expect(1)
        .mount(&server)
        .await;
    assert!(matches!(
        c.download(&id()).await,
        Err(Error::RedirectBlocked)
    ));
}
#[tokio::test]
async fn scrobble_posts_keep_timestamp_and_submission_distinct_and_do_not_retry() {
    let server = MockServer::start().await;
    Mock::given(path("/rest/ping.view"))
        .respond_with(ok(json!({})))
        .mount(&server)
        .await;
    Mock::given(path("/rest/getOpenSubsonicExtensions.view"))
        .respond_with(ok(
            json!({"openSubsonicExtensions":[{"name":"formPost","versions":[1]}]}),
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/rest/scrobble.view"))
        .respond_with(ok(json!({})))
        .expect(2)
        .mount(&server)
        .await;
    let c = client(&server);
    c.connect().await.unwrap();
    c.scrobble(&id(), 123456, false).await.unwrap();
    c.scrobble(&id(), 123456, true).await.unwrap();
    let requests = server.received_requests().await.unwrap();
    let writes: Vec<_> = requests.iter().filter(|r| r.method == "POST").collect();
    assert_eq!(writes.len(), 2);
    for (i, r) in writes.iter().enumerate() {
        assert!(r.url.query().is_none());
        let fields: std::collections::HashMap<_, _> =
            url::form_urlencoded::parse(&r.body).into_owned().collect();
        assert_eq!(fields["id"], "song & 雪");
        assert_eq!(fields["time"], "123456");
        assert_eq!(fields["submission"], if i == 0 { "false" } else { "true" });
    }
    server.reset().await;
    Mock::given(path("/rest/scrobble.view"))
        .respond_with(ResponseTemplate::new(503).set_body_string("test-secret"))
        .expect(1)
        .mount(&server)
        .await;
    let error = c.scrobble(&id(), 123, true).await.unwrap_err();
    assert_eq!(error, Error::Http { status: 503 });
    assert!(!format!("{error:?}").contains("test-secret"));
}
#[tokio::test]
async fn download_errors_and_foreign_ids_cannot_become_local_audio() {
    let server = MockServer::start().await;
    let c = client(&server);
    let foreign = MediaId::new(ServerId::new("other").unwrap(), MediaKind::Track, "a").unwrap();
    assert!(matches!(
        c.download(&foreign).await,
        Err(Error::WrongServer)
    ));
    assert_eq!(
        c.scrobble(&foreign, 1, true).await.unwrap_err(),
        Error::WrongServer
    );
    assert!(server.received_requests().await.unwrap().is_empty());
    for mime in ["text/html", "text/xml"] {
        server.reset().await;
        Mock::given(path("/rest/download.view"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Content-Type", mime)
                    .set_body_string("not audio test-secret"),
            )
            .mount(&server)
            .await;
        assert!(matches!(
            c.download(&id()).await,
            Err(Error::InvalidResponse)
        ));
    }
    server.reset().await;
    Mock::given(path("/rest/download.view"))
        .respond_with(
            ResponseTemplate::new(206)
                .insert_header("Content-Type", "audio/flac")
                .set_body_bytes(vec![1; 100]),
        )
        .mount(&server)
        .await;
    assert!(matches!(
        c.download(&id()).await,
        Err(Error::InvalidResponse)
    ));
}
