# Generated decoder fixtures

These are synthetic two-second tones, with no personal or third-party recording:

```sh
ffmpeg -f lavfi -i sine=frequency=440:sample_rate=44100:duration=2 -ac 2 -codec:a libmp3lame -b:a 128k -map_metadata -1 tone.mp3
ffmpeg -f lavfi -i sine=frequency=660:sample_rate=48000:duration=2 -ac 1 -codec:a flac -map_metadata -1 tone.flac
```

They exercise MP3 stereo/gapless trim and FLAC mono/48 kHz source format. Native
output tests convert that format through Sonicfast's retained resampler/EQ/tap.

`python3 generate-m4a.py` regenerates the AAC-LC and ALAC M4A files. Both are
two-second, 44.1 kHz stereo synthetic 880 Hz tones. The `fast` variants put
metadata before audio; `tail` variants insert a valid 128 KiB `free` atom before
the final `moov`. This forces container byte seeks beyond the decoder's 64 KiB
rewind buffer while keeping audio offsets intact. It does not change decoded
audio. No ffmpeg runtime dependency is needed to run Cargo tests.
