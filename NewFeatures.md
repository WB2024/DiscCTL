
#### You:
what about ripping, going back to what we talked about earlier, when i said 'okay, so if you were building a modern linux cli interface tool that supports burning data discs (data cds), redbook and bluebooks, how would you go about it, based on whats been discussed re the bluebooks, if you were building a tool that you could simply specify the format (redbook,data, bluebook) and have it handle everything under the hood, how would the backend work?' take that same question but instead of burning, ripping - this includes ripping to the highest possible quality, multiple different file formats, enhanced cd audio and data extraction etc - how would you build this in a modern cli - i'll combine both the burning and ripping features into a single cli - by the way its now called RustyDisc (as its written in rust)

#### ChatGPT:
I think combining both authoring and extraction under **RustyDisc** is actually the right abstraction. If DiscCTL was fundamentally a **disc compiler**, RustyDisc becomes a **disc systems toolkit**.

The key insight is:

> Burning and ripping are inverse operations of the same Disc Graph.

---

# Philosophy

Instead of:

```text
Burning = files → disc
Ripping = disc → files
```

Think:

```text
            DiscGraph
           /         \
      Burn Plan     Rip Plan
         ↓              ↑
      Physical Disc ↔ Physical Disc
```

The same internal graph representation should support both directions.

---

# High-Level Architecture

```text
CLI
 ↓
Disc Analyzer
 ↓
Disc Graph
 ↓
Rip Planner / Burn Planner
 ↓
Backend Drivers
 ↓
Filesystem Output
```

---

# Supported Input Types

### Data CDs

Output:

- ISO image
- Directory tree
- TAR archive

### Red Book Audio CDs

Output:

- WAV
- FLAC
- ALAC
- AIFF
- OGG Vorbis
- MP3
- Opus

### Blue Book / Enhanced CDs

Output:

```text
disc/
    audio/
        Track01.flac
        Track02.flac

    data/
        cover.jpg
        index.html

    metadata/
        disc.json
        toc.txt
        cdtext.json
```

---

# Core Abstraction

## DiscGraph

Same graph as burning:

```rust
Disc
├── Session1
│     ├── Track01 (audio)
│     ├── Track02 (audio)
│
└── Session2
      └── ISO9660 filesystem
```

The analyzer builds this graph from the physical disc.

---

# Major Components

## Disc Analyzer

Responsible for:

### TOC reading

Read:

- sessions
- tracks
- indices
- pregaps
- leadouts

via:

```text
MMC READ TOC/PMA/ATIP
READ SUBCHANNEL
```

Linux:

```text
/dev/sr*
sg_io
```

---

## Session Analyzer

Determines:

### Red Book

Single session

```text
audio tracks only
```

### Data Disc

Single ISO session

### Blue Book

```text
Session 1 → audio
Session 2 → data
```

---

# Audio Extraction Backend

The hardest component.

---

## Secure Rip Engine

Think EAC (Exact Audio Copy) quality.

Not simple sequential reads.

### Features

Multiple reads

```text
sector read
compare
re-read bad sectors
majority vote
```

C2 error support

Drive offset correction

AccurateRip support (future)

Paranoia mode

---

Backends:

### cdparanoia algorithm

Reimplemented in Rust or wrapped.

---

## Internal audio representation

Always:

```rust
PCM 44.1 kHz
16-bit
Stereo
```

Lossless internal master.

Everything else derived from this.

---

# Encoders

Trait:

```rust
trait Encoder {
    fn encode(
        pcm: PCMBuffer,
        output: Path
    );
}
```

Implementations:

## WAV

Lossless.

## FLAC

Compression levels 0-8.

Embedded metadata.

## ALAC

Apple ecosystem.

## AIFF

Archival.

## Opus

Modern lossy.

## OGG Vorbis

Legacy open.

## MP3

LAME backend.

---

# Data Extraction

For ISO session:

Read session boundaries.

Extract:

```rust
mkisofs image
```

or

Raw image:

```rust
Session2.bin
```

Options:

```bash
rustydisc rip data --iso
rustydisc rip data --extract
```

---

# Blue Book Extraction

Disc:

```text
Session 1
audio

Session 2
ISO filesystem
```

Produces:

```text
album/
    audio/
    extras/
    metadata/
```

Or:

```text
album/
    audio.flac/
    session2.iso
```

---

# Metadata Layer

Read:

## CD-Text

Album title

Track names

Perform:

