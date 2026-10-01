# Audio quality

[← Back to the README](../README.md)

RustyDisc lets you choose how a rip is encoded, tells you what you actually got, and gives you the tools to check it.

## Choosing the quality

On the **Rip** page (and as a default in **Settings → Rip defaults**), pick a format and a quality. The first choice for each format is the best, and it's the default.

| Format | Choices |
|---|---|
| **FLAC** (lossless) | Best compression (level 8, default) · maximum (12) · standard (5) · fastest (0). Every level is bit-perfect; only size and speed change. |
| **ALAC**, **WAV**, **AIFF** (lossless) | One choice each: the disc's audio exactly as read, 16-bit / 44.1 kHz. |
| **MP3** | V0 (~245 kbps VBR, default) · 320 kbps · V2 (~190 kbps) · 192 kbps · 128 kbps |
| **AAC** | 320 · 256 · 192 · 128 kbps |
| **Opus** | 320 (default) · 192 · 128 · 96 kbps |
| **OGG Vorbis** | Quality 10 (~500 kbps, default) · 8 · 6 · 4 |

A CD holds 16-bit / 44.1 kHz stereo audio, so a lossless rip is the best that exists: there's nothing more to capture. From the command line: `rustydisc rip --format flac --quality 12`, `--format mp3 --quality cbr320`, `--format aac --quality 256`.

## What did I get?

Right after a rip, the job shows a **Quality** line: the format, bit depth, sample rate, channels and average bitrate, with a **lossless · CD quality** badge when it matches the disc.

In the **Library**, every rip has an **Audio quality** card:

- **Format, bit depth, sample rate and bitrate** for the set and for each file, with notes when files differ or aren't CD quality.
- **Test the audio** decodes every file end to end and reports any that don't decode cleanly (a damaged download, a bad disk sector).
- **Measure loudness** shows each track's integrated loudness (LUFS), loudness range and true peak (flagging any that clip), and the album's. **Write ReplayGain tags** adds standard ReplayGain 2.0 track and album gain and peak to the files, so players can level tracks and albums without touching the audio. You can also switch this on for every rip in Settings.
- **Spectrogram** draws any track's frequencies over time. Real CD audio fills the picture up to 20 kHz and beyond; a sharp ceiling around 16 kHz means the "lossless" file was once an MP3 (or similar) and only the wrapper is lossless.

RustyDisc's own numbers come from `ffprobe` and `ffmpeg` (EBU R128 loudness), the same tools that encode the audio.

## Dynamic range (DR)

RustyDisc colour-codes the result and says what it means in plain English:

| DR | Rating | Meaning |
|---|---|---|
| 14 and up | **Excellent** (green) | Very dynamic; quiet parts really quiet, loud parts hit hard |
| 11–13 | **Good** (light green) | Natural and open, some compression |
| 8–10 | **Average** (yellow) | Typical modern pop and rock |
| 6–7 | **Poor** (orange) | Noticeably squashed, tiring over a whole album |
| 5 and below | **Bad** (red) | A loudness-war master; look for another pressing |

