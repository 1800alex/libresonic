//! Playlist protocol regressions with real HTTP and synthetic metadata.
use libresonic_core::*;
use libresonic_opensubsonic::{Client, ClientOptions, Credentials};
use serde_json::{Value, json};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

fn ok(value: Value) -> ResponseTemplate {
    let mut envelope = json!({"status":"ok"});
    envelope
        .as_object_mut()
        .unwrap()
        .extend(value.as_object().unwrap().clone());
    ResponseTemplate::new(200).set_body_json(json!({"subsonic-response": envelope}))
}
fn id(kind: MediaKind, raw: &str) -> MediaId {
    MediaId::new(ServerId::new("one").unwrap(), kind, raw).unwrap()
}
fn client(server: &MockServer) -> Client {
    Client::new(
        ServerId::new("one").unwrap(),
        &server.uri(),
        Credentials::token("alice", "secret-password"),
        ClientOptions::default().allow_http(true),
    )
    .unwrap()
}
async fn endpoint(server: &MockServer, name: &str, response: ResponseTemplate) {
    Mock::given(path(format!("/rest/{name}.view")))
        .respond_with(response)
        .mount(server)
        .await;
}
fn playlist() -> Value {
    json!({"id":"p", "name":"Road & 雪", "owner":"alice", "comment":"hello", "public":false,
        "songCount":5, "coverArt":"cover", "changed":"same-timestamp",
        "entry":[{"id":"a", "suffix":"mp3"}, null, {"id":"a", "suffix":"mp3"}, {"id":"dir", "isDir":true}, {"id":"b"}]})
}

#[tokio::test]
async fn playlist_reads_preserve_slots_duplicates_owner_and_content_fingerprints() {
    let server = MockServer::start().await;
    endpoint(
        &server,
        "getPlaylists",
        ok(json!({"playlists":{"playlist":[playlist(), {"id":"other", "owner":"bob"}]}})),
    )
    .await;
    endpoint(&server, "getPlaylist", ok(json!({"playlist":playlist()}))).await;
    let client = client(&server);
    let listed = client.playlists().await.unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].editable, Some(true));
    assert_eq!(listed[1].editable, Some(false));
    let p = client
        .playlist(&id(MediaKind::Playlist, "p"))
        .await
        .unwrap();
    assert_eq!(p.entries.len(), 5);
    assert_eq!(p.entries[0], p.entries[2]);
    assert!(p.entries[1].is_none() && p.entries[3].is_none());
    assert_eq!(p.playlist.id.server().as_str(), "one");
    assert_eq!(p.playlist.description.as_deref(), Some("hello"));
    server.reset().await;
    let mut changed = playlist();
    changed["entry"][2]["id"] = json!("changed");
    endpoint(&server, "getPlaylist", ok(json!({"playlist": changed}))).await;
    let changed = client
        .playlist(&id(MediaKind::Playlist, "p"))
        .await
        .unwrap();
    assert_ne!(
        p.revision, changed.revision,
        "same timestamps must not hide occurrence changes"
    );
}

#[tokio::test]
async fn editability_hints_do_not_invent_ownership_for_shared_or_api_key_playlists() {
    let server = MockServer::start().await;
    endpoint(
        &server,
        "getPlaylists",
        ok(json!({"playlists":{"playlist":[
            {"id":"locked", "owner":"alice", "readonly":true},
            {"id":"shared", "owner":"bob", "readonly":false},
            {"id":"unknown", "owner":"bob"}
        ]}})),
    )
    .await;
    let token = client(&server).playlists().await.unwrap();
    assert_eq!(
        (token[0].owned, token[0].editable),
        (Some(true), Some(false))
    );
    assert_eq!(
        (token[1].owned, token[1].editable),
        (Some(false), Some(true))
    );
    let key = Client::new(
        ServerId::new("one").unwrap(),
        &server.uri(),
        Credentials::api_key("secret-key"),
        ClientOptions::default().allow_http(true),
    )
    .unwrap();
    let items = key.playlists().await.unwrap();
    assert_eq!((items[1].owned, items[1].editable), (None, Some(true)));
    assert_eq!((items[2].owned, items[2].editable), (None, None));
}

