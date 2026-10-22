//! Song hearts and ratings are independent, scoped server annotations.
use libresonic_core::*;
use libresonic_opensubsonic::{Client, ClientOptions, Credentials};
use serde_json::{Value, json};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};
fn ok(data: Value) -> ResponseTemplate {
    let mut response = json!({"status":"ok"});
    response
        .as_object_mut()
        .unwrap()
        .extend(data.as_object().unwrap().clone());
    ResponseTemplate::new(200).set_body_json(json!({"subsonic-response":response}))
}
fn id(raw: &str) -> MediaId {
    MediaId::new(ServerId::new("one").unwrap(), MediaKind::Track, raw).unwrap()
}
fn client(server: &MockServer) -> Client {
    Client::new(
        ServerId::new("one").unwrap(),
        &server.uri(),
        Credentials::token("alice", "test-secret"),
        ClientOptions::default().allow_http(true),
    )
    .unwrap()
}
#[tokio::test]
async fn favorites_and_personal_ratings_map_sparse_values_independently() {
    let server = MockServer::start().await;
    Mock::given(path("/rest/getStarred2.view")).respond_with(ok(json!({"starred2":{"song":[{"id":"a","starred":"date","userRating":5},null,{"id":"b","userRating":0},{"id":"c","userRating":6}]}}))).mount(&server).await;
    let songs = client(&server).favorite_songs().await.unwrap();
    assert_eq!(songs.len(), 3);
    assert!(songs[0].favorite);
    assert_eq!(songs[0].rating, Some(5));
    assert!(songs[1].favorite);
    assert_eq!(songs[1].rating, None);
    assert_eq!(songs[2].rating, None);
    server.reset().await;
    Mock::given(path("/rest/getStarred2.view"))
        .respond_with(ok(json!({})))
        .mount(&server)
        .await;
    assert!(client(&server).favorite_songs().await.unwrap().is_empty());
}
#[tokio::test]
async fn advertised_form_posts_encode_hearts_and_all_ratings_without_credentials_in_url() {
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
    for endpoint in ["star", "unstar", "setRating"] {
        Mock::given(method("POST"))
            .and(path(format!("/rest/{endpoint}.view")))
            .respond_with(ok(json!({})))
            .mount(&server)
            .await;
    }
    let c = client(&server);
    c.connect().await.unwrap();
    let ids = vec![id("a&secret=雪"), id("b")];
    c.set_favorite_songs(&ids, true).await.unwrap();
    c.set_favorite_songs(&ids, false).await.unwrap();
    for rating in 0..=5 {
        c.set_song_rating(&ids[0], rating).await.unwrap();
    }
    let requests = server.received_requests().await.unwrap();
    let writes: Vec<_> = requests.iter().filter(|r| r.method == "POST").collect();
    assert_eq!(writes.len(), 8);
    for request in &writes {
        assert!(request.url.query().is_none());
        assert!(!format!("{:?}", request.url).contains("test-secret"));
    }
    let values: Vec<_> = url::form_urlencoded::parse(&writes[0].body)
        .into_owned()
        .collect();
    assert_eq!(
        values
            .iter()
            .filter(|(k, _)| k == "id")
            .map(|(_, v)| v.as_str())
            .collect::<Vec<_>>(),
        vec!["a&secret=雪", "b"]
    );
    assert_eq!(
        url::form_urlencoded::parse(&writes[7].body)
            .find(|(k, _)| k == "rating")
            .unwrap()
            .1,
        "5"
    );
}
#[tokio::test]
async fn invalid_scopes_kinds_values_and_batches_fail_before_network() {
    let server = MockServer::start().await;
    let c = client(&server);
    let foreign = MediaId::new(ServerId::new("other").unwrap(), MediaKind::Track, "a").unwrap();
    let album = MediaId::new(ServerId::new("one").unwrap(), MediaKind::Album, "a").unwrap();
    for key in [foreign, album] {
        assert!(
            c.set_favorite_songs(std::slice::from_ref(&key), true)
                .await
                .is_err()
        );
        assert!(c.set_song_rating(&key, 5).await.is_err());
    }
    assert!(c.set_song_rating(&id("a"), 6).await.is_err());
    assert!(c.set_favorite_songs(&[], true).await.is_err());
    assert!(
        c.set_favorite_songs(&vec![id("a"); 2001], true)
            .await
            .is_err()
    );
    assert!(
        c.set_favorite_songs(&[id(&"x".repeat(9000))], true)
            .await
            .is_err()
    );
    assert!(server.received_requests().await.unwrap().is_empty());
}
#[tokio::test]
async fn legacy_get_permission_failure_and_unsupported_evidence_are_safe_and_not_retried() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/rest/star.view"))
        .respond_with(ok(json!({})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/rest/setRating.view"))
        .respond_with(ResponseTemplate::new(403).set_body_string("test-secret authenticated-url"))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/rest/getStarred2.view"))
        .respond_with(ResponseTemplate::new(501))
        .expect(1)
        .mount(&server)
        .await;
    let c = client(&server);
    c.set_favorite_songs(&[id("a")], true).await.unwrap();
    let error = c.set_song_rating(&id("a"), 3).await.unwrap_err();
    assert_eq!(error, Error::PermissionDenied);
    assert!(!format!("{error:?}").contains("test-secret"));
    assert_eq!(c.favorite_songs().await.unwrap_err(), Error::Unsupported);
    assert_eq!(
        c.capabilities()
            .operation(Operation::FavoriteSongs)
            .availability,
        Availability::Unavailable
    );
}