The [Dynamic Range Database](https://dr.loudness-war.info/) lists a "DR" number for albums: how far the peaks rise above the loud parts of the music. A high number (14 and up) is natural and dynamic; a low one (under 8) is a loudness-war master squashed to be as loud as possible. Different pressings of the same album often score very differently, so it's the number to check when choosing an edition.

RustyDisc measures it itself, with the published DR method (3-second blocks per channel, the second-highest peak against the RMS of the loudest 20% of blocks, averaged over the channels; an album's DR is the average of its tracks'). A value can differ from a listed one by a point, because this is a reimplementation of the meter, not the meter itself.

- **In the Library:** the **Dynamic range** button on the Audio quality card shows each track's DR, peak and RMS, the album's DR with what it means, and lets you **save DR as tags** (`DYNAMIC RANGE` and `ALBUM DYNAMIC RANGE`, the names the desktop DR meters use), **download a DR log** in the usual layout, and **compare on the Dynamic Range DB**, which opens the database's listing for that artist.
- **When ripping:** tick **Measure dynamic range (DR)** (or make it a default in Settings) and the job reports the album's DR and writes the tags. On the command line: `rustydisc rip --dynamic-range`.

The database has no public API or export, so RustyDisc links to it instead of reading it.

### Userscript for the Dynamic Range DB

`userscripts/dynamic-range-db.user.js` works with Tampermonkey, Violentmonkey or Greasemonkey. Install it from **Settings → Dynamic Range DB userscript** (served by your RustyDisc), or from [GitHub](https://raw.githubusercontent.com/WB2024/DiscCTL/main/userscripts/dynamic-range-db.user.js) for automatic updates.

- **On MusicBrainz release pages** a panel lists what the Dynamic Range DB holds for that release, with DR, min/max, codec and source. Entries with the same barcode or catalogue number are marked, since pressings differ a lot. If there's no entry it links to the upload form, pre-filled from MusicBrainz.
- **On the Dynamic Range DB** every album gets a MusicBrainz search link, and album pages link to a search by barcode or catalogue number.
- **Submitting:** in the Library, **Dynamic range → Submit to Dynamic Range DB** opens the upload form with artist, album, year, codec, source, label, catalogue number, barcode and MusicBrainz link filled in and the DR log attached as `dr.txt`. You check it and press submit yourself; nothing is sent automatically. RustyDisc's DR can differ by a point from the original meter, so only submit values you trust.

## The rip log

Every rip writes two files describing how it went: `rip.log` to read, and `rip-report.json` for scripts. They sit next to the audio, or in `metadata/` in archive mode.

The log records the drive (model and firmware), the reader and its mode, whether a read offset was corrected, the disc's table of contents, and for each track the file written, its AccurateRip result and a SHA-256 of the raw track exactly as the drive delivered it. A Notes section lists anything worth knowing, such as a disc missing from AccurateRip or only some tracks matching. In the web UI the Library album page shows it in a **Rip log** card, with a download button.

## Read offset correction

Every drive reads the disc a fixed number of samples early or late. The audio is fine, but the whole disc is shifted, so a rip from your drive differs by a few samples from the same disc ripped on another drive. AccurateRip still recognises it (it searches for the shift, and the rip log reports it), but the files themselves are slightly off.

**Settings → Rip defaults → Drive read offset** (or the option on the Rip page, or `--offset` on the command line) chooses what to do:

| Choice | What happens |
|---|---|
| **Off** (default) | The audio is saved exactly as the drive returned it. |
| **Auto** | The rip is checked against AccurateRip first. If every verified track matches at the same shift, RustyDisc corrects the audio by that amount and checks again. If the second check is not at least as good, the correction is undone and the original audio kept. |
| **A number** | Always correct by that many samples, for example `6` or `-30` (the value your drive has in the AccurateRip drive list). |

Correcting shifts the whole disc's audio and re-cuts it at the track boundaries: each track loses its first N samples and takes the next track's first N (the reverse for a negative offset), so nothing is lost between tracks. Only the very start or end of the disc, where there is nothing to borrow from, is filled with silence (a drive can't read past the lead-out), and the rip log says how many samples that was. The log also records the offset applied, and whether it was found automatically or set by you.

Auto needs AccurateRip, so it does nothing for discs that aren't in the database. For those, set the number yourself once; your drive's offset doesn't change.

```bash
rustydisc rip --dir /rips --offset auto     # let AccurateRip decide
rustydisc rip --dir /rips --offset 6        # your drive's known offset
```

## How cleanly the disc was read

cdparanoia reads each sector more than once and compares, fixing what it can. RustyDisc listens to its report while ripping and gives every track a plain verdict, shown in the job, in the rip log and on the Library album page:

| Verdict | Meaning |
|---|---|
| **Clean** | Nothing beyond routine edge jitter, which every drive produces and cdparanoia quietly fixes. The jitter count is listed for interest only. |
| **Repaired** | cdparanoia hit real trouble (corrections, a scratch, dropped or duplicated samples, a drive error) and fixed it. AccurateRip tells you whether the result is right. |
| **Suspect** | cdparanoia had to skip sectors, so the audio may glitch there. Clean the disc and rip again, or try another drive. |

The rip log's Notes say which of these AccurateRip did or didn't confirm. A drive reset right after a disc is loaded (a "unit attention" error) is normal and is not held against the disc. If cdparanoia warns the drive appears to **cache** audio reads, that is noted too, because caching can hide errors.

**Read checking** (Settings → Rip defaults, the Rip page, or `--paranoia full|fast|off` on the command line) chooses how hard cdparanoia checks: **Full** (default) rereads and verifies every sector; **Fast** only does overlap checking; **Off** does no checking and is quickest. Lower levels make the verdicts less meaningful, and the log says so.