#[tokio::test]
async fn api_key_owner_identity_uses_advertised_token_info_without_password_fallback() {
    let server = MockServer::start().await;
    endpoint(&server, "ping", ok(json!({}))).await;
    endpoint(
        &server,
        "getOpenSubsonicExtensions",
        ok(json!({"openSubsonicExtensions":[{"name":"apiKeyAuthentication","versions":[1]}]})),
    )
    .await;
    endpoint(
        &server,
        "tokenInfo",
        ok(json!({"tokenInfo":{"username":"alice"}})),
    )
    .await;
    endpoint(
        &server,
        "getPlaylists",
        ok(json!({"playlists":{"playlist":[playlist()]}})),
    )
    .await;
    let key = Client::new(
        ServerId::new("one").unwrap(),
        &server.uri(),
        Credentials::api_key("secret-key"),
        ClientOptions::default().allow_http(true),
    )
    .unwrap();
    key.connect().await.unwrap();
    let p = key.playlists().await.unwrap();
    assert_eq!((p[0].owned, p[0].editable), (Some(true), Some(true)));
    assert_eq!(
        key.capabilities()
            .operation(Operation::AccountIdentity)
            .availability,
        Availability::Available
    );
    for r in server.received_requests().await.unwrap() {
        let fields: Vec<_> = r.url.query_pairs().into_owned().collect();
        assert!(
            !fields
                .iter()
                .any(|(k, _)| matches!(k.as_str(), "u" | "p" | "t" | "s"))
        );
    }
}

#[tokio::test]
async fn advertised_form_post_writes_keep_repeated_ids_and_exact_original_indices() {
    let server = MockServer::start().await;
    endpoint(&server, "ping", ok(json!({}))).await;
    endpoint(
        &server,
        "getOpenSubsonicExtensions",
        ok(json!({"openSubsonicExtensions":[{"name":"formPost","versions":[1]}]})),
    )
    .await;
    for name in ["createPlaylist", "updatePlaylist", "deletePlaylist"] {
        Mock::given(method("POST"))
            .and(path(format!("/rest/{name}.view")))
            .respond_with(if name == "createPlaylist" {
                ok(json!({"playlist":playlist()}))
            } else {
                ok(json!({}))
            })
            .mount(&server)
            .await;
    }
    let client = client(&server);
    client.connect().await.unwrap();
    let a = id(MediaKind::Track, "a&extra=secret");
    let b = id(MediaKind::Track, "b");
    client
        .create_playlist("Road & +雪", &[a.clone(), a.clone(), b.clone()])
        .await
        .unwrap();
    client
        .update_playlist(
            &id(MediaKind::Playlist, "p"),
            &PlaylistUpdate {
                description: Some(String::new()),
                public: Some(false),
                add: vec![a.clone(), a],
                remove: vec![0, 2, 2],
                ..Default::default()
            },
        )
        .await
        .unwrap();
    client
        .replace_playlist(&id(MediaKind::Playlist, "p"), &[b.clone(), b])
        .await
        .unwrap();
    client
        .delete_playlist(&id(MediaKind::Playlist, "p"))
        .await
        .unwrap();
    let requests = server.received_requests().await.unwrap();
    let writes: Vec<_> = requests.iter().filter(|r| r.method == "POST").collect();
    assert_eq!(writes.len(), 4);
    for request in &writes {
        assert!(request.url.query().is_none());
        assert_eq!(
            request.headers["content-type"],
            "application/x-www-form-urlencoded"
        );
        let fields: Vec<_> = url::form_urlencoded::parse(&request.body)
            .into_owned()
            .collect();
        assert!(fields.contains(&("u".into(), "alice".into())));
        assert!(!fields.iter().any(|(k, _)| k == "p"));
    }
    let fields: Vec<_> = url::form_urlencoded::parse(&writes[0].body)
        .into_owned()
        .collect();
    assert!(fields.contains(&("name".into(), "Road & +雪".into())));
    assert_eq!(
        fields
            .iter()
            .filter(|(k, _)| k == "songId")
            .map(|(_, v)| v.as_str())
            .collect::<Vec<_>>(),
        ["a&extra=secret", "a&extra=secret", "b"]
    );
    let fields: Vec<_> = url::form_urlencoded::parse(&writes[1].body)
        .into_owned()
        .collect();
    assert_eq!(
        fields
            .iter()
            .filter(|(k, _)| k == "songIndexToRemove")
            .map(|(_, v)| v.as_str())
            .collect::<Vec<_>>(),
        ["2", "0"]
    );
    assert!(fields.contains(&("comment".into(), String::new())));
    assert_eq!(
        client
            .capabilities()
            .operation(Operation::UpdatePlaylist)
            .availability,
        Availability::Available
    );
}