```text
READ SUBCHANNEL
```

Export:

```json
cdtext.json
```

---

Future:

MusicBrainz

AccurateRip

CUETools DB

---

# Image Formats

The most interesting part.

---

## ISO

Data only.

---

## BIN/CUE

Single-session mixed mode.

Support:

Import/export.

---

## CloneCD

CCD/IMG/SUB

Preserve subchannels.

---

## CDRDAO

TOC/BIN

Excellent for audio.

---

## NRG

Read support.

Write optional.

---

## MDF/MDS

Read support.

Write optional.

---

## Full Raw Dump

Ultimate archival:

```text
disc.bin
disc.sub
disc.toc
disc.json
```

Equivalent to "dd for CDs".

---

# Enhanced CD Handling

The difficult part.

Need session-aware imaging.

DiscGraph:

```rust
Disc
{
    sessions: [
        AudioSession,
        DataSession
    ]
}
```

Can export:

### exploded layout

```text
audio/
extras/
```

### raw image

```text
session1.bin
session2.iso
```

### reconstruction manifest

```json
discgraph.json
```

allowing:

```bash
rustydisc burn discgraph.json
```

to recreate disc.

---

# Verification

Critical feature.

After ripping:

Generate:

```json
checksums.json
```

SHA256

Track CRC

Disc CRC

Session CRC

---

Verify:

```bash
rustydisc verify album/
```

---

# Plugin Encoder Architecture

```rust
trait Codec {
    encode();
    decode();
}
```

Built-ins:

- FLAC
- WAV
- ALAC
- Opus
- Vorbis

Optional:

- AAC
- MP3

---

# CLI UX

## Detect

```bash
rustydisc info
```

Output:

```text
Disc Type: Blue Book
Sessions: 2

Session1
Audio
12 tracks

Session2
ISO9660
18 MB
```

---

## Rip everything

```bash
rustydisc rip
```

Produces:

```text
album/
```

Automatically.

---

## Rip FLAC

```bash
rustydisc rip --format flac
```

---

## Rip lossless archive

```bash
rustydisc rip --archive
```

Produces:

```text
discgraph.json
audio/
session2.iso
checksums.json
cdtext.json
```

---

## Rebuild

```bash
rustydisc burn archive/discgraph.json
```

Reconstructs original layout.

---

# Internal Architecture

```text
rustydisc-core
    DiscGraph
    SessionGraph
    Metadata

rustydisc-rip
    Analyzer
    SecureRipEngine

rustydisc-burn
    SessionPlanner
    BurnExecutor

rustydisc-codecs
    FLAC
    WAV
    Opus

rustydisc-images
    ISO
    CUE
    CCD
    NRG

rustydisc-drives
    sg_io
    MMC commands

rustydisc-cli
```

---

# The Most Important Principle

RustyDisc shouldn't think in terms of:

> "burning files" or "ripping tracks".

It should think in terms of:

> **constructing, analyzing, preserving, and reconstructing DiscGraphs.**

That gives you something much closer to a modern equivalent of:

- cdparanoia
- cdrdao
- xorriso
- Exact Audio Copy
- CloneCD

all unified behind one deterministic Rust architecture.


#### You:
perfect, can you provide me with a dev/info spec that i can provide to my developer to devlop onto RustyDisc? this needs to include everything you mentioned in your last mentioned plus all of the developer info to make this happen

#### ChatGPT:
# RustyDisc

## Unified Optical Disc Toolkit

### Developer Specification v2.0

### (Burning + Ripping + Archival + Reconstruction)

---

# 1. Overview

RustyDisc is a modern Rust-based CLI toolkit for optical disc authoring, extraction, preservation, and reconstruction.

RustyDisc treats optical discs as structured objects rather than files.

The core abstraction is:

```text
DiscGraph
```

which serves as the canonical intermediate representation.

RustyDisc supports:

- Red Book Audio CDs
- Data CDs
- Blue Book (Enhanced/CD Extra)
- Disc imaging
- Lossless extraction
- Exact archival
- Reconstruction
- Metadata preservation
- Future extension to DVD/BluRay

---

# 2. Design Philosophy

Traditional tools think:

```text
files → disc
disc → files
```

RustyDisc thinks:

```text
          DiscGraph
         /         \
 Burn Plan       Rip Plan
      ↓               ↑
 Physical Disc ↔ Physical Disc
```

Burning and ripping are inverse operations.

---

# 3. Core Objectives

