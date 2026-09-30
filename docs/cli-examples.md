# Command-line examples

Worked examples for ripping, burning, archiving, playlists, transcoding and multi-disc jobs.

[← Back to the README](../README.md)

---

# Usage Examples

## Inspecting a Disc

```bash
# Human-readable summary (shows DiscID, CD-Text, session layout)
rustydisc info

# JSON output (for scripting)
rustydisc info --json | jq '.discid'
rustydisc info --json | jq '.sessions[0].tracks | length'
```

---

## Ripping Audio CDs

The simplest command — folder is named automatically from MusicBrainz:

```bash
rustydisc rip --format flac --dir ~/rips
# creates: ~/rips/Morrissey - Bona Drag (2010)/
#          cover.jpg + 20 FLAC files, all tagged and cover-embedded
```

Other formats:

```bash
# WAV (fastest — no encoding step)
rustydisc rip --format wav --dir ~/rips

# MP3
rustydisc rip --format mp3 --dir ~/rips

# ALAC (Apple ecosystem)
rustydisc rip --format alac --dir ~/rips

# Explicit output path (skip auto-naming)
rustydisc rip --format flac --output ~/rips/Morrissey\ -\ Bona\ Drag\ \(2010\)

# Offline — skip MusicBrainz, use CD-Text only
rustydisc rip --format flac --dir ~/rips --no-musicbrainz

# Verbose output
rustydisc rip --format flac --dir ~/rips --debug
```

Metadata source priority: **MusicBrainz** (if disc is found) → **CD-Text** (if encoded on disc) → track numbers only.

---

## Ripping Data CDs

```bash
# Extract filesystem as directory tree
rustydisc rip --output ~/rips/data_disc

# With debug output showing xorriso progress
rustydisc rip --output ~/rips/data_disc --debug
```

---

## Ripping Blue Book / CD Extra

Blue Book is handled automatically. Both sessions are detected and extracted:

```bash
rustydisc rip --format flac --dir ~/rips
```

Produces:
```
Artist - Album (Year)/
  cover.jpg
  audio/                  ← Session 1: FLAC files with full metadata + cover embedded
  data/                   ← Session 2: ISO filesystem contents
  metadata/
    disc.json
    cdtext.json
    musicbrainz.json
```

---

## Archive Mode

Archive mode adds checksums and a complete metadata set for long-term preservation:

```bash
rustydisc rip --format flac --dir ~/archive --archive
```

Produces:
```
Morrissey - Bona Drag (2010)/
  cover.jpg
  audio/
    01. Morrissey - Bona Drag - Piccadilly Palare.flac
    02. Morrissey - Bona Drag - Interesting Drug.flac
    ...
  metadata/
    disc.json           ← full DiscInfo: session layout, track LBAs, DiscID
    cdtext.json         ← CD-Text in structured JSON
    musicbrainz.json    ← full MusicBrainz release data
    checksums.json      ← SHA256 + size per file
```

---

## Verifying an Archive

```bash
rustydisc verify ~/archive/Morrissey\ -\ Bona\ Drag\ \(2010\)
```

Verifies every file against the SHA256 checksums recorded at rip time. Useful for long-term storage integrity checks.

---

## Red Book Audio CDs

**Burn a single album from WAV files:**
```bash
rustydisc burn --format redbook \
  --audio ~/music/Loveless/*.wav \
  --label "Loveless"
```

**Burn with CD-Text populated from embedded tags:**
```bash
rustydisc burn --format redbook \
  --audio ~/music/Loveless/*.wav \
  --label "Loveless" \
  --cd-text
```

**Burn from a playlist, dry-run first:**
```bash
rustydisc plan --format redbook --playlist "Mix.m3u8" --label "Mix"
rustydisc burn --format redbook --playlist "Mix.m3u8" --label "Mix"
```

**Convert MP3s to WAV automatically, then burn:**
```bash
rustydisc burn --format redbook \
  --audio ~/music/album/*.mp3 \
  --transcode wav \
  --label "My Album"
```

---

## Data CDs

**Burn a directory:**
```bash
rustydisc burn --format datacd \
  --data /srv/backups/project \
  --label "Project Backup"
```

**Convert all audio to MP3 256kbps CBR, then burn:**
```bash
rustydisc burn --format datacd \
  --data /srv/Media/Music \
  --transcode mp3:256 \
  --label "Music Collection"
```

---

## Blue Book / CD Extra

```bash
rustydisc burn --format bluebook \
  --audio ~/album/tracks/*.wav \
  --data ~/album/extras \
  --label "My Album"
```

The disc will play as a standard audio CD in any player, and show the `extras/` content when inserted into a computer.

---

## Playlists (M3U/M3U8)

Pass any M3U or M3U8 playlist with `--playlist`; it works for Audio CDs, Enhanced CDs and Data CDs.

- **Relative paths** are resolved from the playlist file's own folder (`../Albums/01.flac`, `Album One/01 Track.flac`).
- **Other ways of writing a path** are understood: Windows separators (`Music\Album\01.flac`), `C:\…` drive prefixes, `file://` URIs, `%20`-style escapes, and quoted paths. If a playlist was written on another machine and the absolute path doesn't exist here, RustyDisc looks for the same tail of the path next to the playlist (`C:\Users\me\Music\Album\01.flac` finds `Album/01.flac` beside the playlist).
- **Encodings:** `.m3u8` is UTF-8 (a BOM is fine); plain `.m3u` files that aren't valid UTF-8 are read as Windows-1252/Latin-1. CRLF line endings are fine.
- `#EXTINF` durations are used for multi-disc planning without requiring `ffprobe`.
- Entries that can't be used (missing files, folders, streams) are skipped with a warning naming each one.

