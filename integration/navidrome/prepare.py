"""Generate a self-contained synthetic music library using FFmpeg and stdlib."""

import argparse
import json
from pathlib import Path
import shutil
import struct
import subprocess
import tempfile
import zlib


ARTISTS = (
    "Sonic Echo", "Amber Current", "Binary Bloom", "Cobalt Waves",
    "Delta Lights", "Emerald Sky", "Fable Signal", "Golden Field",
)
TITLES = (
    "Lunar Path", "Sacred Fire", "Dark Surge", "Prism Pulse", "Golden Current",
    "Velvet River", "Stellar Stream", "Shadow Storm", "Crystal Field",
    "Silver Horizon", "Solar Bloom",
)
FORMATS = {
    "mp3": ["-c:a", "libmp3lame", "-b:a", "128k"],
    "flac": ["-c:a", "flac"],
    "m4a": ["-c:a", "aac", "-b:a", "96k", "-movflags", "+faststart"],
}
MARKER = ".libresonic-fixture.json"
PARAMETERS = {"generator": 1, "artists": 8, "albums": 24, "tracks": 264}


def slug(text):
    return text.lower().replace(" ", "_")


def cover(path, index):
    """Write original RGB PNG artwork without an image-library dependency."""
    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

    rows = bytearray()
    for y in range(128):
        rows.append(0)
        for x in range(128):
            rows.extend(((index * 37 + x) % 256, (index * 53 + y) % 256, 96 + (x + y) // 2))
    path.write_bytes(
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", 128, 128, 8, 2, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(rows))
        + chunk(b"IEND", b"")
    )


def ffmpeg(*args):
    subprocess.run(
        ["ffmpeg", "-nostdin", "-hide_banner", "-loglevel", "error", *map(str, args)],
        check=True,
    )


def generate(target):
    target = target.resolve()
    if target.exists():
        marker = target / MARKER
        if marker.is_file() and json.loads(marker.read_text()) == PARAMETERS:
            print("Using the existing generated fixture.")
            return
        raise ValueError("Output already exists and is not this generated fixture; choose a new directory.")
    if shutil.which("ffmpeg") is None:
        raise ValueError("Install FFmpeg to generate the synthetic fixture.")
    target.parent.mkdir(parents=True, exist_ok=True)
    staging = Path(tempfile.mkdtemp(prefix=".library-", dir=target.parent))
    try:
        templates = staging / "templates"
        templates.mkdir()
        for suffix, options in FORMATS.items():
            for seconds in range(3, 8):
                ffmpeg(
                    "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=44100",
                    "-t", seconds, "-ac", 2, *options, templates / f"{seconds}.{suffix}",
                )
        tracks = []
        for artist_index, artist in enumerate(ARTISTS):
            for album_index, suffix in enumerate(FORMATS):
                album = (
                    "Alpha Divide" if artist_index == album_index == 0
                    else f"Fixture Album {artist_index + 1:02d}-{album_index + 1:02d}"
                )
                directory = staging / slug(artist) / slug(album)
                directory.mkdir(parents=True)
                cover(directory / "cover.png", artist_index * 3 + album_index)
                for track_index, title in enumerate(TITLES, 1):
                    if track_index == 1 and album != "Alpha Divide":
                        title = "Signal Path"
                    destination = directory / f"{track_index:02d}_{slug(title)}.{suffix}"
                    ffmpeg(
                        "-i", templates / f"{3 + (track_index - 1) % 5}.{suffix}",
                        "-map", "0:a", "-c:a", "copy",
                        "-metadata", f"title={title}", "-metadata", f"artist={artist}",
                        "-metadata", f"album_artist={artist}", "-metadata", f"album={album}",
                        "-metadata", f"track={track_index}", "-metadata", "date=2024",
                        *(["-movflags", "+faststart"] if suffix == "m4a" else []),
                        destination,
                    )
                    tracks.append(destination.relative_to(staging).as_posix())
        shutil.rmtree(templates)
        playlists = staging / "playlists"
        playlists.mkdir()
        for name, selection in {
            "mixtape": tracks[:5],
            "duplicates": [tracks[0], tracks[0], tracks[1]],
            "favorites": tracks[::11],
            "everything": tracks,
        }.items():
            (playlists / f"{name}.m3u").write_text(
                "#EXTM3U\n" + "".join(f"../{track}\n" for track in selection)
            )
        (staging / MARKER).write_text(json.dumps(PARAMETERS, sort_keys=True) + "\n")
        staging.rename(target)
    finally:
        shutil.rmtree(staging, ignore_errors=True)
    print("Generated 264 original test tones, 24 albums, artwork and playlists.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=Path(__file__).parent / ".library")
    args = parser.parse_args()
    try:
        generate(args.output)
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"Fixture generation failed: {error}\n")


if __name__ == "__main__":
    main()
