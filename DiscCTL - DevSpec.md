---

# 📀 DiscCTL - Unified CD Authoring CLI Tool

## Developer Specification (v1.0)

## 1. Overview

DiscCTL is a Linux CLI tool that provides a unified interface for authoring and burning optical discs:

- **Red Book Audio CDs** (CD-DA)
- **Data CDs (ISO9660)**
- **Blue Book / CD Extra (Enhanced CDs)**

It abstracts low-level complexity (sessions, TOC management, filesystem generation, and device quirks) into a **declarative disc model + backend execution system**.

---

## 2. Goals

### Primary Goals

- Provide a **single CLI tool** for all CD formats
- Support deterministic creation of:
  - Red Book audio discs
  - ISO data discs
  - CD Extra (Blue Book multi-session discs)
- Hide backend complexity (cdrdao, libburn, xorriso, etc.)
- Provide reproducible builds (disc definition → identical output)

### Secondary Goals

- Validation before burn (dry-run planning)
- Extensible backend system
- Scriptable + CI-friendly
- JSON-based intermediate disc representation

---

## 3. Non-Goals

- DVD/Blu-ray authoring
- DRM / copy protection
- Streaming / playback features
- GUI frontend (CLI only in v1)
- Online metadata lookup (optional future feature)

---

## 4. Supported Disc Formats

### 4.1 Red Book Audio CD

- Standard CD-DA (IEC 60908)
- PCM audio → CD audio conversion
- Optional CD-Text support

### 4.2 Data CD

- ISO9660 filesystem
- Optional:
  - Joliet
  - Rock Ridge

### 4.3 Blue Book (CD Extra)

- Session 1: Red Book audio
- Session 2: ISO9660 data
- Strict ordering enforcement

---

## 5. Architecture Overview

DiscCTL is composed of 4 core subsystems:

```
CLI Layer
   ↓
Disc Intent Parser
   ↓
Disc Graph Builder
   ↓
Session Planner (Blue Book engine)
   ↓
Backend Execution Layer
   ↓
Hardware Device Layer
```

---

## 6. Core Abstractions

## 6.1 Disc Graph Model

All input is converted into a graph structure:

```json
{
  "format": "bluebook",
  "label": "My Album",
  "sessions": [
    {
      "type": "audio",
      "tracks": [
        "track01.wav",
        "track02.wav"
      ]
    },
    {
      "type": "data",
      "filesystem": "iso9660",
      "source_dir": "./extras"
    }
  ]
}
```

---

## 6.2 Session Types

### AudioSession

- Input: WAV/FLAC
- Output: CDDA tracks
- Constraints:
  - 44.1kHz, 16-bit stereo required
  - gap/index handling supported

### DataSession

- Input: directory
- Output: ISO9660 image
- Options:
  - Rock Ridge
  - Joliet

---

## 6.3 Disc Formats Enum

```rust
enum DiscFormat {
  RedBook,
  DataCD,
  BlueBook
}
```

---

## 7. Session Planner (Critical Component)

The Session Planner converts the Disc Graph into a **physical burn plan**.

### Responsibilities

- Validate format rules
- Enforce session ordering
- Ensure hardware compatibility
- Compute burn sequence steps

### Blue Book Rules Engine

For `BlueBook`:

#### Required constraints:

- Session 1 MUST be Audio
- Session 2 MUST be Data
- Session 1 must be written first and not invalidated
- Disc must remain appendable after session 1
- Finalization occurs only after session 2

#### Output plan:

```json
{
  "steps": [
    {
      "action": "burn_audio_session",
      "finalize": false
    },
    {
      "action": "append_data_session",
      "filesystem": "iso9660"
    },
    {
      "action": "finalize_disc"
    }
  ]
}
```

---

## 8. Backend System

Backends are pluggable modules.

### 8.1 Required Backends

#### Audio Backend

Responsibilities:

- PCM → CDDA conversion
- track layout generation
- DAO writing

Suggested implementation:

- `cdrdao` integration OR libburn direct calls

Interface:

```rust
trait AudioBackend {
  fn write_audio_session(session: AudioSession, device: Device) -> Result<()>;
}
```

---

#### Data Backend

Responsibilities:

- ISO image generation
- multisession support

Suggested:

- xorriso / libisofs

```rust
trait DataBackend {
  fn create_iso(session: DataSession) -> Result<IsoImage>;
  fn append_session(image: IsoImage, device: Device) -> Result<()>;
}
```

---

#### Session Backend

Responsibilities:

- multi-session orchestration
- TOC management
- finalization rules

---

## 9. Device Abstraction Layer

Abstract optical drive operations:

```rust
trait DeviceBackend {
  fn open(path: &str);
  fn reserve();
  fn write_sector();
  fn finalize();
  fn supports_multisession() -> bool;
}
```

Must support:

- /dev/sr\*
- SCSI generic commands
- buffer underrun detection

---

## 10. CLI Design

### 10.1 Burn Command

```bash
discctl burn --format bluebook \
  --audio ./tracks/*.wav \
  --data ./extras \
  --label "Album Name"
```

---

### 10.2 Plan Command

```bash
discctl plan --format bluebook --input disc.json
```

Outputs full execution plan.

---

### 10.3 Validate Command

```bash
discctl validate disc.json
```

Checks:

- format correctness
- session rules
- file integrity
- device capability match

---

### 10.4 Debug Mode

```bash
discctl burn --debug --dry-run
```

Prints:

- session layout
- backend calls
- device operations

---

## 11. Backend Execution Flow

### Red Book Flow

```
validate → encode audio → burn DAO → finalize
```

### Data CD Flow

```
create ISO → burn single session → finalize
```

### Blue Book Flow

```
burn audio session (no finalization)
→ append ISO session
→ finalize disc
```

---

## 12. Error Handling Model

All errors must be structured:

```json
{
  "error": "SESSION_ORDER_INVALID",
  "message": "Data session cannot precede audio session in BlueBook format",
  "recoverable": false
}
```

---

## 13. Dependencies (Suggested)

### Core system tools

- libburn
- libisofs
- xorriso
- cdrdao (fallback audio precision mode)

### Optional

- ffmpeg (audio normalization)
- sox (resampling)

---

## 14. Testing Requirements

### Unit Tests

- session planner correctness
- graph validation
- format constraints

### Integration Tests

- ISO generation
- mock device burning
- multisession sequencing

### Hardware Tests (optional CI tag)

- actual CD-R burn verification
- multi-session detection validation

---

## 15. Edge Cases

Must explicitly handle:

- CD-RW vs CD-R differences
- drives without multisession support
- buffer underrun interruptions
- partial burn recovery
- ISO size limits (700MB CD-R)
- audio track padding/gaps

---

## 16. Future Extensions (Out of Scope for v1)

- DVD/Blu-ray support
- metadata lookup (Gracenote-like)
- GUI frontend
- image embedding standards for players
- UDF filesystem support

---

## 17. Key Design Principle

> Treat disc authoring as a **compiler problem**, not a burner tool problem.

- Input = declarative disc graph
- Output = deterministic physical session plan
- Execution = backend adapters

---

## 18. Success Criteria

The system is successful if:

- A Blue Book disc can be created with a single command
- Output is reproducible across runs
- Audio CDs remain universally compatible
- Data sessions are readable on all OSes
- Session ordering is deterministic and validated before burning

---
