//! Temporary server connection and browsing harness. Secrets are environment-only.

use std::env;
use std::process::ExitCode;

use libresonic::opensubsonic::{Client, ClientOptions, Credentials};
use libresonic::{AlbumOrder, Error, MusicService, PageRequest, ServerId};

fn required(name: &'static str) -> Result<String, Error> {
    env::var(name).map_err(|_| {
        Error::InvalidConfiguration(match name {
            "LIBRESONIC_URL" => "set LIBRESONIC_URL",
            "LIBRESONIC_USERNAME" => "set LIBRESONIC_USERNAME",
            "LIBRESONIC_PASSWORD" => "set LIBRESONIC_PASSWORD or LIBRESONIC_API_KEY",
            _ => "required connection environment variable is missing",
        })
    })
}

async fn browse() -> Result<(), Error> {
    let mut allow_http = false;
    for argument in env::args().skip(1) {
        if argument != "--allow-http" {
            return Err(Error::InvalidConfiguration(
                "only --allow-http is supported; supply connection settings through the environment",
            ));
        }
        allow_http = true;
    }
    let credentials = match env::var("LIBRESONIC_API_KEY") {
        Ok(key) => Credentials::api_key(key),
        Err(env::VarError::NotPresent) => Credentials::token(
            required("LIBRESONIC_USERNAME")?,
            required("LIBRESONIC_PASSWORD")?,
        ),
        Err(_) => {
            return Err(Error::InvalidConfiguration(
                "invalid API key environment variable",
            ));
        }
    };
    let client = Client::new(
        ServerId::new(env::var("LIBRESONIC_SERVER_ID").unwrap_or_else(|_| "library-probe".into()))?,
        &required("LIBRESONIC_URL")?,
        credentials,
        ClientOptions::default().allow_http(allow_http),
    )?;
    let service: &dyn MusicService = &client;
    let capabilities = service.connect().await?;
    println!(
        "Connected; {} advertised extensions",
        capabilities.extensions.len()
    );
    let artists = service.artists().await?;
    println!("{} artists", artists.len());
    for artist in artists.iter().take(10) {
        println!("  {}", artist.name);
    }
    let mut page = PageRequest::new(0, 10)?;
    let mut first_album = None;
    let mut album_count = 0;
    loop {
        let albums = service.albums(AlbumOrder::Title, page).await?;
        if first_album.is_none() {
            first_album = albums.items.first().cloned();
        }
        for album in &albums.items {
            println!("Album: {}", album.title);
        }
        album_count += albums.items.len();
        let Some(next) = albums.next else { break };
        if next.offset() > 10_000 {
            return Err(Error::InvalidConfiguration(
                "probe stops after 10000 album slots",
            ));
        }
        page = next;
    }
    println!("{album_count} albums fetched");
    if let Some(album) = first_album {
        let album = service.album(&album.id).await?;
        println!(
            "Opened {} ({} tracks)",
            album.album.title,
            album.tracks.len()
        );
        if let Some(track) = album.tracks.first() {
            let track = service.song(&track.id).await?;
            let results = service
                .search(&track.title, PageRequest::new(0, 10)?)
                .await?;
            println!(
                "Track: {}; search returned {} tracks",
                track.title,
                results.tracks.items.len()
            );
        }
        if let Some(artwork) = album.album.artwork {
            let bytes = service.artwork(&artwork, Some(128)).await?;
            println!("Fetched {} artwork bytes", bytes.len());
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> ExitCode {
    match browse().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
