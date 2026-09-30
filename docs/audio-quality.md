# Audio quality

[← Back to the README](../README.md)

RustyDisc lets you choose how a rip is encoded, tells you what you actually got, and gives you the tools to check it.

## Choosing the quality

On the **Rip** page (and as a default in **Settings → Rip defaults**), pick a format and a quality. The first choice for each format is the best, and it's the default.

| Format | Choices |
|---|---|
| **FLAC** (lossless) | Best compression (level 8, default) · maximum (12) · standard (5) · fastest (0). Every level is bit-perfect; only size and speed change. |
| **ALAC**, **WAV**, **AIFF** (lossless) | One choice each: the disc's audio exactly as read, 16-bit / 44.1 kHz. |
| **MP3** | V0 (~245 kbps VBR, default) · 320 kbps · V2 (~190 kbps) · 192 kbps · 128 kbps |
| **AAC** | 320 · 256 · 192 · 128 kbps |
| **Opus** | 320 (default) · 192 · 128 · 96 kbps |
| **OGG Vorbis** | Quality 10 (~500 kbps, default) · 8 · 6 · 4 |

A CD holds 16-bit / 44.1 kHz stereo audio, so a lossless rip is the best that exists: there's nothing more to capture. From the command line: `rustydisc rip --format flac --quality 12`, `--format mp3 --quality cbr320`, `--format aac --quality 256`.

## What did I get?

Right after a rip, the job shows a **Quality** line: the format, bit depth, sample rate, channels and average bitrate, with a **lossless · CD quality** badge when it matches the disc.

In the **Library**, every rip has an **Audio quality** card:

- **Format, bit depth, sample rate and bitrate** for the set and for each file, with notes when files differ or aren't CD quality.
- **Test the audio** decodes every file end to end and reports any that don't decode cleanly (a damaged download, a bad disk sector).
- **Measure loudness** shows each track's integrated loudness (LUFS), loudness range and true peak (flagging any that clip), and the album's. **Write ReplayGain tags** adds standard ReplayGain 2.0 track and album gain and peak to the files, so players can level tracks and albums without touching the audio. You can also switch this on for every rip in Settings.
- **Spectrogram** draws any track's frequencies over time. Real CD audio fills the picture up to 20 kHz and beyond; a sharp ceiling around 16 kHz means the "lossless" file was once an MP3 (or similar) and only the wrapper is lossless.

RustyDisc's own numbers come from `ffprobe` and `ffmpeg` (EBU R128 loudness), the same tools that encode the audio.

## Dynamic range (DR)

The [Dynamic Range Database](https://dr.loudness-war.info/) lists a "DR" number for albums: how far the peaks rise above the loud parts of the music. A high number (14 and up) is natural and dynamic; a low one (under 8) is a loudness-war master squashed to be as loud as possible. Different pressings of the same album often score very differently, so it's the number to check when choosing an edition.

RustyDisc measures it itself, with the published DR method (3-second blocks per channel, the second-highest peak against the RMS of the loudest 20% of blocks, averaged over the channels; an album's DR is the average of its tracks'). A value can differ from a listed one by a point, because this is a reimplementation of the meter, not the meter itself.

- **In the Library:** the **Dynamic range** button on the Audio quality card shows each track's DR, peak and RMS, the album's DR with what it means, and lets you **save DR as tags** (`DYNAMIC RANGE` and `ALBUM DYNAMIC RANGE`, the names the desktop DR meters use), **download a DR log** in the usual layout, and **compare on the Dynamic Range DB**, which opens the database's listing for that artist.
- **When ripping:** tick **Measure dynamic range (DR)** (or make it a default in Settings) and the job reports the album's DR and writes the tags. On the command line: `rustydisc rip --dynamic-range`.

The database has no public API or export, so RustyDisc links to it instead of reading it.
