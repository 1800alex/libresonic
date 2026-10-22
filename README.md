# Libresonic

Reusable music models and an async OpenSubsonic service for
[Sonicfast](https://github.com/1800alex/sonicfast). Connection/browsing and bounded single-track MP3/FLAC/M4A
playback are implemented. The API is pre-1.0 and may change.

This repository builds independently. Sonicfast consumes a pinned Git revision,
so neither repository requires a particular checkout directory. The release
workflow publishes all four workspace crates together. Application integration decisions are recorded
in the [Sonicfast design documents](https://github.com/1800alex/sonicfast/tree/main/docs/design).

The root facade reexports `libresonic-core` (server/kind-scoped IDs, neutral
models, safe errors, capabilities and object-safe `MusicService`) and
`libresonic-opensubsonic` (the client) plus `libresonic-playback` (neutral PCM
output, decoder, transport and latest-state API). Protocol DTOs and authenticated URLs
stay private. Construct and use the client on the application's backend runtime.

Implemented reads: authenticated ping and optional extension discovery, artists,
artist albums, paginated album lists, album tracks, single songs, paginated
search and artwork bytes. Capability support starts unknown and is learned from
actual requests. Missing legacy discovery does not disable baseline browsing.

Song hearts support favorite-song reads and bounded star/unstar writes. Personal
song ratings (1–5, or zero to clear) are independent of hearts; metadata retains
valid personal ratings and defaults older saved queue records to unknown.
See [ADR 007](https://github.com/1800alex/sonicfast/blob/main/docs/design/adr-007-song-feedback-and-lyrics.md).

Original-file downloads expose bounded nonseekable audio streams without
transcoding; filenames, original byte sizes and play counts are optional neutral
metadata. Scoped song scrobbles support now-playing and timestamped submissions,
using advertised POST or bounded legacy GET without automatic retries.
`playback::scrobble` provides opt-in listened-time thresholds and occurrence state;
the application owns scheduling, receipts and filesystem/download policy. See
[ADR 008](https://github.com/1800alex/sonicfast/blob/main/docs/design/adr-008-scrobbling-and-downloads.md) and the
[verification report](https://github.com/1800alex/sonicfast/blob/main/docs/design/scrobbling-and-downloads.md).

Server playlists support list/details, creation, metadata updates, appending,
exact-position removal, ordered content replacement and deletion. Duplicates and
unavailable entry positions are preserved. Ownership and editability are
separate evidence. Advertised API-key `tokenInfo` can verify account identity.
Writes use form POST only when advertised, otherwise bounded GET; they are never
automatically retried. Mutation batches are limited to 2,000 songs/rows and
1 MiB of encoded form data; legacy GET requests must fit within 8 KiB. See
[ADR 006](https://github.com/1800alex/sonicfast/blob/main/docs/design/adr-006-server-playlists.md).

Requests have 5s connect/20s whole-request defaults, disabled redirects/retries,
and byte caps (4 MiB ordinary JSON, 32 MiB for the unpaginated `getArtists`
index, 16 MiB artwork). `response_limits` overrides all JSON budgets;
`artist_index_limit` can override the full index separately. Limit errors identify
the endpoint and budget without exposing URLs or response bodies.
Audio streams instead use connect/
header/individual-read deadlines, so long songs and paused consumers remain valid.
Dropping a request future cancels its
work. Certificate verification is enabled. HTTP requires explicit `allow_http`.
Credentials are kept only in memory and have redacted Debug. Errors discard raw
URLs, server messages and response snippets; native credential persistence
belongs to the application.

The adapter uses the same reqwest 0.12/Rustls family as Sonicfast. The evaluated
opensubsonic 0.6.1 client could not enforce bounded reads and enabled a second
crypto provider incompatible with the retained engine's implicit provider path;
see ADR 002. Only the needed endpoint subset is implemented, using reqwest, serde,
and standard Subsonic token signing, without a new HTTP stack or copied source.

## Try a server

The [library example](examples/library.rs) lists artists/albums, opens an album,
reads a song, searches and fetches artwork. It requires a Tokio runtime (provided
by the example). It stores no credentials or metadata and accepts no secret
command-line arguments:

```sh
export LIBRESONIC_URL='https://music.example.com'
export LIBRESONIC_USERNAME='alice'
read -r -s -p 'Server password: ' LIBRESONIC_PASSWORD
export LIBRESONIC_PASSWORD
cargo run --locked --example library
unset LIBRESONIC_PASSWORD
```

For API-key auth, set `LIBRESONIC_API_KEY` instead of username/password. Its
presence selects API-key auth; failures never fall back to a password. Optional
`LIBRESONIC_SERVER_ID` is a local profile namespace (default `library-probe`),
not a URL or credential. For an explicitly trusted HTTP server, add
`-- --allow-http` to the command. The probe stops after 10,000 album slots.

## Playback

Supply an `Arc<dyn StreamProvider>` (the OpenSubsonic client implements it), a
boxed `playback::AudioOutput` and the application's Tokio runtime handle to
`playback::Player::spawn`. Output construction must defer device work to the
feeder. Use `load`, `play`, `pause`, `seek`, `stop`, `set_volume`, `snapshot` and
`subscribe` for local transport; `shutdown().await` cancels and joins the worker.
`playback::queue::Queue` owns context/manual occurrences, previous history,
repeat/shuffle and serializable resume state; repeated IDs stay separate.
Restoration policy belongs to the app (Sonicfast always restores paused).
The runtime must remain alive until shutdown. There is one selected track and
no automatic queue advancement. The public output contract receives bounded,
finite source PCM; the app converts it to its processing/device formats.

The encoded channel is bounded to 16 chunks of at most 16 KiB; the public peak
counter budget is 288 KiB including reader/in-flight bytes. This excludes the
HTTP retained chunk (at most 512 KiB), decoder lookahead/metadata, PCM and native buffers; see
[ADR 003](https://github.com/1800alex/sonicfast/blob/main/docs/design/adr-003-streamed-playback.md). Watch snapshots
replace old state. Loads/seeks cancel superseded generations; errors are safe,
recoverable values. Seek timing grows with target position and network speed.
No audio-device or GUI dependency enters this workspace.

M4A supports DRM-free AAC-LC and ALAC via the existing Symphonia family. Core
streams can optionally expose byte length and cancellable absolute byte seeks.
The protocol adapter validates authenticated HTTP byte ranges before allowing
MP4 container parsing to seek to tail metadata. Fast-start M4A can stream without
ranges. Seekable M4A uses one bounded input read instead of the encoded channel;
container metadata/sample tables are outside the input counter. See
[ADR 005](https://github.com/1800alex/sonicfast/blob/main/docs/design/adr-005-m4a-playback.md). Time seeks still
reopen/decode forward; byte seeks here do not optimize seeking by playback time.

## Validation

```sh
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace --all-targets
cargo test --locked --workspace --doc
RUSTDOCFLAGS='-D warnings' cargo doc --locked --workspace --no-deps
cargo publish --workspace --locked --dry-run --registry crates-io
```

## Releasing

All four crates inherit `workspace.package.version` and publishing settings from
the root manifest. Internal dependencies are defined once in
`workspace.dependencies`, with both a local path and a crates.io version
requirement. Cargo uses the paths during development and the versions when
packaging for publication.

Before releasing, update the shared version and keep the three internal version
requirements compatible with it. Commit the changes and tag that commit
`v<version>`. The release workflow checks that the tag matches the manifest,
performs a workspace publish dry-run, then runs
`cargo publish --workspace --locked --registry crates-io` using the repository's
`CARGO_REGISTRY_TOKEN` secret. Cargo handles publishing dependencies before their
dependents and waits for registry availability. A plain `cargo publish` at the
workspace root selects only the facade crate.

The CI dry-run checks all four packaged crates without uploading them. When
checking uncommitted local changes, add `--allow-dirty` to the dry-run command.
The release workflow always publishes from a clean tagged checkout. Cargo 1.90
or newer supports workspace publishing; the declared Rust minimum is 1.98.

## Validation coverage

To explicitly choose another installed toolchain or Cargo cache, use
`CARGO_HOME="$PWD/.cache/cargo" cargo +stable ...`. Current
validation uses Linux/Rust 1.99.1; the declared Rust 1.98 minimum remains untested.

Mock tests exercise real HTTP signing, sparse metadata, paging, legacy discovery,
safe failures, redirects, response bounds, deadlines and cancellation. The
[Navidrome fixture](integration/navidrome/README.md) generates its own synthetic
tones and artwork with FFmpeg. `make -C integration/navidrome test-rust` runs the
opt-in Rust browse
test after fixture readiness and always tears down its own stack. To override
the toolchain/cache, set an absolute `CARGO_HOME` and `CARGO="cargo +stable"`.

Current limits: JSON protocol envelopes only, direct connections (app proxy
integration is later), no persistent credentials. Playback supports direct MP3/FLAC/M4A (AAC-LC/ALAC), mono/stereo
at 8..192 kHz; time seeks reopen/decode forward with linear cost. The
application owns devices, rate/channel conversion, EQ and visualizers. Random
album selection has no continuation
hint because its order is unstable. Artwork requires an image Content-Type.
HTTP Retry-After supports seconds, not dates. A second real server, other OSes
and physical hardware remain unverified. See the
[Phase 2 report](https://github.com/1800alex/sonicfast/blob/main/docs/design/phase-2.md) and
[native harness](https://github.com/1800alex/sonicfast/blob/main/examples/subsonic_playback.rs) for the Linux
PulseAudio capture proof and commands.
