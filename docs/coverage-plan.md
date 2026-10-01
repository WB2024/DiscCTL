# Ripping and burning coverage plan

A working checklist of the ripping and burning topics RustyDisc should cover, what it does today, and how each will be built. One item is done at a time; each is marked complete here once it has been built, tested and checked.

**Status key:** ⬜ not started · 🔧 in progress · ✅ complete

| # | Topic | Status | Today |
|---|---|---|---|
| 1 | [Rip log and disc report](#1-rip-log-and-disc-report) | ✅ | `rip.log` + `rip-report.json` on every rip (v1.3.0) |
| 2 | [Read offset correction](#2-read-offset-correction) | ✅ | Off / Auto / fixed number (v1.5.0) |
| 3 | [Burn and write speeds](#3-burn-and-write-speeds) | ✅ | Auto or chosen speed (v1.6.0) |
| 4 | [Jitter and read-error reporting](#4-jitter-and-read-error-reporting) | ✅ | Per-track clean / repaired / suspect (v1.7.0) |
| 5 | [Pregaps, hidden tracks, track boundaries](#5-pregaps-hidden-tracks-and-track-boundaries) | ✅ | Hidden track as 00; optional gap scan (v1.9.0) |
| 6 | [TOC anomaly checks](#6-toc-anomaly-checks) | ✅ | Disc checks before and after ripping (v1.10.0) |
| 7 | [Disc-at-once vs track-at-once](#7-disc-at-once-vs-track-at-once) | ✅ | Stated in the plan; explained (v1.10.1) |
| 8 | [Normalization](#8-normalization) | ⬜ | ReplayGain tags only |
| 9 | [Exact disc images](#9-exact-disc-images) | ⬜ | ISO build for data only |
| 10 | [Subchannels and subcode data](#10-subchannels-and-subcode-data) | ⬜ | CD-TEXT read; ISRC from MusicBrainz |
| 11 | [Batch ripping](#11-batch-ripping) | ⬜ | One disc at a time |
| 12 | [Streams](#12-streams) (URL inputs and stream choice) | ⬜ | File inputs only, default stream |
| 13 | [Forensic examination](#13-forensic-examination) | ⬜ | Parts exist, no combined report |
| 14 | [Non-compliant and difficult discs](#14-non-compliant-and-difficult-discs) | ⬜ | Not handled |

The order is deliberate: the rip log comes first because items 2, 4, 5, 6 and 13 all write into it.

Open questions that need a real disc or drive to settle are marked **(verify on hardware)**. Nothing in that state gets documented in the README as working until it has been tried.

---

## 1. Rip log and disc report
**Status:** ✅ complete in v1.3.0 (awaiting your check on a real rip)

**What was built:** every Red Book and Blue Book rip, from the CLI and the web UI, writes `rip-report.json` (structured) and `rip.log` (readable). They sit next to the audio, or in `metadata/` in archive mode (so the checksum manifest covers them). The log has the drive (model, firmware), reader and read mode, read-offset status, the table of contents, each track's file, AccurateRip result and a SHA-256 of the raw track as the drive delivered it, an AccurateRip summary, and notes (AccurateRip missing or partial, no MusicBrainz release). The Library album page has a **Rip log** card with the summary, notes, the full log and a download button (`GET /api/library/{name}/report`). Code: `src/rip/report.rs`. Later items add their findings to the same report.

**Today:** a rip writes `disc.json`, `cdtext.json`, `musicbrainz.json` and `checksums.json`. There is no human-readable log of how the rip went.

**Plan:**
- Write a `rip.log` next to the audio: RustyDisc version, date, drive (model and firmware), read mode, offset applied, per-track results (read quality, AccurateRip confidence, CRC), and the TOC.
- Build it as one structured value (also saved as `rip-report.json`) so later items can add sections without touching the text layer.
- Show the report in the Rip job panel and Library album page, with a download button.
- Add the log to the checksum manifest so edits are noticed.

**Done when:** every rip, from the CLI and the web UI, produces both files; the Library shows the report; tests cover formatting.

## 2. Read offset correction
**Status:** ✅ complete in v1.5.0 (awaiting your check on a real rip)

**What was built:** a drive read offset choice: **Off** (default, audio as returned), **Auto** (rip, let AccurateRip prove the shift, correct by it, re-check, and undo it if the result isn't at least as good) or a fixed number of samples. Available in Settings → Rip defaults, on the Rip page, and as `--offset off|auto|N`. The correction re-cuts the whole disc's audio at the track boundaries (borrowing from the neighbouring track; only the disc's very start or end is padded with silence). The rip log records the offset applied and how it was found. Code: `src/rip/offset.rs`, wired in `verify_with_offset` in `src/rip/mod.rs`. Tests include an end-to-end one: a disc ripped with a +6 and a -30 drive offset is detected, corrected, and then matches AccurateRip at shift 0 on every track.

**Left for you to check:** a real rip with Auto on (your drive should come out at +6 and re-verify at exactly 0). Writing the offset to your setting is a one-time choice; I left the default Off.

**Today:** AccurateRip matching searches ±2939 samples and reports a likely drive offset. The saved audio is not shifted.

**Plan:**
- Add a "drive read offset" setting (samples), plus a `--offset` flag on `rip`.
- Apply the shift when writing the WAV data, padding or trimming at the edges so track lengths stay correct across the disc.
- Add a "use detected offset" button when AccurateRip finds one, and record the offset in the rip log.
- Keep the current behaviour (no shift) as the default so nothing changes silently.

**Done when:** a test disc rip with a known offset matches AccurateRip at zero shift after correction. **(verify on hardware)**

## 3. Burn and write speeds
**Status:** ✅ complete in v1.6.0 (awaiting your check with a blank disc)

**What was built:** a write speed choice for every burn: **Auto** (default, as before) or an "x" multiple. The Burn page asks the drive when it opens (and on a CD/DVD format change) and fills the *Write speed* menu from what it offers for the disc in it (`xorriso -list_speeds`); Auto shows the speed it will use, and **Refresh** re-asks. (v1.6.1 made this automatic and visible after feedback that the first version's button gave no clear result.) On the command line it is `burn --speed N` and `plan --speed N`, and a disc graph file can carry `"speed": N`. The speed is shown in the plan, validated (1 to 100), and refused before anything is written if it is above what the drive reports. It is passed as `cdrdao --speed N` (audio CDs) or `xorriso -as cdrecord speed=Nc` / `speed=Nd` (data CDs, DVDs). Code: `src/backend/speed.rs`. Tests cover parsing, the drive's speed list (including the real drive's output), the limit check, and the exact tool arguments.

**Left for you to check (hardware):** that your drive actually writes at the chosen speed. Drives treat the number as an upper limit and some ignore it, so the proof is a burn with a blank disc and a look at the speed the tool reports. The speed list needs a blank disc in the drive; with a pressed CD in it the drive offers only one speed.

**Plan:**
- Add a speed choice in the burn options and Settings: Auto, or a specific speed. Offer the speeds the drive reports.
- Pass it through to cdrdao (`--speed`) and the data-burning tool; include it in the plan output.
- Default to Auto; document that slower is usually better for audio compatibility.

**Done when:** `plan` shows the speed, the burn command carries it, and validation rejects speeds the drive doesn't support. **(verify on hardware)**

## 4. Jitter and read-error reporting
**Status:** ✅ complete in v1.7.0 (awaiting your check)

**What was built:** cdparanoia now runs with `--stderr-progress`, so it reports every event (jitter fix-ups, corrections, scratches, skipped sectors, drift, dropped or duplicated samples, drive errors, cache warnings). RustyDisc parses those, maps each event to its track from the TOC, and gives every track a verdict: **clean** (routine edge jitter only), **repaired** (real trouble, fixed) or **suspect** (sectors skipped). Verdicts show in the job panel, the rip log (per track and a Read quality section, with Notes that say whether AccurateRip confirms the repaired or suspect tracks), and the report JSON. Real-hardware finding, now handled: the first read after a disc is loaded produces a harmless "unit attention" drive error (sense key 6) plus a couple of corrections, which is not counted against the disc. A **paranoia level** (Full / Fast / Off, `--paranoia`, Settings, Rip page) controls how hard cdparanoia checks. Code: `src/rip/readhealth.rs`, `src/rip/engine.rs`. Tests: the parser against lines captured from your real drive, the verdict rules, an end-to-end test with a stand-in cdparanoia, and the log output.

**Checked on your drive:** a full rip of the Morrissey disc read 18 clean, 0 repaired, 0 suspect, with all 18 tracks verified by AccurateRip. **Still to see:** a verdict on a disc with a real flaw (a light scratch or fingerprint); healthy discs read "clean".

**Today:** cdparanoia corrects jitter and rereads bad sectors, but RustyDisc doesn't report what happened.

**Plan:**
- Parse cdparanoia's progress output for rereads, skips, scratches and repairs per track.
- Classify each track as clean, repaired or suspect, and add it to the rip log and report.
- Add a paranoia level setting (default stays at full).

**Done when:** a scratched or marked disc produces a visibly different report from a clean one. **(verify on hardware)**

## 5. Pregaps, hidden tracks and track boundaries
**Status:** ✅ complete in v1.9.0 (hidden-track path awaiting a disc that has one)

**What was built:**
- **Hidden track one audio.** The TOC reveals it (track 1 starts after sector 0). Auto mode reads it with cdparanoia's "track 0", drops it if it is only silence, and otherwise saves it as `00. … Hidden track`. Skip leaves it out. It takes part in read offset correction (so track 1's start is right) and in the read-quality report, but not in loudness/DR. The rip log's table of contents and notes describe it, including when the drive can't read it.
- **Gaps between tracks.** An opt-in scan (`--gaps report|own-track`, Settings, Rip page) runs `cdrdao read-toc` (about five minutes, measured on your drive at 4 min 43 s) after AccurateRip, parses each track's gap, lists them in the rip log, and with `own-track` moves each gap from the end of the previous track to the start of the track it leads into. The audio stays continuous; only the cut points move.
- Code: `src/rip/gaps.rs`, `src/rip/engine.rs`, wired in `src/rip/mod.rs`. Tests: hidden-track detection and silence, a stand-in cdparanoia that serves track 0 (kept and silent cases), gap parsing, the gap scan against a stand-in cdrdao, and moving gaps (including never across a break in the disc).
- **Not done, on purpose:** a CUE sheet. With one file per track a CUE adds nothing about gaps; it becomes useful with whole-disc images (item 9).

**Left for you to check (hardware):** (1) a disc that really has a hidden track one, to prove the drive serves "track 0" the way the code expects; (2) a disc with real gaps (a live album) to see the scan's list and the `own-track` result. On your Morrissey disc the scan should report "No gaps between tracks were found".

**Plan:**
- Detect a pregap on track 1 longer than the standard 2 seconds from the TOC.
- Offer to rip it as a hidden track (`00`), and tag it clearly.
- Add a "keep gaps" mode for gapless and live albums, and optionally write a CUE sheet that preserves exact boundaries.
- Report any pregap found in the rip log.

**Done when:** a disc with a known hidden track yields that audio. **(verify on hardware: whether the drive allows reading before track 1)**

## 6. TOC anomaly checks
**Status:** ✅ complete in v1.10.0 (built while a rip was running; not deployed until that finished)

**What was built:** `analyzer::toc_check` examines the table of contents already read from the disc and reports, as Notes or Warnings: track numbers not 1, 2, 3 …, tracks out of order or with no length, a very short track, an unreadable end of disc (which stops AccurateRip), audio after a data session, a data track before the audio (mixed mode), a disc longer than 80 minutes, hidden audio before track 1, and a MusicBrainz track count that differs from the disc. They show on the Rip page's disc card, in `rustydisc info`, in the job output before ripping, in the rip log (*Disc checks*, with warnings repeated in the Notes) and in the report JSON. Tests cover every rule with synthetic tables of contents, plus the real Morrissey disc, which correctly comes out clean.

**Left for you to check:** a disc that is genuinely odd (a mixed-mode or Enhanced CD, or a damaged disc). Ordinary discs should show nothing.

**Plan:**
- Add validation rules in the analyzer: track 1 not starting at the standard offset, overlapping or zero-length tracks, an unexpected data track position, session inconsistencies, and a lead-out that doesn't fit.
- Return the findings as warnings in the analyzer output, the rip log and the web UI, before ripping starts.
- Cover each rule with unit tests built from synthetic TOCs.

**Done when:** each rule has a test and the Rip page shows warnings.

## 7. Disc-at-once vs track-at-once
**Status:** ✅ complete in v1.10.1

**What was built:** the write mode is now stated: `plan` and the Burn page's plan say how the disc will be written (audio CDs disc-at-once with no gaps added; Blue Book audio session disc-at-once kept open, then the data session, then closed; data CDs and DVDs written as one image and closed). `docs/hardware-notes.md` explains disc-at-once against track-at-once and why audio uses the former. No track-at-once option was added, as planned.

**Today:** audio burns use disc-at-once through cdrdao, with multisession for Blue Book.

**Plan:**
- Show the write mode explicitly in `plan` and the burn UI.
- Document why audio uses disc-at-once and when multisession changes that.
- No track-at-once option for audio, unless a drive needs it.

**Done when:** the plan output names the write mode and the docs explain it.

## 8. Normalization
**Status:** ⬜

**Today:** loudness measurement and ReplayGain tags, which leave the audio untouched.

**Plan:**
- Add an optional "apply gain before burning" step using the measured loudness, as a single ffmpeg gain stage ahead of CDDA encoding.
- Choose between per-track and album gain; album gain is the default so relative levels survive.
- Make it opt-in, show a clear warning that it changes the audio, and refuse gains that would clip.

**Done when:** a burn plan shows the gain applied and a test confirms peaks stay under full scale.

## 9. Exact disc images
**Status:** ⬜

**Today:** RustyDisc can build ISOs for data discs but can't capture an existing disc.

**Plan:**
- Add `rustydisc image` (and a web job) that reads a disc to BIN with a TOC/CUE using `cdrdao read-cd`, plus a checksum manifest.
- Add burning from such an image.
- Record the image details in the report.

**Done when:** a disc round-trips through image and burn with matching checksums. **(verify on hardware)**

## 10. Subchannels and subcode data
**Status:** ⬜

**Today:** CD-TEXT is read; ISRCs come from MusicBrainz, and there is no media catalog number (MCN).

**Plan:**
- Read ISRC and MCN from the disc itself (cdrdao can report them), compare with MusicBrainz, and note differences in the report.
- Write ISRC and CD-TEXT when burning, from the disc graph.
- Treat raw subchannel dumps as out of scope unless a drive makes it easy.

**Done when:** ISRCs read from a disc appear in the report. **(verify on hardware)**

## 11. Batch ripping
**Status:** ⬜

**Plan:**
- Add a "rip, eject, wait for the next disc, repeat" mode that uses the existing rip job and the eject setting.
- Detect disc changes, keep a running list of finished and failed discs, and stop on a chosen number of failures.
- Show it as one batch with a summary at the end.

**Done when:** three discs in a row rip unattended. **(verify on hardware)**

## 12. Streams
**Status:** ⬜

Both meanings of "streams" are in scope.

### 12a. Burning from a stream or URL
**What it means:** Today every track has to be a file already on disk. This would let a track come from a URL (an HTTP/HTTPS link to an audio file or a stream) or another source ffmpeg can read.

**Plan:**
- Let a track source in the disc graph be a URL as well as a path. Validation checks that it looks like a URL and, where possible, that it is reachable before any hardware is touched.
- Fetch and convert it through the existing converted-files cache to 44.1 kHz 16-bit stereo PCM, like any other input, so the burn step is unchanged.
- Show download and conversion progress in the job panel, and apply the cache retention settings to the result.
- Bound it sensibly: a size and duration limit, a timeout, and clear errors for unreachable links. Live endless streams are rejected, since a disc needs a finite length.
- Only for content the user has the right to use; the docs will say so.

**Done when:** a burn plan with a URL track validates, converts and burns (or dry-runs) like a file track, and tests cover validation and failure cases.

### 12b. Choosing between several audio streams in one file
**What it means:** Some files (video files, certain recordings, some containers) hold more than one audio stream, such as different languages, a commentary, or a stereo and surround mix. ffmpeg normally picks the default one, which may not be the one you want.

**Plan:**
- Read each input's streams with ffprobe and list them (index, codec, channels, language, title).
- Add an optional stream choice per track in the disc graph, defaulting to ffmpeg's current pick so nothing changes silently.
- Show the choice in the web UI when a file has more than one audio stream, and in `validate` and `plan` output.
- Pass the choice to ffmpeg when converting, and key the converted-files cache on it so different choices don't collide.

**Done when:** a test file with two audio streams converts the chosen one, and the choice appears in the plan.

## 13. Forensic examination
**Status:** ⬜

**Plan:**
- Add a "Disc report" that combines TOC, CD-TEXT, subcode findings, TOC warnings, read errors, AccurateRip results, checksums, integrity, loudness, DR and spectrograms for an album.
- Build on the rip report from item 1 and add an export (JSON plus a printable page).

**Done when:** one button produces the full report for any ripped album.

## 14. Non-compliant and difficult discs
**Status:** ⬜

**Plan:**
- Detect likely cases from the TOC and read behaviour: wrong track counts, long data sessions, unreadable regions.
- Retry with more careful read settings, keep unreadable sectors marked rather than hiding them, and report clearly.
- Preserve what the drive can read. No circumvention of copy protection.

**Done when:** a damaged or odd disc gives an honest partial result and report. **(verify on hardware)**

---

## Progress log

| Date | Item | Result |
|---|---|---|
| | | |
| 2026-10-01 | Side fix: file ownership | v1.4.0: RUSTYDISC_PUID/PGID/UMASK, fix-permissions command and Settings card; server set to 1000:1000, existing files fixed |
| 2026-10-01 | 1 Rip log and disc report | Built, unit tested, UI checked with a sample report; real rip pending your check |
| 2026-10-01 | 2 Read offset correction | Built (v1.5.0); tests prove detect, correct and re-verify at shift 0 for +6 and -30; real rip pending your check |
| 2026-10-01 | 3 Burn and write speeds | Built (v1.6.0); arguments verified by tests and a dry run; real burn at a chosen speed pending your check |
| 2026-10-01 | 4 Jitter and read-error reporting | Built (v1.7.0); parser tested on real drive output; verdicts on a flawed disc pending your check |
| 2026-10-01 | Side fixes (v1.8.0) | Loudness and DR results kept and shown again; cover size, format and quality rating; albums found by folder structure (Archive/Artist/Album) in Library and Import |
| 2026-10-01 | 5 Pregaps, hidden tracks, track boundaries | Built (v1.9.0); hidden track and gap handling tested with stand-in tools; real hidden-track and gap discs pending |
| 2026-10-01 | 6 TOC anomaly checks | Built (v1.10.0), unit tested on synthetic tables and the real disc (clean); an odd real disc pending |
| 2026-10-01 | 7 Disc-at-once vs track-at-once | Done (v1.10.1): write mode shown in the plan and explained |
