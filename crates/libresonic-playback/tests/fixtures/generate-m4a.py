"""Regenerate synthetic M4A fixtures; requires ffmpeg, never personal audio."""
from pathlib import Path
import struct
import subprocess


def main():
    for codec in ["aac", "alac"]:
        for layout in ["fast", "tail"]:
            path = Path(__file__).parent / f"tone-{codec}-{layout}.m4a"
            args = ["ffmpeg", "-v", "error", "-y", "-f", "lavfi", "-i",
                    "sine=frequency=880:sample_rate=44100:duration=2", "-ac", "2",
                    "-c:a", codec, "-map_metadata", "-1"]
            if codec == "aac":
                args += ["-b:a", "128k"]
            if layout == "fast":
                args += ["-movflags", "+faststart"]
            subprocess.run(args + [str(path)], check=True)
            if layout == "tail":
                data = path.read_bytes()
                offset = 0
                while data[offset + 4:offset + 8] != b"moov":
                    size = int.from_bytes(data[offset:offset + 4], "big")
                    assert size >= 8 and offset + size <= len(data)
                    offset += size
                # A free atom moves tail metadata beyond the 64 KiB rewind
                # cache. Audio offsets stay valid because mdat precedes it.
                size = 128 * 1024
                padding = struct.pack(">I4s", size, b"free") + bytes(size - 8)
                path.write_bytes(data[:offset] + padding + data[offset:])


if __name__ == "__main__":
    main()
