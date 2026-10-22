# Disposable Navidrome fixture

This self-contained fixture generates original test tones and PNG artwork, then
runs pinned Navidrome 0.64.0 on loopback port 4535. It needs no other source
repository, personal music or server database.

Install Python 3, FFmpeg (MP3/FLAC/AAC encoders), Make and Docker with Compose.
On Debian/Ubuntu, the media prerequisite is `sudo apt install ffmpeg`.
From this directory:

```sh
make library    # Generate 264 tracks across 8 artists and 24 albums
make up         # Generate if needed, then start the disposable server
make smoke      # Wait for scanning and validate browse/search/artwork/streaming
make down       # Remove this fixture's container, network and database volume
make test       # up + smoke + guaranteed teardown
make test-rust  # up + smoke + Rust service test + guaranteed teardown
```

Use `DOCKER_COMPOSE="docker compose"` for the Docker CLI plugin, or the default
standalone `docker-compose`. Set `NAVIDROME_PORT` to avoid conflicts. The Compose
project is `libresonic-navidrome`; its credentials are disposable `admin` / `admin`.
HTTP is deliberate and limited to loopback. The smoke runner disables redirects
and proxies and never prints authenticated URLs or arbitrary server errors.

`prepare.py` uses FFmpeg and Python's standard library to generate three album
formats per artist: MP3, FLAC and fast-start AAC/M4A, 88 tracks of each format.
Every album has 11 tagged tracks lasting 3–7 seconds and original cover artwork.
M3U playlists cover ordinary selections, duplicate entries and the full library.
Generated media remains ignored in `.library/`; only the generator is tracked.
The script reuses its own marked fixture, refuses an unrelated existing output,
and publishes a new library only after generation completes. To regenerate,
move the existing generated folder aside first. `--output PATH` supports an
isolated output location for generator checks.

`make smoke` validates the fixture independently through Python. `make test-rust`
then runs the opt-in Rust `MusicService` test: authentication, artists, artist
albums, pagination, album/song/search/artwork and rejected credentials. Its test
is ignored in ordinary Cargo runs and requires explicit `LIBRESONIC_TEST_URL`.
To choose an installed toolchain/cache, set an absolute `CARGO_HOME` and
`CARGO="cargo +stable"`; targets pass them through.

Application playback and GUI proofs are maintained in
[Sonicfast's test instructions](https://github.com/1800alex/sonicfast/blob/main/docs/design/phase-3.md).
Keep this fixture running during those proofs and run `make down` afterward.
This fixture is not physical audio, cross-platform or second-server coverage.
