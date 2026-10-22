# Libresonic agent guide

Read `README.md` and the relevant workspace crate before editing. This repository
builds independently; it has no checkout-location requirement. Application
architecture decisions are maintained in the public
[Sonicfast design documents](https://github.com/1800alex/sonicfast/tree/main/docs/design).
The library owns neutral models, OpenSubsonic transport, decoder/queue/scrobble
state and PCM output contracts. Sonicfast owns native devices, UI, protected
credential persistence, filesystem downloads, EQ and visualization.

- Keep changes focused on domain, protocol and local playback. Casting and
  multiroom require a separate design.
- Keep IDs server-scoped and models independent of wire DTOs. Credentials and
  authenticated URLs stay inside the protocol adapter.
- No egui, librespot, Spotify or Sonicfast dependency. Native device/EQ/
  visualization integration belongs in the app.
- HTTP needs deadlines, cancellation and disabled authenticated redirects.
  Treat whole authenticated URLs and arbitrary server bodies as secrets.
  Native secret persistence belongs in the app, never ordinary library JSON.
- Test meaningful behavior with mock servers and opt-in synthetic media.
  Document limits and actual platform/server coverage.
- Run fmt, clippy with warnings denied, tests and docs for implemented APIs.
- Use this checkout's `target/` and ignored `.cache/`; never share targets or
  put build output in `/tmp`.
- Do not commit, push or publish unless requested.
