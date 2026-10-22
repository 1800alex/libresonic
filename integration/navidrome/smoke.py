"""Validate the disposable server fixture, independently of the Rust adapter.

Never print authenticated URLs or raw HTTP/server errors. This intentionally
uses only the Python standard library and disposable fixture credentials.
"""

import argparse
import hashlib
import json
import secrets
import time
import urllib.error
import urllib.parse
import urllib.request


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--url", default="http://127.0.0.1:4535")
    args = parser.parse_args()
    base = urllib.parse.urlsplit(args.url)
    if (
        base.scheme != "http"
        or base.hostname not in ("127.0.0.1", "localhost", "::1")
        or base.username is not None
        or base.password is not None
        or base.query
        or base.fragment
    ):
        parser.error("This fixture smoke test requires a loopback HTTP URL.")
    opener = urllib.request.build_opener(NoRedirect(), urllib.request.ProxyHandler({}))

    def request(endpoint, params=None, password="admin", binary=False):
        salt = secrets.token_hex(8)
        query = {
            "u": "admin",
            "t": hashlib.md5((password + salt).encode(), usedforsecurity=False).hexdigest(),
            "s": salt,
            "v": "1.16.1",
            "c": "libresonic-fixture",
            "f": "json",
            **(params or {}),
        }
        url = args.url.rstrip("/") + "/rest/" + endpoint + ".view?"
        try:
            with opener.open(url + urllib.parse.urlencode(query), timeout=10) as response:
                body = response.read(2 * 1024 * 1024 + 1)
                if len(body) > 2 * 1024 * 1024:
                    raise RuntimeError("Fixture response exceeds 2 MiB budget")
                if binary:
                    if "json" in response.headers.get("Content-Type", "").lower():
                        raise RuntimeError("Fixture returned JSON instead of media")
                    return body
                return json.loads(body)["subsonic-response"]
        except (urllib.error.URLError, TimeoutError, OSError, ValueError, KeyError):
            raise RuntimeError(f"Fixture request failed: {endpoint}") from None

    def ok(endpoint, params=None):
        data = request(endpoint, params)
        if data.get("status") != "ok":
            raise RuntimeError(f"Fixture API failed: {endpoint}")
        return data

    deadline = time.monotonic() + 60
    while True:
        try:
            info = ok("ping")
            break
        except RuntimeError:
            if time.monotonic() >= deadline:
                raise RuntimeError("Fixture did not become ready within 60 seconds") from None
            time.sleep(1)
    print("PASS authenticated ping")
    invalid = request("ping", password="invalid-fixture-password")
    if invalid.get("status") != "failed" or invalid.get("error", {}).get("code") != 40:
        raise RuntimeError("Fixture failed to reject invalid credentials")
    print("PASS invalid credentials")
    # Authentication is available before the initial library scan completes.
    deadline = time.monotonic() + 60
    while True:
        scan = ok("getScanStatus").get("scanStatus", {})
        if not scan.get("scanning", True) and scan.get("count", 0) >= 264:
            break
        if time.monotonic() >= deadline:
            raise RuntimeError("Fixture did not scan 264 songs within 60 seconds")
        time.sleep(1)
    print("PASS initial scan (264 or more songs)")
    extensions = ok("getOpenSubsonicExtensions").get("openSubsonicExtensions", [])
    artists = ok("getArtists")["artists"].get("index", [])
    if not any(index.get("artist") for index in artists):
        raise RuntimeError("Fixture has no artists")
    print("PASS artists and extension discovery")

    albums = []
    offset = 0
    while True:
        page = ok("getAlbumList2", {"type": "alphabeticalByName", "size": 10, "offset": offset})
        page = page.get("albumList2", {}).get("album", [])
        albums.extend(page)
        if len(page) < 10:
            break
        offset += len(page)
        if offset > 1000:
            raise RuntimeError("Unexpected fixture pagination size")
    if len(albums) < 20:
        raise RuntimeError("Expected the multi-page synthetic album library")
    print(f"PASS album pagination ({len(albums)} albums)")
    formats = set()
    covers = 0
    for album in albums:
        detail = ok("getAlbum", {"id": album["id"]})["album"]
        if covers == 0 and detail.get("coverArt"):
            if not request("getCoverArt", {"id": detail["coverArt"], "size": 64}, binary=True):
                raise RuntimeError("Fixture artwork was empty")
            covers += 1
        for song in detail.get("song", []):
            suffix = song.get("suffix")
            if suffix not in ("mp3", "flac", "m4a") or suffix in formats:
                continue
            ok("getSong", {"id": song["id"]})
            if not request("stream", {"id": song["id"], "format": "raw"}, binary=True):
                raise RuntimeError("Fixture stream was empty")
            results = ok("search3", {"query": song["title"], "songCount": 5})
            if not results.get("searchResult3", {}).get("song"):
                raise RuntimeError("Fixture search returned no songs")
            formats.add(suffix)
    if formats != {"mp3", "flac", "m4a"} or covers == 0:
        raise RuntimeError("Fixture lacks required MP3, FLAC, M4A or artwork")
    print("PASS album/song mapping, search, artwork, MP3, FLAC and M4A stream bytes")
    # Only safe server identity metadata is printed. Never dump the envelope.
    identity = {key: info.get(key) for key in ("type", "serverVersion", "version")}
    print(json.dumps({"server": identity, "extension_count": len(extensions)}))


if __name__ == "__main__":
    try:
        main()
    except RuntimeError as error:
        raise SystemExit(str(error)) from None
