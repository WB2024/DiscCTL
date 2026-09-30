<div align="center">

<img src="Images/Assets/Logo.jpeg" alt="RustyDisc" width="220">

# RustyDisc

**Rip, burn and archive CDs and DVDs — and fill USB sticks — from your browser.**<br>
Built in Rust for Linux. Runs headless in Docker, so the drive can live in a server and you drive it from anywhere.

[![Version](https://img.shields.io/badge/version-0.10-e8743b?style=flat-square)](https://github.com/WB2024/DiscCTL)
[![Docker pulls](https://img.shields.io/docker/pulls/wb20244/rustydisc?style=flat-square&logo=docker&logoColor=white&color=2496ED)](https://hub.docker.com/r/wb20244/rustydisc)
[![Rust](https://img.shields.io/badge/rust-1.85%2B-orange?style=flat-square&logo=rust)](docs/install.md)
[![License](https://img.shields.io/badge/license-MIT-blue?style=flat-square)](#license)
<a href="https://buymeacoffee.com/succinctrecords"><img src="https://img.shields.io/badge/Buy%20Me%20A%20Coffee-Support-yellow?logo=buy-me-a-coffee" alt="Buy Me A Coffee"></a>

[**Quick start**](#-quick-start) · [**Features**](#-features) · [**Screenshots**](#-a-quick-tour) · [**Docs**](#-documentation) · [**Support ☕**](#-support-rustydisc)

<br>

<img src="Images/Screenshots/disc-scan.png" alt="RustyDisc scanning an audio CD and matching it on MusicBrainz" width="900">

</div>

<br>

## Why RustyDisc?

- 🎧 **Rips you can trust.** Secure ripping, automatic MusicBrainz tags and cover art, and an [AccurateRip](https://www.accuraterip.com) check that tells you whether each track matches other people's rips.
- 🔥 **One tool for every disc.** Audio CDs, data CDs, Blue Book enhanced CDs, data DVDs and music DVDs that play in any DVD player. Everything is planned and validated *before* the drive is touched, so mistakes fail early instead of wasting a disc.
- 🔌 **Rusty Stick.** Put music on a USB stick filed exactly the way you want, converted to fit, without clobbering what's already there.
- 🖥️ **Runs where the drive is.** A web UI for your headless server or Proxmox box, with live progress in every open browser. Prefer a terminal? Everything is scriptable, with JSON output.

<br>

## ✨ Features

<table>
<tr>
<td width="50%" valign="top">

### 💿 Rip
- Secure extraction with error correction
- **FLAC, ALAC, WAV, AIFF, MP3, Opus, OGG**
- MusicBrainz tags, plus a built-in **search** when the match is wrong
- Cover art from Cover Art Archive and/or fanart.tv, saved and embedded
- **AccurateRip** verification (v1 + v2, offset tolerant)
- **Every MusicBrainz tag** on every file: IDs, sort names, disc, label, ISRCs, credits
- **Your own cover art**: upload a picture when ripping, or add one later in the Library
- **Edit tags in the Library**: album-wide or per track, every field including MusicBrainz IDs
- **Import into your library** with your own Picard naming script, or hand rips to **Lidarr**, matched by MusicBrainz ID
- **Archive mode:** disc structure, CD-Text and SHA-256 checksums, re-verifiable years later
- Enhanced CDs: audio and data sessions ripped separately

</td>
<td width="50%" valign="top">

### 🔥 Burn
- **Audio CD** with CD-Text from your tags
- **Data CD / Data DVD** (Blu-ray sizes too)
- **Enhanced CD** (Blue Book: audio, then data)
- **Enhanced Music DVD**: plays in any DVD player
- Files, folders or **M3U / M3U8 playlists**
- **Transcoding** to fit, and **multi-disc** planning that counts the converted size
- *Show plan* before you commit

</td>
</tr>
<tr>
<td width="50%" valign="top">

### 🔌 Rusty Stick
- **Identify** any stick: filesystem, partitions, what's on it, how it's organised
- **Inspect** it down to individual tracks
- **Reformat** the whole stick (exFAT, FAT32, ext4, NTFS)
- File music your way: A–Z, artist, album, disc, or your own pattern
- **Convert to fit**, with one-click size suggestions, and keep converted files for reuse (or delete them right away, your call)
- **Tidy existing music** and resolve duplicates by quality, date or rule

</td>
<td width="50%" valign="top">

### 🖥️ Web UI & platform
- Library with cover grid, in-browser playback and **one-click verification**
- Job history with full logs, cancel, and multi-disc prompts
- **Optional login** (Argon2, rate limited)
- Docker image, Dockge stack, Proxmox friendly
- Errors and progress are structured **JSON** for scripting
- `--mock` mode to try everything without a drive

</td>
</tr>
</table>

<br>

## 🚀 Quick start

**Docker** (recommended). Save as `compose.yaml`, adjust the marked lines, then `docker compose up -d`:

```yaml
services:
  rustydisc:
    image: wb20244/rustydisc:latest
    restart: unless-stopped
    ports: ["8080:8080"]                 # → http://<server>:8080
    devices:
      - /dev/sr0:/dev/sr0                # your optical drive
      - /dev/sg0:/dev/sg0                # needed for burning (find it: ls /sys/class/block/sr0/device/scsi_generic)
    cap_add: [SYS_RAWIO]
    volumes:
      - /path/to/CDRips:/rips            # where rips are saved
      - /path/to/burn-sources:/media:ro  # files you want to burn
      - ./config:/config                 # settings
```

Want a look around first? Add `RUSTYDISC_MOCK: "true"` for a simulated drive, no hardware needed.

**Command line:**

```bash
rustydisc rip --dir ~/Music/CDRips --archive      # rip the disc; the folder is named from MusicBrainz
rustydisc burn --format redbook --playlist mix.m3u8 --cd-text
rustydisc stick --target /run/media/you/STICK --folder ~/Music --transcode mp3:256
```

Running on Proxmox, adding a login, or letting RustyDisc mount USB sticks itself? See [Web UI & Docker](docs/web-ui-and-docker.md). Installing from source? See [Install](docs/install.md).

<br>

## 📸 A quick tour

### Rip, verify, keep

Scan the disc, see every track, and rip. Results are checked against AccurateRip, and each track reports how well it matched.

<p align="center"><img src="Images/Screenshots/rip-done.png" alt="A finished rip with cover art and AccurateRip results for every track" width="900"></p>

Wrong or missing match? **Find…** searches MusicBrainz from inside the app. Browse releases, preview track lists, and pick the right edition.

<p align="center"><img src="Images/Screenshots/musicbrainz-search.png" alt="Searching MusicBrainz for the right release" width="900"></p>

### Burn anything

Audio CDs from a hand-picked list, a folder or a playlist. **Show plan** tells you how many discs you need and how full each will be, before anything is written.

<p align="center"><img src="Images/Screenshots/burn.png" alt="An audio CD from eight tracks, with the plan" width="900"></p>

Turn a playlist into a **music DVD** that plays in any DVD player, one chapter per track.

<p align="center"><img src="Images/Screenshots/burn-music-dvd.png" alt="An enhanced music DVD built from a playlist" width="900"></p>

### Rusty Stick

Plug in a stick and RustyDisc **identifies** it: filesystem and what that means for you, partitions, how much music is already there, and whether it follows a layout it recognises.

<p align="center"><img src="Images/Screenshots/stick-identify.png" alt="Identifying a USB stick" width="900"></p>

**Inspect** it, from the root to single tracks, with tags and quality for every file.

<p align="center"><img src="Images/Screenshots/stick-inspect.png" alt="Browsing the contents of a stick" width="900"></p>

Choose the layout and what to do about music that's already there: leave it, or **re-file it** to match. Duplicates are matched by artist, album and track, so a different folder or format still counts. Replace when the new file is higher quality, lower quality, newer, always, or keep both.

<p align="center"><img src="Images/Screenshots/stick-organise.png" alt="Choosing a layout and how to handle existing music" width="900"></p>

The plan spells out every decision before a single byte is written, and how full the stick will be afterwards. If it won't fit, one click applies a bitrate that does.

<p align="center"><img src="Images/Screenshots/stick-plan.png" alt="A plan that replaces six low-quality files with FLAC and re-files the rest" width="900"></p>

<table>
<tr>
<td width="50%"><img src="Images/Screenshots/stick-write.png" alt="A finished write, converted to MP3 and flushed"></td>
<td width="50%"><img src="Images/Screenshots/stick-format.png" alt="Reformatting a whole stick"></td>
</tr>
<tr>
<td align="center"><sub><b>Write</b> in sorted order, then flush and eject safely</sub></td>
<td align="center"><sub><b>Reformat</b> the whole stick, partitions and all</sub></td>
</tr>
</table>

### Library, jobs and settings

Every rip in one place, with cover art, AccurateRip status and one-click checksum verification. Play tracks right in the browser.

<table>
<tr>
<td width="50%"><img src="Images/Screenshots/library.png" alt="Library grid of ripped albums"></td>
<td width="50%"><img src="Images/Screenshots/library-detail.png" alt="Album detail with AccurateRip results and players"></td>
</tr>
</table>

<table>
<tr>
<td width="50%"><img src="Images/Screenshots/jobs.png" alt="Job history with full logs"></td>
<td width="50%"><img src="Images/Screenshots/settings.png" alt="Settings for cover art and defaults"></td>
</tr>
<tr>
<td align="center"><sub><b>Jobs</b> keep running when you close the tab</sub></td>
<td align="center"><sub><b>Settings</b> for cover art sources and defaults</sub></td>
</tr>
</table>

Sharing the server? Turn on the optional **login** in Settings → Security, or set it from the environment.

<table>
<tr>
<td width="50%"><img src="Images/Screenshots/login.png" alt="The login page"></td>
<td width="50%"><img src="Images/Screenshots/settings-security.png" alt="Turning on the login in Settings"></td>
</tr>
</table>

<sub>Screenshots use RustyDisc's built-in <code>--mock</code> mode, which simulates the drive and sticks. The cover art and albums are demo data.</sub>

<br>

## ⚖️ How it compares

| | **RustyDisc** | K3b | Brasero | abcde | whipper |
|---|:---:|:---:|:---:|:---:|:---:|
| Rip audio CDs securely | ✅ | ✅ | — | ✅ | ✅ |
| Burn audio, data and DVD discs | ✅ | ✅ | ✅ | — | — |
| Blue Book / CD Extra | ✅ | mixed-mode | — | — | — |
| MusicBrainz tags + cover art | ✅ | CDDB | — | ✅ | ✅ |
| AccurateRip verification | ✅ | — | — | — | ✅ |
| Archive mode with checksums | ✅ | — | — | — | rip log |
| Write music to USB sticks | ✅ | — | — | — | — |
| Runs headless (no desktop) | ✅ | — | — | ✅ | ✅ |
| Web UI, usable from another machine | ✅ | — | — | — | — |
| Plan and validate before writing | ✅ | — | — | — | — |

<sub>Based on each project's documented behaviour; corrections welcome. RustyDisc's AccurateRip check is read-only: it reports the drive offset it detects but doesn't yet offset-correct the audio the way whipper and EAC do.</sub>

<br>

## 📚 Documentation

The README stays short on purpose. The details live here:

| | |
|---|---|
| 📥 [**Library import**](docs/library-import.md) | Move rips into your library with your own Picard naming script or through Lidarr, and the full MusicBrainz tags written on rip |
| 🖥️ [**Web UI & Docker**](docs/web-ui-and-docker.md) | Deployment, Proxmox, environment variables, the optional login |
| 🔌 [**Rusty Stick**](docs/rusty-stick.md) | Layouts and tokens, conversion, conflicts, reformatting, hot-plug mounting, CLI |
| 📀 [**Disc formats**](docs/disc-formats.md) | Red Book, Data, Blue Book, Data DVD and Music DVD explained |
| ⌨️ [**CLI reference**](docs/cli-reference.md) · [**examples**](docs/cli-examples.md) | Every command and flag, with worked examples |
| 🧩 [**Disc graph & errors**](docs/disc-graph.md) | The JSON disc description and structured error codes |
| 🛠️ [**Install from source**](docs/install.md) | Requirements and packages for Debian, Arch and Fedora |
| 🔬 [**Hardware notes**](docs/hardware-notes.md) | Drive behaviour, permissions, AccurateRip, CD-R vs CD-RW |
| 🏗️ [**Architecture & development**](docs/architecture.md) | How it works inside, building, testing |

<br>

## ☕ Support RustyDisc

<div align="center">

**RustyDisc is free and open source, and it's a one-person project.**

If it rescued a disc collection, saved you from a coaster, or turned a headless server into a proper ripping station, a coffee is the nicest way to say thanks. It goes straight into keeping the updates coming.

<a href="https://buymeacoffee.com/succinctrecords"><img src="https://img.shields.io/badge/%E2%98%95%20Buy%20me%20a%20coffee-Support%20RustyDisc-FFDD00?style=for-the-badge&logo=buy-me-a-coffee&logoColor=black" alt="Buy me a coffee"></a>

</div>

**What your support goes towards**

| | |
|---|---|
| 🚀 **New features** | Every feature above started as someone's "wouldn't it be nice if…". |
| 💿 **Real hardware** | Blank discs, drives and USB sticks, so burns and formats are tested on the real thing and not just in theory. |
| 🛠️ **Upkeep** | Keeping the Docker image, docs and dependencies fresh as things change. |

Can't chip in? That's completely fine. **⭐ Starring the repo**, sharing it with someone who still has a shelf of CDs, or opening an issue with your drive's quirks helps just as much.

<br>

## Contributing

Bug reports, drive quirks and pull requests are welcome. Start with [Architecture & development](docs/architecture.md).

## License

MIT.

<div align="center"><sub>Built with Rust. Burns and rips with precision.</sub></div>
