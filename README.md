# DiscCTL

A Linux CLI tool for authoring and burning optical discs. Supports Red Book Audio CDs, Data CDs (ISO9660), and Blue Book / CD Extra enhanced CDs.

## Install

Requires Rust (1.85+). System dependencies: `cdrdao`, `xorriso`. Optional: `ffmpeg` or `sox` for audio conversion.

```bash
cargo build --release
# binary at target/release/discctl
```

## Usage

### Burn a Red Book audio CD

```bash
discctl burn --format redbook --audio ./tracks/*.wav --label "My Album"
```

### Burn a data CD

```bash
discctl burn --format datacd --data ./files --label "Backup"
```

### Burn a Blue Book / CD Extra disc

```bash
discctl burn --format bluebook \
  --audio ./tracks/*.wav \
  --data ./extras \
  --label "My Album"
```

### From a disc graph JSON

```bash
discctl burn --input disc.json
discctl plan --input disc.json
discctl validate disc.json
```

### Dry run (plan without burning)

```bash
discctl burn --format bluebook --audio ./tracks/*.wav --data ./extras --dry-run
```

## Disc Graph JSON

All disc definitions share a common JSON schema:

```json
{
  "format": "bluebook",
  "label": "My Album",
  "sessions": [
    {
      "type": "audio",
      "tracks": ["track01.wav", "track02.wav"],
      "cd_text": { "title": "My Album", "artist": "Artist Name" }
    },
    {
      "type": "data",
      "source_dir": "./extras",
      "filesystem": "iso9660",
      "joliet": true,
      "rock_ridge": true
    }
  ]
}
```

Formats: `redbook`, `datacd`, `bluebook`

## Commands

| Command | Description |
|---------|-------------|
| `discctl burn` | Burn a disc |
| `discctl plan` | Print the burn plan without burning |
| `discctl validate <file>` | Validate a disc graph JSON |

All errors are emitted as structured JSON:

```json
{
  "error": "SESSION_ORDER_INVALID",
  "message": "Data session cannot precede audio session in BlueBook format",
  "recoverable": false
}
```

## Architecture

```
CLI → Disc Intent Parser → Disc Graph Builder → Session Planner → Backend Execution → Hardware
```

The Session Planner enforces all format rules before any hardware is touched. Blue Book session ordering (Audio → Data) is validated at plan time, not burn time.

Backends: `cdrdao` (audio DAO writing), `xorriso` (ISO9660 generation and multisession append).

## Development

```bash
cargo build
cargo test
cargo clippy
cargo fmt
```

Hardware tests require a physical CD-R drive and must be enabled explicitly:

```bash
cargo test --features hardware_tests
```