## Burning

Support:

- Red Book audio CDs
- Data CDs
- Blue Book CDs

via:

```bash
rustydisc burn ...
```

---

## Ripping

Support:

```bash
rustydisc rip ...
```

with:

- Secure audio extraction
- Session extraction
- Metadata preservation
- Archive creation

---

## Reconstruction

Support:

```bash
rustydisc rebuild archive/
```

to recreate discs from archive manifests.

---

# 4. Repository Layout

```text
rustydisc/

├── rustydisc-cli
├── rustydisc-core
├── rustydisc-burn
├── rustydisc-
#### You:
what about ripping, going back to what we talked about earlier, when i said 'okay, so if you were building a modern linux cli interface tool that supports burning data discs (data cds), redbook and bluebooks, how would you go about it, based on whats been discussed re the bluebooks, if you were building a tool that you could simply specify the format (redbook,data, bluebook) and have it handle everything under the hood, how would the backend work?' take that same question but instead of burning, ripping - this includes ripping to the highest possible quality, multiple different file formats, enhanced cd audio and data extraction etc - how would you build this in a modern cli - i'll combine both the burning and ripping features into a single cli - by the way its now called RustyDisc (as its written in rust)

#### ChatGPT:
I think combining both authoring and extraction under **RustyDisc** is actually the right abstraction. If DiscCTL was fundamentally a **disc compiler**, RustyDisc becomes a **disc systems toolkit**.

The key insight is:

> Burning and ripping are inverse operations of the same Disc Graph.

---

# Philosophy

Instead of:

```text
Burning = files → disc
Ripping = disc → files
```

Think:

```text
            DiscGraph
           /         \
      Burn Plan     Rip Plan
         ↓              ↑
      Physical Disc ↔ Physical Disc
```

The same internal graph representation should support both directions.

---

# High-Level Architecture

```text
CLI
 ↓
Disc Analyzer
 ↓
Disc Graph
 ↓
Rip Planner / Burn Planner
 ↓
Backend Drivers
 ↓
Filesystem Output
```

---

# Supported Input Types

### Data CDs

Output:

- ISO image
- Directory tree
- TAR archive

### Red Book Audio CDs

Output:

- WAV
- FLAC
- ALAC
- AIFF
- OGG Vorbis
- MP3
- Opus

### Blue Book / Enhanced CDs

Output:

```text
disc/
    audio/
        Track01.flac
        Track02.flac

    data/
        cover.jpg
        index.html

    metadata/
        disc.json
        toc.txt
        cdtext.json
```

---

# Core Abstraction

## DiscGraph

Same graph as burning:

```rust
Disc
├── Session1
│     ├── Track01 (audio)
│     ├── Track02 (audio)
│
└── Session2
      └── ISO9660 filesystem
```

The analyzer builds this graph from the physical disc.

---

# Major Components

## Disc Analyzer

Responsible for:

### TOC reading

Read:

- sessions
- tracks
- indices
- pregaps
- leadouts

via:

```text
MMC READ TOC/PMA/ATIP
READ SUBCHANNEL
```

Linux:

```text
/dev/sr*
sg_io
```

---

## Session Analyzer

Determines:

### Red Book

Single session

```text
audio tracks only
```

### Data Disc

Single ISO session

### Blue Book

```text
Session 1 → audio
Session 2 → data
```

---

# Audio Extraction Backend

The hardest component.

---

## Secure Rip Engine

Think EAC (Exact Audio Copy) quality.

Not simple sequential reads.

### Features

Multiple reads

```text
sector read
compare
re-read bad sectors
majority vote
```

C2 error support

Drive offset correction

AccurateRip support (future)

Paranoia mode

---

Backends:

### cdparanoia algorithm

Reimplemented in Rust or wrapped.

---

## Internal audio representation

Always:

```rust
PCM 44.1 kHz
16-bit
Stereo
```

Lossless internal master.

Everything else derived from this.

---

# Encoders

Trait:

```rust
trait Encoder {
    fn encode(
        pcm: PCMBuffer,
        output: Path
    );
}
```

Implementations:

## WAV

Lossless.

## FLAC

Compression levels 0-8.

Embedded metadata.

## ALAC

Apple ecosystem.

## AIFF

Archival.

## Opus

Modern lossy.

## OGG Vorbis

Legacy open.

## MP3

LAME backend.

---

# Data Extraction

For ISO session:

Read session boundaries.

Extract:

```rust
mkisofs image
```

or

Raw image:

```rust
Session2.bin
```

Options:

```bash
rustydisc rip data --iso
rustydisc rip data --extract
```

---

# Blue Book Extraction

Disc:

```text
Session 1
audio

