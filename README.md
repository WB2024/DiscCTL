# Rusty Disc

<!-- LOGO: drop assets/logo.png here once generated -->
<!-- ![Rusty Disc](assets/logo.png) -->

> A blazing-fast disc authoring CLI for Linux — burn Red Book audio CDs, Data CDs, and Blue Book/CD Extra enhanced discs straight from the terminal.

[![Build](https://img.shields.io/badge/build-passing-brightgreen)](#development)
[![Rust](https://img.shields.io/badge/rust-1.85%2B-orange)](https://www.rust-lang.org)
[![License](https://img.shields.io/badge/license-MIT-blue)](#license)

---

## Contents

- [Overview](#overview)
- [Features](#features)
- [System Requirements](#system-requirements)
- [Installation](#installation)
- [Quick Start](#quick-start)
- [Disc Formats](#disc-formats)
- [Commands](#commands)
  - [burn](#rustydisc-burn)
  - [plan](#rustydisc-plan)
  - [validate](#rustydisc-validate)
  - [recover](#rustydisc-recover)
- [Usage Examples](#usage-examples)
  - [Red Book Audio CDs](#red-book-audio-cds)
  - [Data CDs](#data-cds)
  - [Blue Book / CD Extra](#blue-book--cd-extra)
  - [Playlists (M3U/M3U8)](#playlists-m3um3u8)
  - [Transcoding with FFmpeg](#transcoding-with-ffmpeg)
  - [Multi-Disc Burning](#multi-disc-burning)
  - [CD-Text from Tags](#cd-text-from-tags)
  - [CD-RW Operations](#cd-rw-operations)
- [Disc Graph JSON](#disc-graph-json)
- [Error Handling](#error-handling)
- [Hardware Notes](#hardware-notes)
- [Development](#development)
- [License](#license)

---

## Overview

Rusty Disc treats disc authoring as a **compiler problem**: you describe what you want to burn (tracks, files, format, label), and it figures out the rest — validating format rules, generating a deterministic burn plan, transcoding if needed, splitting across multiple discs automatically, and delegating the actual write to the appropriate backend.

The pipeline is:

```
CLI flags / JSON graph
        │
        ▼
  Disc Intent Parser      ← validates input, resolves playlists, reads tags
        │
        ▼
   Disc Graph Builder     ← unified intermediate representation
        │
        ▼
   Session Planner        ← enforces Red/Blue Book rules, checks WAV specs
        │
        ▼
 Backend Execution Layer  ← cdrdao (audio), xorriso (data/ISO), ffmpeg (transcode)
        │
        ▼
  Hardware Device Layer   ← ATIP detection, buffer underrun guard, multi-session
```

Format constraints are enforced **before** any hardware is touched, so you get a clear error rather than a half-burned coaster.

---

## Features

- **Three disc formats** — Red Book Audio, ISO9660 Data CD, Blue Book/CD Extra enhanced CD
- **Playlist support** — burn directly from an M3U or M3U8 playlist, durations read from `#EXTINF` tags
- **FFmpeg transcoding** — convert any audio format to MP3, AAC, Opus, FLAC, or WAV before burning; stage and clean up automatically
- **Multi-disc splitting** — automatically detects when content exceeds a single disc and prompts you to swap discs, no manual slicing required
- **CD-Text from tags** — reads track title, artist, and album from embedded metadata (ID3, Vorbis, etc.) and writes it to the disc lead-in
- **Disc state detection** — uses ATIP to reliably distinguish blank, appendable, and finalized discs; refuses to write on a full disc
- **CD-RW guard** — blocks multi-session Blue Book burns on rewriteable media; allows single-session DataCD on CD-RW
- **Buffer underrun protection detection** — warns if your drive lacks BURN-Proof/SMART-BURN
- **Dry run mode** — `--dry-run` prints the full execution plan as JSON without touching hardware
- **Structured errors** — every error is machine-readable JSON with a code, message, and `recoverable` flag

---

## System Requirements

### Runtime dependencies

| Tool | Purpose | Required |
|------|---------|----------|
| `cdrdao` | Red Book audio DAO burning | Yes (audio burns) |
| `xorriso` | ISO9660 generation, data burns, multisession | Yes (data/Blue Book) |
| `cdrecord` | Disc state detection (ATIP, -msinfo) | Yes |
| `ffmpeg` | Audio transcoding and format probing | Optional |

### Install on Debian / Ubuntu

```bash
sudo apt install cdrdao xorriso cdrecord ffmpeg
```

### Install on Arch Linux

```bash
sudo pacman -S cdrtools cdrdao xorriso ffmpeg
```

### Install on Fedora

```bash
sudo dnf install cdrtools cdrdao xorriso ffmpeg
```

### Rust toolchain

Requires **Rust 1.85 or newer**. Install via [rustup](https://rustup.rs):

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

---

## Installation

### From source (recommended)

```bash
git clone https://github.com/WB2024/DiscCTL.git
cd DiscCTL
cargo build --release
sudo install -m755 target/release/rustydisc /usr/local/bin/
```

### Local user install (no sudo)

```bash
cargo install --path .
# binary lands at ~/.cargo/bin/rustydisc
```

### Verify

```bash
rustydisc --version
rustydisc --help
```

---

## Quick Start

```bash
# Burn a set of WAV files as a Red Book audio CD
rustydisc burn --format redbook --audio ~/music/album/*.wav --label "My Album"

# Burn a directory as a data CD
rustydisc burn --format datacd --data ~/files/backup --label "Backup 2026"

# Burn from a playlist
rustydisc burn --format redbook --playlist "Smooch Hits.m3u8" --label "Smooch Hits"

# Preview the plan without touching the drive
rustydisc plan --format redbook --audio ~/music/album/*.wav --label "My Album"
```

---

## Disc Formats

### Red Book (`--format redbook`)

The standard audio CD format (IEC 60908). Supports up to **99 tracks** and **74 minutes** of 44.1 kHz 16-bit stereo PCM audio. Written in Disc-At-Once (DAO) mode via `cdrdao`. CD-Text is stored in the R-W subchannels of the lead-in area.

Audio files must be **44.1 kHz, 16-bit stereo WAV** (CDDA spec). Non-compliant files are rejected at plan time. Use `--transcode wav` to convert first.

### Data CD (`--format datacd`)

A single-session ISO9660 data disc with Joliet and Rock Ridge extensions. Supports up to **700 MB** of content. Built and burned with `xorriso`. Volume label is written as uppercase ISO9660 (max 32 characters).

### Blue Book / CD Extra (`--format bluebook`)

An enhanced CD with **two sessions**: Session 1 is a Red Book audio session (left open), Session 2 is appended as an ISO9660 data session, then the disc is finalized. Audio tracks play on any CD player; the data session is visible when inserted in a computer.

Session ordering (Audio → Data) is enforced at plan time. Blue Book requires a CD-R; CD-RW does not support the required multisession append.

---

## Commands

### `rustydisc burn`

Burns a disc from command-line flags or a JSON disc graph.

```
rustydisc burn [OPTIONS]
```

#### Input flags

| Flag | Description |
|------|-------------|
| `--format <fmt>` | Disc format: `redbook`, `datacd`, `bluebook` (default: `redbook`) |
| `--audio <files...>` | Audio track files or glob patterns (WAV/FLAC/MP3/M4A/OGG etc.) |
| `--playlist <file>` | M3U or M3U8 playlist — takes precedence over `--audio` if both given |
| `--data <dir>` | Source directory for the data session |
| `--label <text>` | Disc volume label (default: `Untitled`) |
| `--input <file>` | Load a disc graph JSON instead of building from flags |

#### Behaviour flags

| Flag | Description |
|------|-------------|
| `--device <dev>` | Target drive device (default: `/dev/sr0`) |
| `--dry-run` | Print the burn plan as JSON — do not burn |
| `--debug` | Print backend commands and verbose output |
| `--cd-text` | Read CD-Text (title, artist) from embedded audio file tags |

#### Transcoding flags

| Flag | Description |
|------|-------------|
| `--transcode <spec>` | Convert audio before burning: `mp3:256`, `aac:320`, `opus:192`, `flac`, `wav` |
| `--stage-dir <dir>` | Where to write transcoded files (auto-temp directory if omitted) |
| `--keep-staged` | Keep staged files after burn (default: delete on exit) |

---

### `rustydisc plan`

Prints the burn plan as JSON without writing to any device. Useful for scripting and verifying disc layout before committing to media.

```
rustydisc plan --format redbook --audio ~/music/*.wav --label "Preview"
rustydisc plan --input disc.json
```

| Flag | Description |
|------|-------------|
| `--format` | Disc format |
| `--audio` | Audio files or globs |
| `--playlist` | M3U/M3U8 playlist |
| `--data` | Data source directory |
| `--label` | Volume label |
| `--input` | Disc graph JSON |
| `--cd-text` | Include CD-Text in plan |

---

### `rustydisc validate`

Validates a disc graph JSON file for correctness — format rules, WAV spec, ISO size, session ordering — without touching hardware.

```
rustydisc validate disc.json
```

Returns exit code `0` on success, `1` on validation failure (with a structured JSON error on stderr).

---

### `rustydisc recover`

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

---

## Usage Examples

### Red Book Audio CDs

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
rustydisc plan --format redbook --playlist "Smooch Hits.m3u8" --label "Smooch Hits"
rustydisc burn --format redbook --playlist "Smooch Hits.m3u8" --label "Smooch Hits"
```

**Convert MP3s to WAV automatically, then burn:**
```bash
rustydisc burn --format redbook \
  --audio ~/music/album/*.mp3 \
  --transcode wav \
  --label "My Album"
```

---

### Data CDs

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

**Keep the transcoded files after burn:**
```bash
rustydisc burn --format datacd \
  --data /srv/Media/Music \
  --transcode mp3:192 \
  --stage-dir ~/staged \
  --keep-staged \
  --label "Music 192k"
```

---

### Blue Book / CD Extra

```bash
rustydisc burn --format bluebook \
  --audio ~/album/tracks/*.wav \
  --data ~/album/extras \
  --label "My Album"
```

The disc will play as a standard audio CD in any player, and show the `extras/` content when inserted into a computer.

---

### Playlists (M3U/M3U8)

Pass any M3U or M3U8 playlist with `--playlist`. Relative paths in the playlist are resolved relative to the playlist file's directory. `#EXTINF` durations are used for multi-disc planning without requiring `ffprobe`.

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

---

### Transcoding with FFmpeg

The `--transcode` flag accepts a format name with an optional bitrate:

| Spec | Result |
|------|--------|
| `mp3:256` | MP3 CBR at 256 kbps |
| `mp3:320` | MP3 CBR at 320 kbps |
| `mp3:128` | MP3 CBR at 128 kbps |
| `aac:256` | AAC at 256 kbps |
| `opus:192` | Opus at 192 kbps |
| `flac` | Lossless FLAC (no bitrate) |
| `wav` | PCM WAV — required for Red Book if source is not already 44.1/16/stereo |

Transcoded files are staged in a temporary directory, used for the burn, then deleted. Supply `--stage-dir` to control where they land, and `--keep-staged` to retain them.

**Transcode a playlist to AAC, burn as data disc:**
```bash
rustydisc burn --format datacd \
  --playlist "Collection.m3u8" \
  --transcode aac:256 \
  --label "Collection AAC"
```

---

### Multi-Disc Burning

When the total content exceeds single-disc capacity, Rusty Disc automatically calculates how many discs are needed and walks you through burning each one.

Thresholds:
- **Data CD** — 690 MB per disc (conservative, accounting for ISO overhead)
- **Red Book audio** — 74 minutes / 99 tracks per disc (30-second safety margin)

```bash
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

[burns disc 1]

Disc 1/6 complete. Remove the disc.

══ Disc 2 of 6 ═══════════════════════════════════════
  18 tracks  |  74:01
Insert blank disc 2 into /dev/sr0 and press ENTER to burn...
```

Each disc's volume label is automatically suffixed: `"Giant Collection (1/6)"`, `"Giant Collection (2/6)"`, etc.

With transcoding, all files are converted **first**, then the staged output is measured and split:

```bash
rustydisc burn --format datacd \
  --data /srv/Media/Library \
  --transcode mp3:128 \
  --label "Library"
# transcodes all files, then: "23 discs required"
```

---

### CD-Text from Tags

`--cd-text` reads embedded metadata from audio files and writes it to the disc lead-in:

| Disc field | Source |
|-----------|--------|
| Album title | `ALBUM` tag from first tagged track |
| Disc artist | `ALBUMARTIST` if present; common `ARTIST` if all tracks agree; `"Various Artists"` for compilations |
| Track title | `TITLE` tag per track |

Supported tag formats: ID3v2 (WAV, MP3), Vorbis comments (FLAC, OGG), iTunes atoms (M4A).

```bash
rustydisc burn --format redbook \
  --audio ~/music/album/*.flac \
  --transcode wav \
  --cd-text \
  --label "Album Name"
```

---

### CD-RW Operations

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

**Burn a single-session Data CD to a CD-RW:**
```bash
rustydisc burn --format datacd \
  --data ~/files \
  --label "Test" \
  --device /dev/sr0
```

> **Note:** CD-RW only supports single-session burns. Blue Book (multi-session) requires a CD-R.

---

## Disc Graph JSON

All disc definitions share a common JSON schema. This is the intermediate representation that every input path converges to.

```json
{
  "format": "bluebook",
  "label": "My Album",
  "sessions": [
    {
      "type": "audio",
      "tracks": [
        "/music/track01.wav",
        "/music/track02.wav"
      ],
      "cd_text": {
        "title": "My Album",
        "artist": "Artist Name"
      },
      "track_titles": [
        { "title": "Track One" },
        { "title": "Track Two" }
      ]
    },
    {
      "type": "data",
      "source_dir": "/music/extras",
      "filesystem": "iso9660",
      "joliet": true,
      "rock_ridge": true
    }
  ]
}
```

**Format values:**

| Value | Description |
|-------|-------------|
| `redbook` | Red Book Audio CD |
| `datacd` | ISO9660 Data CD |
| `bluebook` | Blue Book / CD Extra |

**Burn from a JSON graph:**
```bash
rustydisc burn --input disc.json --device /dev/sr0
rustydisc plan --input disc.json
rustydisc validate disc.json
```

---

## Error Handling

All errors are emitted to stderr as structured JSON:

```json
{
  "error": "SESSION_ORDER_INVALID",
  "message": "Data session cannot precede audio session in BlueBook format",
  "recoverable": false
}
```

```json
{
  "error": "DISC_ALREADY_FINALIZED",
  "message": "Disc on /dev/sr0 is already finalized. Insert a blank disc or use `rustydisc recover --blank fast` for CD-RW.",
  "recoverable": true
}
```

```json
{
  "error": "WAV_SPEC_INVALID",
  "message": "track01.wav: expected 44100 Hz stereo 16-bit PCM, got 48000 Hz",
  "recoverable": false
}
```

`recoverable: true` means you can fix the issue (insert a blank disc, erase the CD-RW, etc.) and retry the same command. `recoverable: false` means there is a problem with your input that must be corrected before burning.

Exit codes:
- `0` — success
- `1` — error (details on stderr as JSON)

---

## Hardware Notes

### Disc state detection

Rusty Disc uses ATIP (Absolute Time in Pregroove) as its primary signal for disc state:

- **ATIP readable** → the disc is a blank or partially-written writable disc (CD-R/CD-RW)
- **ATIP not readable** → the disc is finalized (pressed or burned and closed)

This is more reliable than reading the TOC, which can be present on both blank and finalized discs.

### Drive requirements

Any standard CD-R/CD-RW burner supported by Linux will work. The device node is typically `/dev/sr0` or `/dev/cdrom`. Use `--device` to specify a different path.

### Buffer underrun protection

Rusty Disc warns at burn time if your drive does not report buffer underrun protection (BURN-Proof, BurnFree, SMART-BURN, JustLink). On modern drives this is almost always supported. If you see the warning, avoid running heavy background tasks during the burn.

### CD-RW vs CD-R

| Feature | CD-R | CD-RW |
|---------|------|-------|
| Red Book burn | Yes | Yes |
| Data CD burn | Yes | Yes |
| Blue Book / multisession | Yes | **No** |
| Can be erased and reused | No | Yes |

---

## Development

```bash
# Build
cargo build

# Release build (optimised)
cargo build --release

# Run all unit tests
cargo test

# Run tests with stdout visible
cargo test -- --nocapture

# Lint
cargo clippy

# Format
cargo fmt
```

### Hardware integration tests

Hardware tests require a physical CD-R drive and should be run one at a time:

```bash
cargo test --features hardware_tests -- --test-threads=1
```

Destructive burn tests are opt-in:

```bash
DISCCTL_ENABLE_BURN_TESTS=1 \
DISCCTL_TEST_DEVICE=/dev/sr0 \
cargo test --features hardware_tests -- --test-threads=1
```

Environment variables:

| Variable | Default | Description |
|----------|---------|-------------|
| `DISCCTL_TEST_DEVICE` | `/dev/sr0` | Drive device node |
| `DISCCTL_TEST_AUDIO_DIR` | `tests/fixtures/audio` | WAV fixtures directory |
| `DISCCTL_TEST_DATA_DIR` | `tests/fixtures/data` | Data fixtures directory |
| `DISCCTL_ENABLE_BURN_TESTS` | `0` | Set to `1` to run destructive burns |

### Architecture

```
src/
├── main.rs               ← CLI entry point (clap)
├── model/
│   ├── disc.rs           ← DiscGraph, Session, AudioSession, DataSession
│   └── plan.rs           ← BurnPlan, BurnStep
├── parser/
│   ├── mod.rs            ← from_cli(), from_file(), glob expansion
│   ├── cdtext.rs         ← tag reading via lofty
│   └── playlist.rs       ← M3U/M3U8 parsing
├── planner/
│   ├── mod.rs            ← plan(), validate(), WAV/ISO validation
│   └── split.rs          ← multi-disc bin packing
├── backend/
│   ├── mod.rs            ← execute(), disc state pre-flight
│   ├── audio.rs          ← cdrdao TOC generation and burn
│   ├── data.rs           ← xorriso ISO generation and append
│   ├── convert.rs        ← ffmpeg PCM conversion
│   ├── transcode.rs      ← TranscodeSpec, StagedDir, full transcode pipeline
│   └── device.rs         ← ATIP, msinfo, finalize, blank, buffer underrun
├── commands/
│   ├── burn.rs           ← BurnArgs, multi-disc orchestration
│   ├── plan.rs           ← PlanArgs
│   ├── validate.rs       ← ValidateArgs
│   └── recover.rs        ← RecoverArgs
└── error.rs              ← unified Error type, DiscError JSON struct
```

---

## License

MIT — see [LICENSE](LICENSE) for details.

---

*Built with Rust. Burns with precision.*
