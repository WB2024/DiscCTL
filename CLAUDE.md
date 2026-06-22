# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

DiscCTL is a Linux CLI tool for authoring and burning optical discs (Red Book Audio CDs, Data CDs, and Blue Book/CD Extra enhanced CDs). It is implemented in Rust. The full design specification lives in `DiscCTL - DevSpec.md` and is the authoritative source for requirements and format rules.

The core design principle: treat disc authoring as a **compiler problem** — input is a declarative disc graph, output is a deterministic physical session plan, execution is handled by pluggable backend adapters.

## Build & Test Commands

```bash
cargo build                    # build
cargo build --release          # release build
cargo test                     # run all tests
cargo test <test_name>         # run a single test
cargo test -- --nocapture      # show stdout during tests
cargo clippy                   # lint
cargo fmt                      # format
```

Hardware-dependent tests should be gated with a `#[cfg(feature = "hardware_tests")]` feature flag or similar tag, as they require a physical CD-R drive.

## Architecture

The execution pipeline flows through 5 layers in order:

```
CLI Layer → Disc Intent Parser → Disc Graph Builder → Session Planner → Backend Execution Layer → Hardware Device Layer
```

### Disc Graph Model

All input (CLI flags, JSON files) is converted into a unified JSON-serializable disc graph before any planning or burning occurs. This is the intermediate representation. See `DiscCTL - DevSpec.md §6.1` for the schema.

### Session Planner

The most critical component. It converts the disc graph into a **physical burn plan** — a sequence of steps with explicit finalization rules. Blue Book requires strict ordering: Session 1 must be Audio (written without finalizing), Session 2 must be Data (appended, then finalized). The planner is responsible for enforcing all format constraints before any hardware is touched.

### Backend System

Three trait-based pluggable backends:

- **`AudioBackend`** — PCM→CDDA conversion, DAO writing (via `cdrdao` or `libburn`)
- **`DataBackend`** — ISO9660 image generation and multisession append (via `xorriso`/`libisofs`)
- **`DeviceBackend`** — low-level drive operations: open, reserve, write_sector, finalize, multisession detection

### CLI Commands

- `discctl burn` — burn a disc from flags or a disc graph JSON
- `discctl plan` — print the execution plan without burning
- `discctl validate` — validate a disc graph JSON for correctness before any hardware use
- `discctl burn --debug --dry-run` — print session layout and backend calls without touching hardware

### Error Model

All errors must be structured JSON with `error` (machine-readable code), `message` (human-readable), and `recoverable` (bool). See `DiscCTL - DevSpec.md §12`.

## Key Constraints

- Audio tracks must be 44.1kHz 16-bit stereo PCM before CDDA encoding
- Blue Book session order is always Audio → Data; violations must be caught at validation time
- ISO size limit is 700MB
- Must handle CD-RW vs CD-R differences in device layer
- Buffer underrun detection is required in the device abstraction
- System dependencies: `libburn`, `libisofs`, `xorriso`; optional: `ffmpeg`, `sox`
