# Architecture & development

How RustyDisc works inside, and how to build and test it.

[← Back to the README](../README.md)

---

# Overview

Rusty Disc treats optical discs as structured objects, not just files. The core abstraction is the **DiscGraph** — a unified intermediate representation that drives both directions of the pipeline:

```
          DiscGraph
         /         \
   Burn Plan     Rip Plan
        ↓             ↑
   Physical Disc ↔ Physical Disc
```

Burning and ripping are inverse operations of the same graph. You describe what you want (tracks, files, format, label), RustyDisc figures out the rest.

**Burn pipeline:**
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

**Rip pipeline:**
```
Physical Disc
        │
        ▼
  Disc Analyzer           ← reads TOC via cdrecord, detects format, extracts CD-Text
        │                    computes MusicBrainz DiscID from audio track layout
        ▼
  MusicBrainz Lookup      ← queries MB REST API by DiscID; fetches album, artist,
        │                    year, per-track titles, recording IDs
        ▼
  Cover Art Archive       ← downloads front cover image; saved as cover.jpg/png
        │
        ▼
  Secure Rip Engine       ← cdparanoia (paranoia mode: overlapping reads, jitter correction,
        │                    paranoia retry logic)
        ▼
  AccurateRip Check       ← v1 + v2 checksums per track, matched against the database
        │                    (tolerates drive read offset)
        ▼
   Audio Encoders         ← ffmpeg → WAV / FLAC / ALAC / AIFF / OGG / MP3 / Opus
   Data Extractor         ← xorriso → directory tree or ISO image
        │                    cover art embedded in every audio file
        ▼
  Metadata + Checksums    ← musicbrainz.json, accuraterip.json, cdtext.json, disc.json, checksums.json
```

Format constraints are enforced **before** any hardware is touched, so you get a clear error rather than a half-burned coaster.

---


# Development

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

## Hardware integration tests

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

## Architecture

```
src/
├── main.rs               ← CLI entry point (clap)
├── analyzer/
│   └── mod.rs            ← Disc Analyzer: TOC via cdrecord, CD-Text via cdrdao,
│                             data session metadata via isoinfo; computes MB DiscID
├── rip/
│   ├── mod.rs            ← Rip coordinator: analyze → MB lookup → cover art →
│   │                         rip → encode → metadata
│   ├── engine.rs         ← Secure rip engine: cdparanoia wrapper (EAC-grade)
│   ├── encoder.rs        ← AudioFormat enum + ffmpeg encoding for all 7 formats;
│   │                         cover art embedding via -map 0:a -map 1:v
│   ├── musicbrainz.rs    ← MB API lookup by DiscID; Cover Art Archive fetching
│   ├── data.rs           ← Data session extraction via xorriso osirrox
│   └── metadata.rs       ← SHA256 checksum generation and verification
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
│   ├── info.rs           ← InfoArgs → analyzer::analyze() + display
│   ├── rip.rs            ← RipArgs → rip::rip()
│   ├── verify.rs         ← VerifyArgs → metadata::verify_checksums()
│   ├── burn.rs           ← BurnArgs, multi-disc orchestration
│   ├── plan.rs           ← PlanArgs
│   ├── validate.rs       ← ValidateArgs
│   └── recover.rs        ← RecoverArgs
└── error.rs              ← unified Error type, DiscError JSON struct
```

