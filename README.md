<div align="center">

<img src="Images/Assets/Logo.jpeg" alt="RustyDisc" width="220">

# RustyDisc

**Rip, burn and archive CDs and DVDs — and fill USB sticks — from your browser.**<br>
Built in Rust for Linux. Runs headless in Docker, so the drive can live in a server and you drive it from anywhere.

[![Version](https://img.shields.io/badge/version-1.11-e8743b?style=flat-square)](https://github.com/WB2024/DiscCTL)
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

- 🎧 **Rips you can trust.** Secure ripping, every MusicBrainz tag on every file, cover art from where you want (or your own), and an [AccurateRip](https://www.accuraterip.com) check that tells you whether each track matches other people's rips.
- 🔥 **One tool for every disc.** Audio CDs, data CDs, Blue Book enhanced CDs, data DVDs and music DVDs that play in any DVD player. Everything is planned and validated *before* the drive is touched, so mistakes fail early instead of wasting a disc.
- 📥 **Straight into your library.** File rips with your own Picard naming script (or build one in a few clicks), or hand them to Lidarr, matched by MusicBrainz ID.
- 🔌 **Rusty Stick.** Put music on a USB stick filed exactly the way you want, converted to fit, without clobbering what's already there.
- 🖥️ **Runs where the drive is.** A web UI for your headless server or Proxmox box, with live progress in every open browser. Prefer a terminal? Everything is scriptable, with JSON output.

<br>

## ✨ Features

<table>
<tr>
<td width="50%" valign="top">

### 💿 Rip
- Secure extraction with error correction
- **FLAC, ALAC, WAV, AIFF, MP3, AAC, Opus, OGG**, each with quality choices (best by default)
- **Proper file ownership:** set a user, group and umask and everything RustyDisc creates belongs to them instead of root, with a one-click fix for existing files ([details](docs/web-ui-and-docker.md#file-ownership))
- **Read offset correction:** fixes your drive's read offset in the files (automatically, using AccurateRip, or by a number you set) so rips match any other drive's exactly ([details](docs/audio-quality.md#read-offset-correction))
- **Level the audio before burning:** optional album or per-track normalization to a target loudness, with clip protection, for audio CDs ([details](docs/hardware-notes.md#levelling-the-audio-before-burning-normalization))
- **Write speed:** pick the burn speed (Auto, or any speed the drive offers for your blank disc), in the web UI or with `--speed` ([details](docs/hardware-notes.md#write-speed))
- **Read quality per track:** every track is judged clean, repaired or suspect from cdparanoia's own report, with a choice of how hard it checks ([details](docs/audio-quality.md#how-cleanly-the-disc-was-read))
- **Hidden tracks and gaps:** hidden audio before track 1 is ripped as track 00, and gaps between tracks can be found and kept with the track they lead into ([details](docs/audio-quality.md#hidden-tracks-and-gaps-between-tracks))
- **Disc checks:** the table of contents is examined before ripping and anything unusual (damaged numbering, an unreadable end of disc, mixed-mode, a track count that doesn't match MusicBrainz…) is explained in plain words ([details](docs/audio-quality.md#disc-checks))
- **Rip log:** every rip saves a readable `rip.log` and a JSON report: drive, read settings, table of contents, per-track AccurateRip results and checksums, and notes
- **Quality report:** bit depth, sample rate and bitrate after ripping, plus an integrity test, **loudness / ReplayGain**, **dynamic range (DR)** and **spectrograms** that expose fake lossless
- **Every MusicBrainz tag** on every file: IDs, sort names, disc, label, ISRCs, credits
- MusicBrainz **search** when the match is wrong
- Cover art from Cover Art Archive and/or fanart.tv, or **upload your own**
- **AccurateRip** verification (v1 + v2, offset tolerant)
- **Archive mode:** structure, CD-Text and SHA-256 checksums, re-verifiable years later
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
- *Show plan* before you commit; kept conversions are reused

</td>
</tr>
<tr>
<td width="50%" valign="top">

### 📥 Library & import
- **Picard naming scripts** run as written, plus a **script builder** with six presets
- **Import** rips: move, copy or hard link, with conflict rules by quality, date or name
- **Lidarr** import: matched by MusicBrainz ID, artists and albums added unmonitored
- **Edit tags** per track or album-wide, and **add or change cover art**, right in the Library
- Browse, play, download and **verify** every rip, and check its **audio quality**

</td>
<td width="50%" valign="top">

### 🔌 Rusty Stick
- **Identify** any stick: filesystem, partitions, contents, layout
- **Inspect** it down to individual tracks
- **Reformat** the whole stick (exFAT, FAT32, ext4, NTFS)
- File music your way: A–Z, artist, album, disc, or your own pattern
- **Convert to fit**, with one-click size suggestions
- **Tidy existing music** and resolve duplicates by quality, date or rule

</td>
</tr>
<tr>
<td colspan="2" valign="top">

### 🖥️ Web UI & platform
Live job progress in every open tab, with full history and logs · **optional login** (Argon2, rate limited) · Docker image, Dockge stack and Proxmox notes · converted files **kept for reuse** or deleted right away, your call · errors and progress as structured **JSON** for scripting · `--mock` mode to try everything without a drive

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
rustydisc import --rip "~/Music/CDRips/Artist - Album (2001)" --library ~/Music
rustydisc burn --format redbook --playlist mix.m3u8 --cd-text
rustydisc stick --target /run/media/you/STICK --folder ~/Music --transcode mp3:256
```

Running on Proxmox, adding a login, letting RustyDisc mount USB sticks itself, or pointing Import at your library? See [Web UI & Docker](docs/web-ui-and-docker.md) and [Library import](docs/library-import.md). Installing from source? See [Install](docs/install.md).

<br>

## 📸 A quick tour

### 💿 Rip, verify, keep

Scan the disc, see every track, and rip. Results are checked against AccurateRip, and each track reports how well it matched.

<p align="center"><img src="Images/Screenshots/rip-done.png" alt="A finished rip with AccurateRip results for every track" width="900"></p>

<table>
<tr>
<td width="50%"><img src="Images/Screenshots/musicbrainz-search.png" alt="Searching MusicBrainz for the right release"></td>
<td width="50%"><img src="Images/Screenshots/rip-cover.png" alt="Uploading your own cover art before ripping"></td>
</tr>
<tr>
<td align="center"><sub><b>Wrong match?</b> Search MusicBrainz from inside the app</sub></td>
<td align="center"><sub><b>Your own cover:</b> saved, embedded, or both</sub></td>
</tr>
</table>

### 🎚 Audiophile tools

Pick the quality when you rip (the best is the default), and see exactly what you got: **bit depth, sample rate and bitrate** for every file, an integrity test that decodes each track, **loudness with ReplayGain** you can write to the files, and the album's **dynamic range (DR)**, the loudness-war number, with a link to compare pressings on the Dynamic Range DB and a **userscript** that shows DR on MusicBrainz and pre-fills the DB's upload form. [More →](docs/audio-quality.md)

<p align="center"><img src="Images/Screenshots/library-quality.png" alt="The audio quality card: format, bit depth, bitrate, integrity test and loudness" width="900"></p>

<table>
<tr>
<td width="50%"><img src="Images/Screenshots/library-spectrogram.png" alt="A spectrogram of a genuine CD rip, filled to 22 kHz"></td>
<td width="50%"><img src="Images/Screenshots/library-spectrogram-lossy.png" alt="A spectrogram of a lossless file that started as a 128 kbps MP3, with a hard ceiling at 16 kHz"></td>
</tr>
<tr>
<td align="center"><sub><b>Genuine CD audio:</b> full spectrum</sub></td>
<td align="center"><sub><b>"Lossless" from an MP3:</b> the 16 kHz ceiling gives it away</sub></td>
</tr>
</table>

### 🔥 Burn anything

Audio CDs from a hand-picked list, a folder or a playlist. **Show plan** tells you how many discs you need and how full each will be, before anything is written.

<p align="center"><img src="Images/Screenshots/burn.png" alt="An audio CD from eight tracks, with the plan" width="900"></p>

<table>
<tr>
<td width="50%"><img src="Images/Screenshots/burn-music-dvd.png" alt="An enhanced music DVD built from a playlist"></td>
<td width="50%"><img src="Images/Screenshots/burn-data-dvd.png" alt="A data DVD with the audio converted to MP3"></td>
</tr>
<tr>
<td align="center"><sub><b>Music DVD</b> that plays in any DVD player</sub></td>
<td align="center"><sub><b>Data DVD</b>, converted to fit</sub></td>
</tr>
</table>

More than one disc's worth? The plan counts the size *after* conversion and prompts you to swap discs.

<p align="center"><img src="Images/Screenshots/burn-plan.png" alt="A folder that needs two CDs, with the fill of each" width="900"></p>

### 📥 Library, tags and covers

Every rip in one place, with cover art, AccurateRip status and one-click checksum verification. Play tracks right in the browser.

<p align="center"><img src="Images/Screenshots/library.png" alt="Library grid of ripped albums" width="900"></p>

<table>
<tr>
<td width="50%"><img src="Images/Screenshots/library-detail.png" alt="Album detail with cover, actions and players"></td>
<td width="50%"><img src="Images/Screenshots/library-tags.png" alt="Inspecting and editing every tag of one track"></td>
</tr>
<tr>
<td align="center"><sub><b>Album view:</b> play, verify, change the cover</sub></td>
<td align="center"><sub><b>Tags:</b> every field, including MusicBrainz IDs</sub></td>
</tr>
<tr>
<td width="50%"><img src="Images/Screenshots/library-album-tags.png" alt="Editing album-wide tags"></td>
<td width="50%"><img src="Images/Screenshots/library-cover.png" alt="Replacing the cover art of a finished rip"></td>
</tr>
<tr>
<td align="center"><sub><b>Album-wide edits</b> written to every track</sub></td>
<td align="center"><sub><b>Forgot the cover?</b> Add it later, embedded and saved</sub></td>
</tr>
</table>

### 🗂 Import into your library

Move finished rips into your music library, named by **your own Picard naming script**. The plan shows every destination before a file moves.

<p align="center"><img src="Images/Screenshots/import.png" alt="Choosing rips to import, with the import options" width="900"></p>

Or hand them to **Lidarr**. Each rip carries its MusicBrainz IDs, so Lidarr is told exactly which album it is; artists it doesn't have are added unmonitored, and you see how it matched every file first.

<p align="center"><img src="Images/Screenshots/import-lidarr.png" alt="The Lidarr plan: which artists exist, which will be added, and how files match" width="900"></p>

<p align="center"><img src="Images/Screenshots/jobs.png" alt="Job history with the full log of an import" width="900"></p>

### 🛠 Build a naming script

No Picard script? Pick a preset and adjust: artist folders, sort names, year position, multi-disc folders, featured artists, Windows-safe names. Six kinds of made-up release show exactly how each choice files them.

<p align="center"><img src="Images/Screenshots/settings-script-builder.png" alt="The naming script builder with presets, options and live examples" width="900"></p>

<table>
<tr>
<td width="50%"><img src="Images/Screenshots/settings-library.png" alt="Library settings with the naming script and import defaults"></td>
<td width="50%"><img src="Images/Screenshots/settings-lidarr.png" alt="Lidarr settings"></td>
</tr>
<tr>
<td align="center"><sub><b>Music library:</b> paste any Picard script</sub></td>
<td align="center"><sub><b>Lidarr:</b> address, key, profiles, path mapping</sub></td>
</tr>
</table>

### 🔌 Rusty Stick

Plug in a stick and RustyDisc **identifies** it: filesystem and what that means for you, partitions, how much music is already there, and whether it follows a layout it recognises.

<p align="center"><img src="Images/Screenshots/stick-identify.png" alt="Identifying a USB stick" width="900"></p>

<table>
<tr>
<td width="50%"><img src="Images/Screenshots/stick-inspect.png" alt="Browsing the contents of a stick"></td>
<td width="50%"><img src="Images/Screenshots/stick-format.png" alt="Reformatting a whole stick"></td>
</tr>
<tr>
<td align="center"><sub><b>Inspect</b> from the root down to single tracks</sub></td>
<td align="center"><sub><b>Reformat</b> the whole stick, partitions and all</sub></td>
</tr>
</table>

Choose the layout and what to do about music that's already there: leave it, or **re-file it** to match. Duplicates are matched by artist, album and track, so a different folder or format still counts.

<p align="center"><img src="Images/Screenshots/stick-organise.png" alt="Choosing a layout and how to handle existing music" width="900"></p>

The plan spells out every decision before a single byte is written, and how full the stick will be afterwards. If it won't fit, one click applies a bitrate that does.

<p align="center"><img src="Images/Screenshots/stick-plan.png" alt="A plan that replaces six low-quality files with FLAC and re-files the rest" width="900"></p>

<p align="center"><img src="Images/Screenshots/stick-write.png" alt="A finished write, converted to MP3 and flushed" width="900"></p>

### ⚙️ Settings, security and tools

Cover art sources in the order you want, rip defaults (format, quality, ReplayGain), converted files kept for reuse or deleted right away, and an optional login for shared servers.

<table>
<tr>
<td width="50%"><img src="Images/Screenshots/settings.png" alt="Settings for cover art and defaults"></td>
<td width="50%"><img src="Images/Screenshots/settings-rip-defaults.png" alt="Rip defaults: format, quality and ReplayGain"></td>
</tr>
<tr>
<td align="center"><sub><b>Cover art</b> sources and order</sub></td>
<td align="center"><sub><b>Rip defaults:</b> format, quality, ReplayGain</sub></td>
</tr>
<tr>
<td width="50%"><img src="Images/Screenshots/settings-convert.png" alt="How long converted files are kept"></td>
<td width="50%"><img src="Images/Screenshots/settings-security.png" alt="Turning on the login"></td>
</tr>
<tr>
<td align="center"><sub><b>Converted files:</b> keep or delete</sub></td>
<td align="center"><sub><b>Optional login</b> for shared servers</sub></td>
</tr>
<tr>
<td width="50%"><img src="Images/Screenshots/login.png" alt="The login page"></td>
<td width="50%"><img src="Images/Screenshots/tools.png" alt="Disc recovery, CD-RW blanking and dependency checks"></td>
</tr>
<tr>
<td align="center"><sub>The login page</sub></td>
<td align="center"><sub><b>Tools:</b> recovery, blanking, dependency check</sub></td>
</tr>
</table>

<sub>Screenshots use RustyDisc's built-in <code>--mock</code> mode, which simulates the drive and sticks, with a demo library. The cover art and albums are demo data.</sub>

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
| Quality choices, ReplayGain, dynamic range, spectrograms | ✅ | — | — | ✅ | — |
| Import into a library (Picard scripts, Lidarr) | ✅ | — | — | — | — |
| Edit tags and covers after ripping | ✅ | ✅ | — | — | — |
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
| 📥 [**Library import**](docs/library-import.md) | Picard naming scripts and the built-in script builder, importing through Lidarr, editing tags and covers, and the full MusicBrainz tags written on rip |
| 🎚 [**Audio quality**](docs/audio-quality.md) | Encoder quality choices, the quality report, integrity test, loudness and ReplayGain, spectrograms |
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
