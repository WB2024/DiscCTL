# Test Fixtures

`audio/` — three 5-second 44100Hz 16-bit stereo sine-wave WAV files, generated with:

```bash
for i in 01 02 03; do
  ffmpeg -f lavfi -i "sine=frequency=$((200 * $i)):duration=5" \
    -ar 44100 -ac 2 -sample_fmt s16 audio/track${i}.wav
done
```

`data/` — minimal data directory for data-session burn tests.