For a Data CD, the files from a playlist go in the root of the disc. If two files share a name, the second becomes `name (2).ext` rather than overwriting the first.

```bash
# Data CD from playlist
rustydisc burn --format datacd \
  --playlist "/srv/Media/Playlists/Smooch Hits.m3u8" \
  --label "Smooch Hits"

# Red Book from playlist
rustydisc burn --format redbook \
  --playlist "~/playlists/chill.m3u8" \
  --label "Chill Mix"
```

Streaming URLs (`http://`, `https://`) in the playlist are skipped with a warning.

Data CDs can also be built from individual files with `--files a.txt b.pdf ...`.

---

## Transcoding with FFmpeg

The `--transcode` flag accepts a format name with an optional bitrate:

| Spec | Result |
|------|--------|
| `mp3:256` | MP3 CBR at 256 kbps |
| `mp3:320` | MP3 CBR at 320 kbps |
| `mp3:128` | MP3 CBR at 128 kbps |
| `aac:256` | AAC at 256 kbps |
| `opus:192` | Opus at 192 kbps |
| `flac` | Lossless FLAC (no bitrate) |
| `wav` | PCM WAV — required for Red Book if source is not 44.1/16/stereo |

Transcoded files are staged in a temporary directory, used for the burn, then deleted. Supply `--stage-dir` to control where they land, and `--keep-staged` to retain them.

---

## Multi-Disc Burning

When the content doesn't fit on one disc, RustyDisc works out how many discs are needed and walks you through burning each one. `rustydisc plan` (and **Show plan** in the web UI) shows the whole plan first: how many discs, what goes on each, and how full each will be.

How discs are filled:

- **Data CD** — packed by size, in order (albums and playlists stay together). Every file counts its ISO 9660 overhead (sector padding and directory records), and each disc keeps about 6 MB free, so a "700 MB" disc gets about 693 MB of files. Sub-folders are kept.
- **Transcoding is counted.** If you convert audio first (`--transcode mp3:320`), each file is counted at its *size after converting* (bitrate × length), so 2 GB of FLAC that becomes 1.3 GB of MP3 needs fewer discs. Files are only converted when it helps: lossless files are converted, but a lossy file already at or below the target bitrate is left alone (a 128k MP3 is never "upgraded" to 320k), and non-audio files are never touched. Estimates are slightly conservative.
- **The burn checks the real sizes.** Files are converted one disc at a time; the real converted size decides when a disc is full, and the disc image size is checked against the disc before anything is written. Only one disc's worth of converted files exists at a time (use `--stage-dir` to put them somewhere with more room than `/tmp`).
- **Red Book audio** — packed by playing time (79:30 on an 80-minute disc, up to 99 tracks).
- **Enhanced CD** — everything must fit on one disc, counting the audio, the gap between the two sessions and the data. If it doesn't fit you're told by how much.
- **Disc size** — `--disc-size 700` (default, 80 min), `650` (74 min) or `800` (90 min); it's a drop-down in the web UI.

<p align="center"><img src="../Images/Screenshots/burn-plan.png" alt="Show plan: a playlist of FLAC tracks converted to MP3 320k needs 2 discs instead of 3" width="820"></p>

```bash
# See the plan first: 3 discs as they are, 2 after converting to MP3 320k
rustydisc plan --format datacd --playlist "Giant Collection.m3u8"
rustydisc plan --format datacd --playlist "Giant Collection.m3u8" --transcode mp3:320

# 150 FLAC tracks → automatically split across multiple discs
rustydisc burn --format redbook \
  --playlist "Giant Collection.m3u8" \
  --label "Giant Collection"
```

**Example output:**
```
110 tracks (6:32:00 total) → 6 discs required.

══ Disc 1 of 6 ═══════════════════════════════════════
  17 tracks  |  73:44
Insert blank disc 1 into /dev/sr0 and press ENTER to burn...
```

For converted Data CDs the count is an estimate until the files are converted, so the prompt shows `Disc 1 of ~3` and corrects itself as it goes.

Each disc's volume label is automatically suffixed: `"Giant Collection - Disc 1"`, `"Giant Collection - Disc 2"`, etc. (a single disc keeps the label as it is; long labels are shortened to fit the 32-character limit). In the web UI the label is filled in from the playlist's file name (`Magnum Opus.m3u8` becomes `Magnum Opus`) until you type your own.

---

## CD-Text from Tags

`--cd-text` reads embedded metadata from audio files and writes it to the disc lead-in:

| Disc field | Source |
|-----------|--------|
| Album title | `ALBUM` tag from first tagged track |
| Disc artist | `ALBUMARTIST` if present; common `ARTIST` if all tracks agree; `"Various Artists"` for compilations |
| Track title | `TITLE` tag per track |

Supported tag formats: ID3v2 (WAV, MP3), Vorbis comments (FLAC, OGG), iTunes atoms (M4A).

When **ripping**, CD-Text is read from the disc automatically (no flag needed) and used as a fallback if the disc is not found in MusicBrainz.

---

## CD-RW Operations

**Check disc state:**
```bash
rustydisc recover --device /dev/sr0
```

**Quick-erase a CD-RW (fast blank — reuses existing structure):**
```bash
rustydisc recover --device /dev/sr0 --blank fast
```

**Full-erase a CD-RW (complete physical erase — slower but thorough):**
```bash
rustydisc recover --device /dev/sr0 --blank full
```

> **Note:** CD-RW only supports single-session burns. Blue Book (multi-session) requires a CD-R.