Session 2
ISO filesystem
```

Produces:

```text
album/
    audio/
    extras/
    metadata/
```

Or:

```text
album/
    audio.flac/
    session2.iso
```

---

# Metadata Layer

Read:

## CD-Text

Album title

Track names

Perform:

```text
READ SUBCHANNEL
```

Export:

```json
cdtext.json
```

---

Future:

MusicBrainz

AccurateRip

CUETools DB

---

# Image Formats

The most interesting part.

---

## ISO

Data only.

---

## BIN/CUE

Single-session mixed mode.

Support:

Import/export.

---

## CloneCD

CCD/IMG/SUB

Preserve subchannels.

---

## CDRDAO

TOC/BIN

Excellent for audio.

---

## NRG

Read support.

Write optional.

---

## MDF/MDS

Read support.

Write optional.

---

## Full Raw Dump

Ultimate archival:

```text
disc.bin
disc.sub
disc.toc
disc.json
```

Equivalent to "dd for CDs".

---

# Enhanced CD Handling

The difficult part.

Need session-aware imaging.

DiscGraph:

```rust
Disc
{
    sessions: [
        AudioSession,
        DataSession
    ]
}
```

Can export:

### exploded layout

```text
audio/
extras/
```

### raw image

```text
session1.bin
session2.iso
```

### reconstruction manifest

```json
discgraph.json
```

allowing:

```bash
rustydisc burn discgraph.json
```

to recreate disc.

---

# Verification

Critical feature.

After ripping:

Generate:

```json
checksums.json
```

SHA256

Track CRC

Disc CRC

Session CRC

---

Verify:

```bash
rustydisc verify album/
```

---

# Plugin Encoder Architecture

```rust
trait Codec {
    encode();
    decode();
}
```

Built-ins:

- FLAC
- WAV
- ALAC
- Opus
- Vorbis

Optional:

- AAC
- MP3

---

# CLI UX

## Detect

```bash
rustydisc info
```

Output:

```text
Disc Type: Blue Book
Sessions: 2

Session1
Audio
12 tracks

Session2
ISO9660
18 MB
```

---

## Rip everything

```bash
rustydisc rip
```

Produces:

```text
album/
```

Automatically.

---

## Rip FLAC

```bash
rustydisc rip --format flac
```

---

## Rip lossless archive

```bash
rustydisc rip --archive
```

Produces:

```text
discgraph.json
audio/
session2.iso
checksums.json
cdtext.json
```

---

## Rebuild

```bash
rustydisc burn archive/discgraph.json
```

Reconstructs original layout.

---

# Internal Architecture

```text
rustydisc-core
    DiscGraph
    SessionGraph
    Metadata

rustydisc-rip
    Analyzer
    SecureRipEngine

rustydisc-burn
    SessionPlanner
    BurnExecutor

rustydisc-codecs
    FLAC
    WAV
    Opus

rustydisc-images
    ISO
    CUE
    CCD
    NRG

rustydisc-drives
    sg_io
    MMC commands

rustydisc-cli
```

---

# The Most Important Principle

RustyDisc shouldn't think in terms of:

> "burning files" or "ripping tracks".

It should think in terms of:

> **constructing, analyzing, preserving, and reconstructing DiscGraphs.**

That gives you something much closer to a modern equivalent of:

- cdparanoia
- cdrdao
- xorriso
- Exact Audio Copy
- CloneCD

all unified behind one deterministic Rust architecture.


#### You:
perfect, can you provide me with a dev/info spec that i can provide to my developer to devlop onto RustyDisc? this needs to include everything you mentioned in your last mentioned plus all of the developer info to make this happen

#### ChatGPT:
# RustyDisc

## Unified Optical Disc Toolkit

### Developer Specification v2.0

### (Burning + Ripping + Archival + Reconstruction)

---

# 1. Overview

RustyDisc is a modern Rust-based CLI toolkit for optical disc authoring, extraction, preservation, and reconstruction.

RustyDisc treats optical discs as structured objects rather than files.

The core abstraction is:

```text
DiscGraph
```

which serves as the canonical intermediate representation.

RustyDisc supports:

- Red Book Audio CDs
- Data CDs
- Blue Book (Enhanced/CD Extra)
- Disc imaging
- Lossless extraction
- Exact archival
- Reconstruction
- Metadata preservation
- Future extension to DVD/BluRay

---

# 2. Design Philosophy

Traditional tools think:

```text
files → disc
disc → files
```

RustyDisc thinks:

```text
          DiscGraph
         /         \
 Burn Plan       Rip Plan
      ↓               ↑
 Physical Disc ↔ Physical Disc
