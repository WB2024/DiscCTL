# Command-line reference

Every command and flag.

[← Back to the README](../README.md)

---

# Quick Start

```bash
# What's on the disc?
rustydisc info

# Rip a disc to FLAC — folder named automatically from MusicBrainz metadata
rustydisc rip --format flac --dir ~/rips

# Rip with full archive (checksums, metadata, disc graph, MusicBrainz JSON)
rustydisc rip --format flac --dir ~/rips --archive

# Burn a set of WAV files as a Red Book audio CD
rustydisc burn --format redbook --audio ~/music/album/*.wav --label "My Album"

# Burn a directory as a data CD
rustydisc burn --format datacd --data ~/files/backup --label "Backup 2026"

# Burn from a playlist
rustydisc burn --format redbook --playlist "Smooch Hits.m3u8" --label "Smooch Hits"

# Preview a burn plan without touching the drive
rustydisc plan --format redbook --audio ~/music/album/*.wav --label "My Album"
```

---


# Commands

## `rustydisc info`

Reads the disc in the drive and reports its format, session layout, track list, durations, data session sizes, CD-Text, and MusicBrainz DiscID — without modifying anything.

```bash
rustydisc info
rustydisc info --device /dev/sr1
rustydisc info --json
```

| Flag | Description |
|------|-------------|
| `--device <dev>` | Drive to inspect (default: `/dev/sr0`) |
| `--json` | Output raw JSON (DiscInfo struct) instead of formatted text |

**Example output:**
```
Disc Type:  Red Book Audio CD
Sessions:   1
DiscID:     8yJcG2R53kN7I374FYHtRiMpPes-

Session 1 — Audio  (20 tracks, 65:02)  «Bona Drag» — Morrissey
  Track  1   3:47  "Piccadilly Palare"
  Track  2   3:19  "Interesting Drug"
  ...
```

---

## `rustydisc burn`

Burns a disc from command-line flags or a JSON disc graph.

```
rustydisc burn [OPTIONS]
```

### Input flags

| Flag | Description |
|------|-------------|
| `--format <fmt>` | Disc format: `redbook`, `datacd`, `bluebook`, `datadvd`, `musicdvd` (default: `redbook`) |
| `--audio <files...>` | Audio track files or glob patterns (WAV/FLAC/MP3/M4A/OGG etc.) |
| `--playlist <file>` | M3U or M3U8 playlist of audio tracks (Audio / Enhanced CD) or files (Data CD) |
| `--data <dir>` | Data CD: the folder to burn (sub-folders are kept). Enhanced CD: the folder for the data session |
| `--files <files...>` | Data CD only: individual files, placed in the root of the disc |
| `--label <text>` | Disc volume label (default: `Untitled`) |
| `--input <file>` | Load a disc graph JSON instead of building from flags |

### Behaviour flags

| Flag | Description |
|------|-------------|
| `--device <dev>` | Target drive device (default: `/dev/sr0`) |
| `--dry-run` | Print the burn plan as JSON — do not burn |
| `--debug` | Print backend commands and verbose output |
| `--cd-text` | Read CD-Text (title, artist) from embedded audio file tags |
| `--progress-json` | Emit machine-readable JSON progress events to stdout |

### Transcoding flags