#[tokio::test]
async fn playlist_validation_rejects_foreign_ids_wrong_kinds_and_oversized_writes_before_network() {
    let server = MockServer::start().await;
    let client = client(&server);
    let foreign = MediaId::new(ServerId::new("two").unwrap(), MediaKind::Playlist, "p").unwrap();
    assert_eq!(client.playlist(&foreign).await, Err(Error::WrongServer));
    assert_eq!(
        client.delete_playlist(&foreign).await,
        Err(Error::WrongServer)
    );
    assert_eq!(
        client
            .update_playlist(&foreign, &PlaylistUpdate::default())
            .await,
        Err(Error::WrongServer)
    );
    assert_eq!(
        client.replace_playlist(&foreign, &[]).await,
        Err(Error::WrongServer)
    );
    assert_eq!(
        client.playlist(&id(MediaKind::Track, "p")).await,
        Err(Error::InvalidIdentifier)
    );
    let foreign_song = MediaId::new(ServerId::new("two").unwrap(), MediaKind::Track, "a").unwrap();
    assert_eq!(
        client
            .create_playlist("P", &[id(MediaKind::Track, "a"), foreign_song.clone()])
            .await,
        Err(Error::WrongServer)
    );
    assert_eq!(
        client
            .update_playlist(
                &id(MediaKind::Playlist, "p"),
                &PlaylistUpdate {
                    add: vec![foreign_song],
                    ..Default::default()
                }
            )
            .await,
        Err(Error::WrongServer)
    );
    assert!(matches!(
        client.create_playlist("", &[]).await,
        Err(Error::InvalidConfiguration(_))
    ));
    assert!(matches!(
        client
            .create_playlist("P", &vec![id(MediaKind::Track, "a"); 2001])
            .await,
        Err(Error::InvalidConfiguration(_))
    ));
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn legacy_get_has_a_request_budget_and_failed_writes_are_safe_without_retries() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/rest/createPlaylist.view"))
        .respond_with(ok(json!({"playlist":playlist()})))
        .expect(1)
        .mount(&server)
        .await;
    let client = client(&server);
    client.create_playlist("legacy", &[]).await.unwrap();
    assert!(matches!(
        client
            .create_playlist(
                "large",
                &vec![id(MediaKind::Track, "a-long-opaque-track-identity"); 1000]
            )
            .await,
        Err(Error::InvalidConfiguration(_))
    ));
    Mock::given(path("/rest/updatePlaylist.view")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"subsonic-response":{"status":"failed","error":{"code":50,"message":"secret-password authenticated-url"}}}))).expect(1).mount(&server).await;
    let error = client
        .update_playlist(&id(MediaKind::Playlist, "p"), &PlaylistUpdate::default())
        .await
        .unwrap_err();
    assert_eq!(error, Error::PermissionDenied);
    assert!(!format!("{error:?} {error}").contains("secret"));
    assert_eq!(
        client
            .capabilities()
            .operation(Operation::CreatePlaylist)
            .availability,
        Availability::Available
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn playlist_mutation_redirects_and_response_budgets_apply_before_any_follow_up_write() {
    let target = MockServer::start().await;
    let server = MockServer::start().await;
    endpoint(
        &server,
        "deletePlaylist",
        ResponseTemplate::new(302).insert_header("Location", target.uri()),
    )
    .await;
    assert_eq!(
        client(&server)
            .delete_playlist(&id(MediaKind::Playlist, "p"))
            .await,
        Err(Error::RedirectBlocked)
    );
    assert!(target.received_requests().await.unwrap().is_empty());
    endpoint(
        &server,
        "createPlaylist",
        ok(json!({"playlist":playlist()})),
    )
    .await;
    let capped = Client::new(
        ServerId::new("one").unwrap(),
        &server.uri(),
        Credentials::api_key("secret-api-key"),
        ClientOptions::default()
            .allow_http(true)
            .response_limits(16, 16),
    )
    .unwrap();
    assert_eq!(
        capped.create_playlist("P", &[]).await.unwrap_err(),
        Error::ResponseLimitExceeded {
            resource: "createPlaylist",
            limit_bytes: 16
        }
    );
}