```

Burning and ripping are inverse operations.

---

# 3. Core Objectives

## Burning

Support:

- Red Book audio CDs
- Data CDs
- Blue Book CDs

via:

```bash
rustydisc burn ...
```

---

## Ripping

Support:

```bash
rustydisc rip ...
```

with:

- Secure audio extraction
- Session extraction
- Metadata preservation
- Archive creation

---

## Reconstruction

Support:

```bash
rustydisc rebuild archive/
```

to recreate disrip
├── rustydisc-drives
├── rustydisc-images
├── rustydisc-codecs
├── rustydisc-metadata
├── rustydisc-archive
├── rustydisc-session
├── rustydisc-testing
└── docs/
```

Workspace-based Cargo project.

---

# 5. Core DiscGraph

Everything converts to DiscGraph.

```rust
pub struct DiscGraph {
    pub format: DiscFormat,
    pub sessions: Vec<Session>,
    pub metadata: DiscMetadata,
}
```

---

## DiscFormat

```rust
enum DiscFormat {
    RedBook,
    DataCD,
    BlueBook,
    Unknown
}
```

---

# Session

```rust
enum Session {
    Audio(AudioSession),
    Data(DataSession)
}
```

---

# AudioSession

```rust
pub struct AudioSession {
    pub tracks: Vec<AudioTrack>,
}
```

---

# DataSession

```rust
pub struct DataSession {
    pub filesystem: FileSystem,
    pub source: PathBuf,
}
```

---

# 6. Drive Layer

Crate:

```text
rustydisc-drives
```

Provides hardware abstraction.

---

## Device Discovery

Linux:

```text
/dev/sr*
```

and

```text
sg_io
```

MMC commands.

---

Trait:

```rust
trait OpticalDrive {
    fn read_toc();
    fn read_subchannel();
    fn reserve();
    fn eject();
    fn finalize();
}
```

---

# 7. Disc Analyzer

Crate:

```text
rustydisc-rip
```

Reads:

- TOC
- PMA
- ATIP
- Session layout
- Track layout

using MMC commands:

```text
READ TOC
READ SUBCHANNEL
READ HEADER
```

Builds:

```rust
DiscGraph
```

---

# 8. Session Detection

Determine:

## Red Book

Single audio session.

---

## Data Disc

Single ISO session.

---

## Blue Book

Session 1:

Audio

Session 2:

Data

---

# 9. Secure Rip Engine

Equivalent to:

- cdparanoia
- EAC

---

Responsibilities:

Multiple reads.

Sector verification.

Retry logic.

Majority voting.

Offset correction.

C2 error support.

Gap detection.

Index support.

---

Trait:

```rust
trait SecureReader {
    fn read_sector();
    fn verify_sector();
}
```

---

# 10. Internal Audio Representation

Canonical:

```rust
PCMBuffer
```

Format:

- 44100Hz
- 16-bit
- stereo

All encoders derive from PCM.

---

# 11. Audio Encoders

Trait:

```rust
trait Encoder {
    fn encode();
}
```

---

Built-in:

### WAV

Lossless.

---

### FLAC

Compression 0-8.

Embedded metadata.

---

### AIFF

---

### ALAC

---

### Opus

---

### OGG Vorbis

---

Optional:

### MP3

Feature gated.

Requires LAME.

---

# 12. Metadata Layer

Crate:

```text
rustydisc-metadata
```

Supports:

### CD-Text

Album

Artist

Track titles

Genre

Composer

---

Future:

MusicBrainz

AccurateRip

CUETools

DiscID

---

Export:

```json
cdtext.json
```

---

# 13. Image Formats

Crate:

```text
rustydisc-images
```

---

## ISO9660

Read/write.

---

## BIN/CUE

Read/write.

---

## CDRDAO TOC/BIN

Read/write.

---

## CloneCD

CCD/IMG/SUB

Read/write.

---

## NRG

Read-only.

---

## MDF/MDS

Read-only.

---

## Raw dump

Read/write.

---

