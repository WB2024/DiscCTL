# Disc formats

What each disc format is, and how RustyDisc builds it.

[← Back to the README](../README.md)

---

<p align="center">
  <img src="../Images/Assets/Untitled.jpeg" alt="Rusty Disc icon" width="160">
</p>

## Red Book (`--format redbook`)

The standard audio CD format (IEC 60908). Supports up to **99 tracks** and **74 minutes** of 44.1 kHz 16-bit stereo PCM audio. Written in Disc-At-Once (DAO) mode via `cdrdao`. CD-Text is stored in the R-W subchannels of the lead-in area.

Audio files must be **44.1 kHz, 16-bit stereo WAV** (CDDA spec). Non-compliant files are rejected at plan time. Use `--transcode wav` to convert first.

## Data CD (`--format datacd`)

A single-session ISO9660 data disc with Joliet and Rock Ridge extensions. Supports up to **700 MB** of content. Built and burned with `xorriso`. Volume label is written as uppercase ISO9660 (max 32 characters).

## Data DVD (`--format datadvd`)

Like a Data CD, but on a DVD (or Blu-ray): files, a folder or a playlist, with the same multi-disc splitting and transcoding. The image is ISO 9660 + Joliet + Rock Ridge + **UDF**, so files over 4 GB and long names work. Choose the blank disc with `--disc-size`:

| `--disc-size` | Disc | Capacity |
|---|---|---|
| `dvd` (default for DVDs) | DVD±R | 4.7 GB (4482 MiB) |
| `dvd-dl` | DVD±R DL | 8.5 GB (8147 MiB) |
| `bd` | BD-R | 25 GB (23866 MiB) |
| `bd-dl` | BD-R DL | 50 GB (47732 MiB) |

Any number of MB also works. A DVD keeps about 16 MB free. The plan counts converted sizes, so 2 GB of FLAC converted to MP3 320k shows as about 1.3 GB and one DVD instead of three CDs.

```bash
rustydisc plan --format datadvd --playlist "Magnum Opus.m3u8" --transcode mp3:320
rustydisc burn --format datadvd --playlist "Magnum Opus.m3u8" --transcode mp3:320 --label "Magnum Opus"
```

<p align="center"><img src="../Images/Screenshots/burn-data-dvd.png" alt="Data DVD plan: 120 FLAC files converted to MP3 fit on one DVD" width="820"></p>

## Enhanced Music DVD (`--format musicdvd`)

The DVD counterpart of an Enhanced CD: **music that plays in any DVD player, with a data folder on the same disc.** There is no Red Book audio on a DVD, so the music is authored as a **DVD-Video** disc:

- every track is a **chapter** (next/previous work, and the disc starts playing as soon as it is inserted), shown over a still picture: the cover art next to the tracks, an image you choose (`--dvd-still`), or a plain background;
- the audio is **Dolby Digital (AC-3) stereo** at 192, 256, 384 or 448 kbps (`--dvd-audio-kbps`, default 448) — lossy, but universally supported;
- the picture standard is **PAL** (default) or **NTSC** (`--dvd-standard`);
- an optional **data folder** (`--data`) goes in the root of the disc next to `VIDEO_TS`, so the same disc is a data DVD in a computer;
- more than 99 tracks become several titles that play one after another (DVD-Video allows 99 chapters per title).

Everything has to fit on **one** disc, and the plan tells you how full it is. As a guide, a track takes about 5 MB per minute at 448 kbps (including the picture), so a 4.7 GB DVD holds roughly 15 hours of music.

```bash
rustydisc burn --format musicdvd --playlist "Album.m3u8" --data ~/Bonus --label "My Album" --dvd-standard pal
```

<p align="center"><img src="../Images/Screenshots/burn-music-dvd.png" alt="Enhanced Music DVD: tracks plus a data folder on one DVD" width="820"></p>

Notes:

- **Lossless audio is not offered** — DVD-Video allows uncompressed LPCM, but the tools that author it produced non-standard streams in testing, so only Dolby Digital is available for now. DVD-Audio (`AUDIO_TS`) discs are not supported.
- The disc structure is verified against the DVD-Video layout (chapters, PAL/NTSC, AC-3, files in the image), but as with all burn features, check it on your own player and drive. The disc must be blank; a used DVD-RW has to be erased first.
- **`--iso-out disc.iso`** builds the finished disc image into a file instead of burning it (for DVD formats). Handy to inspect the result, test in a media player, or burn later with another tool.

## Blue Book / CD Extra (`--format bluebook`)

An enhanced CD with **two sessions**: Session 1 is a Red Book audio session (left open), Session 2 is appended as an ISO9660 data session, then the disc is finalized. Audio tracks play on any CD player; the data session is visible when inserted in a computer.

Session ordering (Audio → Data) is enforced at plan time. Blue Book requires a CD-R; CD-RW does not support the required multisession append.

