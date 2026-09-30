# Installing from source

Requirements and installation for running RustyDisc without Docker.

[← Back to the README](../README.md)

---

# System Requirements

## Runtime dependencies

| Tool | Purpose | Required for |
|------|---------|-------------|
| `cdrdao` | Red Book audio DAO burning; CD-Text reading | Burn (audio), Rip |
| `xorriso` | ISO9660 generation, data burns, data extraction | Burn (data), Rip |
| `cdrecord` / `wodim` | TOC reading, disc state detection | Burn, Rip |
| `isoinfo` | Data session metadata (volume label, size) | Rip |
| `genisoimage` | Builds the UDF disc images for DVDs | Burn (DVD) |
| `dvdauthor` | Builds the DVD-Video structure of a Music DVD | Burn (Music DVD) |
| `cdparanoia` | Secure audio ripping | Rip (audio) |
| `ffmpeg` | Audio transcoding, encoding, cover art embedding | Burn (transcode), Rip (encode) |

## Install on Debian / Ubuntu

```bash
# Burning
sudo apt install cdrdao xorriso cdrecord

# Ripping
sudo apt install cdparanoia ffmpeg

# All at once
sudo apt install cdrdao xorriso cdrecord cdparanoia ffmpeg genisoimage dvdauthor
```

## Install on Arch Linux

```bash
sudo pacman -S cdrtools cdrdao xorriso cdparanoia ffmpeg
```

## Install on Fedora

```bash
sudo dnf install cdrtools cdrdao xorriso cdparanoia ffmpeg
```

## Rust toolchain

Requires **Rust 1.85 or newer**. Install via [rustup](https://rustup.rs):

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

---

# Installation

## From source (recommended)

```bash
git clone https://github.com/WB2024/DiscCTL.git
cd DiscCTL
cargo build --release
sudo install -m755 target/release/rustydisc /usr/local/bin/
```

## Local user install (no sudo)

```bash
cargo install --path .
# binary lands at ~/.cargo/bin/rustydisc
```

## Verify

```bash
rustydisc --version
rustydisc --help
```