# Image Trait

```rust
trait DiscImage {
    fn read();
    fn write();
}
```

---

# 14. Burning Subsystem

Crate:

```text
rustydisc-burn
```

Contains:

## Session Planner

Most important component.

---

Produces:

```rust
BurnPlan
```

from:

```rust
DiscGraph
```

---

# Red Book

Single session.

DAO.

Finalize.

---

# Data CD

ISO generation.

Single session.

Finalize.

---

# Blue Book

Session 1:

Audio

Do not finalize.

Session 2:

ISO9660

Finalize.

---

# Burn State Machine

```rust
enum BurnState {
    Idle,
    Reserving,
    WritingAudio,
    WritingData,
    Finalizing,
    Complete,
    Error
}
```

---

# 15. Ripping Subsystem

Produces:

```rust
RipPlan
```

---

Options:

Exploded layout.

Image layout.

Archive layout.

---

# Exploded Layout

```text
album/

audio/
extras/
metadata/
```

---

# Image Layout

```text
disc.bin
disc.cue
```

---

# Archive Layout

```text
archive/

discgraph.json
audio/
session2.iso
metadata/
checksums.json
```

---

# 16. Archive Crate

Purpose:

Long-term preservation.

---

Contains:

```json
discgraph.json
```

plus:

```text
audio/
session2.iso
metadata/
checksums.json
```

---

Can reconstruct via:

```bash
rustydisc rebuild archive/
```

---

# 17. Session Engine

Crate:

```text
rustydisc-session
```

Provides:

Session graph.

---

Session state machine.

```rust
enum SessionState {
    Pending,
    Active,
    Complete,
    Failed
}
```

---

Blue Book validation.

Ordering enforcement.

---

# 18. Verification

Generate:

```json
checksums.json
```

Includes:

SHA256

Track CRC

Session CRC

Disc CRC

---

Verify:

```bash
rustydisc verify archive/
```

---

# 19. CLI

## Detect

```bash
rustydisc info
```

---

## Burn

```bash
rustydisc burn --format redbook
```

---

```bash
rustydisc burn --format data
```

---

```bash
rustydisc burn --format bluebook
```

---

## Rip

```bash
rustydisc rip
```

---

FLAC:

```bash
rustydisc rip --format flac
```

---

Archive:

```bash
rustydisc rip --archive
```

---

Verify:

```bash
rustydisc verify archive/
```

---

Rebuild:

```bash
rustydisc rebuild archive/
```

---

# 20. Hardware Layer

Support:

CD-R

CD-RW

Multisession

DAO

TAO

Buffer underrun protection

Drive capability detection.

---

# 21. Future Features

DVD

BluRay

SACD

HDCD

AccurateRip

MusicBrainz

CUETools

Parallel ripping

Network archive

Remote drives

---

# 22. Testing

Unit tests:

DiscGraph

Session planner

Image parser

Metadata

---

Integration tests:

Burn plans

Rip plans

Image reconstruction

---

Mock drives.

---

Hardware tests.

---

Golden-image verification.

---

# 23. Performance

Zero-copy where possible.

Streaming architecture.

Large buffers.

Multithreaded encoding.

Rayon parallelism.

---

# 24. Error Model

Structured errors:

```rust
enum RustyDiscError {
    DriveNotFound,
    InvalidSessionLayout,
    BurnFailed,
    VerificationFailed,
    MetadataError,
}
```

---

# 25. Guiding Principle

RustyDisc is not:

> a burner.

RustyDisc is not:

> a ripper.

RustyDisc is:

> A deterministic optical disc compiler, analyzer, archivist, and reconstruction toolkit whose canonical representation is DiscGraph.

---

# MVP Priorities (Recommended)

### Phase 1

✅ Red Book burn

✅ Data CD burn

✅ FLAC ripping

✅ DiscGraph

---

### Phase 2

✅ Blue Book support

✅ Archive format

✅ Verification

---

### Phase 3

✅ BIN/CUE

✅ CloneCD

✅ Reconstruction

---

### Phase 4

✅ Exact Audio Copy-grade secure ripping

✅ AccurateRip

✅ Metadata providers

---

### Phase 5

✅ DVD/BluRay

✅ Network archive

✅ GUI frontend

---

This specification should be considered the foundation for a multi-crate Rust workspace designed around **DiscGraph as the canonical intermediate representation**, with all burn, rip, archive, and reconstruction operations expressed as transformations to and from that graph.