| Flag | Description |
|------|-------------|
| `--transcode <spec>` | Data CD: convert audio before burning: `mp3:256`, `aac:320`, `opus:192`, `flac`, `wav` (only files that would shrink are converted) |
| `--normalize <off\|album\|track>` | Level the audio before burning an audio CD (default off; it changes the audio). See [Levelling the audio](hardware-notes.md#levelling-the-audio-before-burning-normalization) |
| `--normalize-target <LUFS>` | Loudness to aim for when normalizing (default -14) |
| `--speed <auto\|N>` | Write speed: `auto` (the drive chooses, the default) or a multiple such as `8` for 8x. See [Write speed](hardware-notes.md#write-speed) |
| `--disc-size <size>` | Blank disc: a size in MB or a name — `cd650`, `cd700` (default for CDs), `cd800`, `dvd` (default for DVDs), `dvd-dl`, `bd`, `bd-dl` |
| `--dvd-audio-kbps <n>` | Music DVD: Dolby Digital bitrate: 192, 256, 384 or 448 (default) |
| `--dvd-standard <pal\|ntsc>` | Music DVD: picture standard (default `pal`) |
| `--dvd-still <image>` | Music DVD: picture shown while the music plays (default: cover art beside the tracks) |
| `--iso-out <file>` | DVD formats: save the disc image to a file instead of burning |
| `--stage-dir <dir>` | Where to write converted files, one disc at a time (default: `/tmp`) |
| `--keep-staged` | Keep staged files after burn (default: delete on exit) |

---

## `rustydisc rip`

Rips the disc in the drive to files. Auto-detects the disc format and handles Red Book audio, Data CD, and Blue Book appropriately.

After detecting the disc, RustyDisc automatically:
1. Computes the **MusicBrainz DiscID** from the TOC
2. Queries the **MusicBrainz API** for album title, artist, year, and track titles
3. Downloads **cover art** from the Cover Art Archive
4. Rips audio securely with **cdparanoia**
5. Checks each track against the **AccurateRip** database
6. Encodes to the requested format and **embeds all metadata and cover art** into each file

```
rustydisc rip [OPTIONS]
```

| Flag | Description |
|------|-------------|
| `--device <dev>` | Drive to rip from (default: `/dev/sr0`) |
| `--dir <base-dir>` | **Recommended.** Base directory — a subfolder named `Artist - Album (Year)` is auto-created from metadata |
| `--output <dir>` | Explicit output path — use this exact directory name |
| `--format <fmt>` | Audio format: `wav`, `flac`, `alac`, `aiff`, `ogg`, `mp3`, `opus`, `aac` (default: `flac`) |
| `--quality <q>` | Encoder quality, best by default: FLAC `0`–`12` (level), MP3 `v0`/`v2`/`cbr320`/`cbr192`/`cbr128`, AAC and Opus a bitrate in kbps, OGG `4`–`10`. See [Audio quality](audio-quality.md) |
| `--replaygain` | Measure loudness after ripping and write ReplayGain 2.0 tags |
| `--dynamic-range` | Measure dynamic range (DR) after ripping and write DR tags |
| `--hidden-track <auto\|skip>` | Audio hidden before track 1: auto (rip it as track 00 when the disc has some and it isn't silence, the default) or skip |
| `--gaps <off\|report\|own-track>` | Gaps between tracks: off (default), report (scan and note them; about 5 minutes) or own-track (scan, and move each gap to the start of the track it leads into). See [Hidden tracks and gaps](audio-quality.md#hidden-tracks-and-gaps-between-tracks) |
| `--paranoia <full\|fast\|off>` | How hard to check what the drive reads: full (default), fast (overlap checking only) or off. See [How cleanly the disc was read](audio-quality.md#how-cleanly-the-disc-was-read) |
| `--offset <off\|auto\|N>` | Drive read offset: leave the audio as read (default), correct by the shift AccurateRip proves, or by N samples. See [Read offset correction](audio-quality.md#read-offset-correction) |
| `--cover-file <file>` | Use this JPEG or PNG as the cover instead of looking one up |
| `--archive` | Archive mode: store in `audio/` + `metadata/` subdirs; add `musicbrainz.json` + `checksums.json` |
| `--mb-release <id\|url>` | Use this MusicBrainz release for tags, cover art and folder name instead of the DiscID lookup — see [Choosing the MusicBrainz release](#choosing-the-musicbrainz-release) |
| `--no-musicbrainz` | Skip MusicBrainz lookup (for offline use or discs not in the database) |
| `--cover-sources <list>` | Where to get cover art, best first: `fanart`, `caa` (Cover Art Archive). Default `caa`, e.g. `fanart,caa` |
| `--fanart-key <key>` | fanart.tv API key for the `fanart` source (or set `RUSTYDISC_FANART_KEY`, which keeps it off the command line) |
| `--no-cover-file` | Don't save `cover.jpg` / `cover.png` next to the tracks |
| `--no-cover-embed` | Don't embed the cover art in the audio files |
| `--no-accuraterip` | Skip the AccurateRip database check (for offline use) |
| `--debug` | Verbose output including MusicBrainz and AccurateRip URLs, ffmpeg commands |
| `--progress-json` | Emit machine-readable JSON progress events to stdout |

**Audio formats:**

| Format | Type | Notes |
|--------|------|-------|
| `wav` | Lossless | Raw CDDA PCM — no encoding step, fastest |
| `flac` | Lossless | FLAC compression level 8 — recommended |
| `alac` | Lossless | Apple Lossless, `.m4a` container |
| `aiff` | Lossless | Big-endian PCM, archival format |
| `ogg` | Lossy | OGG Vorbis quality 10 (~500 kbps) |
| `mp3` | Lossy | LAME VBR quality 0 (highest) |
| `opus` | Lossy | Opus 320 kbps |

**Default output layout (flat):**
```
Artist - Album (Year)/
  cover.jpg                                       ← front cover from Cover Art Archive
  01. Artist - Album - Track Title.flac
  02. Artist - Album - Track Title.flac
  ...
```

**Archive output layout (`--archive`):**
```
Artist - Album (Year)/
  cover.jpg
  audio/
    01. Artist - Album - Track Title.flac
    ...
  metadata/
    disc.json           ← full TOC, session layout, track LBAs, DiscID
    cdtext.json         ← CD-Text in structured JSON (if present on disc)
    musicbrainz.json    ← full MusicBrainz release data (if found)
    accuraterip.json    ← per-track AccurateRip result (if the disc is in the database)
    checksums.json      ← SHA256 + byte count per file
```

### Cover art sources

```bash
# fanart.tv first, Cover Art Archive as the fallback; embed only (no cover.jpg left behind)
RUSTYDISC_FANART_KEY=your-key rustydisc rip --dir ~/rips --cover-sources fanart,caa --no-cover-file
```

Sources are tried in the order given and the first one with an image wins. fanart.tv needs a free personal API key ([get one here](https://fanart.tv/get-an-api-key/)) and finds albums by their MusicBrainz release group, so it works when the disc was matched (or chosen) on MusicBrainz. If the key is missing or rejected, that source is skipped and the next one is used. Embedding is supported for FLAC, MP3, ALAC and OGG; AIFF, Opus and WAV files can't carry embedded art, so use the cover file for those.

**Metadata priority:** MusicBrainz > CD-Text > auto-generated defaults.

### Choosing the MusicBrainz release

Discs are matched by DiscID, but plenty of pressings aren't attached to their MusicBrainz release yet (and some DiscIDs match the wrong edition). When that happens, look the release up on musicbrainz.org yourself and hand RustyDisc its ID or URL:

```bash
rustydisc rip --dir ~/rips --mb-release https://musicbrainz.org/release/bc8d517f-6ce0-4e45-b6d8-af0f29cdd1ea
rustydisc rip --dir ~/rips --mb-release bc8d517f-6ce0-4e45-b6d8-af0f29cdd1ea
```

The release's title, artist, year, track titles, MusicBrainz IDs and cover art are used exactly as if the DiscID lookup had found it. For multi-disc releases the matching disc is chosen by DiscID, or by track count. If the release's track count doesn't match the disc you get a warning, and if you named a release that doesn't exist (or a multi-disc release with no matching disc) the rip stops before reading the disc rather than silently falling back. In the web UI, open *Use a MusicBrainz release I've found* on the Rip page, then either paste the ID/URL or click **Find…** to search MusicBrainz and browse the candidates before ripping.

**Cover art embedding** is supported for FLAC, ALAC, MP3, and OGG Vorbis. The cover is also always saved as `cover.jpg` / `cover.png` in the output directory regardless of format.

**MusicBrainz tags embedded per file:**

| Tag field | Content |
|-----------|---------|
| `title` | Track title |
| `artist` | Track artist (or album artist if same for all tracks) |
| `album` | Album title |
| `album_artist` | Album artist |
| `date` | Release year |
| `track` | Track number / total |
| `MUSICBRAINZ_TRACKID` | MB recording ID |
| `MUSICBRAINZ_ALBUMID` | MB release ID |
| `MUSICBRAINZ_ARTISTID` | MB artist ID |

---

## `rustydisc verify`

Verifies a ripped archive against its `checksums.json`. Checks every file's SHA256 hash and byte count.

```bash
rustydisc verify ~/rips/my_album
```

| Argument | Description |
|----------|-------------|
| `<directory>` | Root directory of the ripped archive (must contain `metadata/checksums.json`) |

**Example output:**
```
Verifying archive: /home/user/rips/my_album
Passed: 12
OK — all 12 files verified.
```

```
Verifying archive: /home/user/rips/my_album
FAILED (1):
  ✗  audio/03 - Track Title.flac
Passed: 11
```

Exit code `0` on success, `1` on failure (with structured JSON error on stderr).

---

## `rustydisc plan`

Prints the burn plan as JSON without writing to any device: how many discs are needed, what goes on each and how full it is (counting the converted size when `--transcode` is given), plus the burn steps for a single disc. Useful for scripting and verifying disc layout before committing to media. It accepts the same source flags as `burn` (`--audio`, `--playlist`, `--data`, `--files`, `--transcode`, `--disc-size`), and `--speed` to show the chosen write speed in the plan.

```
rustydisc plan --format redbook --audio ~/music/*.wav --label "Preview"
rustydisc plan --format datacd --data ~/Music --transcode mp3:320 --disc-size 700
rustydisc plan --input disc.json
```

---

## `rustydisc validate`

Validates a disc graph JSON file for correctness — format rules, WAV spec, ISO size, session ordering — without touching hardware.

```
rustydisc validate disc.json
```

Returns exit code `0` on success, `1` on validation failure (with a structured JSON error on stderr).

---

## `rustydisc recover`

Inspects the disc in the drive and attempts to recover from an interrupted burn.

```
rustydisc recover --device /dev/sr0
rustydisc recover --device /dev/sr0 --blank fast
rustydisc recover --device /dev/sr0 --blank full
```

| Flag | Description |
|------|-------------|
| `--device <dev>` | Target drive (default: `/dev/sr0`) |
| `--blank <mode>` | Blank a CD-RW: `fast` (quick erase) or `full` (complete erase) |

