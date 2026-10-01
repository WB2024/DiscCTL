# Hardware notes

Drive behaviour, permissions, AccurateRip and CD-R vs CD-RW.

[← Back to the README](../README.md)

---

## Secure ripping

Audio extraction uses **cdparanoia** in its full paranoia mode: it reads sectors with overlap, compares the results, re-reads on disagreement, and corrects jitter, so scratched or marginal discs still come out as close to bit-perfect as the drive allows.

## AccurateRip

After extraction, and before encoding, every track is checked against the [AccurateRip](http://www.accuraterip.com/) database, which holds checksums of the same pressing ripped by other people. RustyDisc looks the disc up by its table of contents, computes the **v1 and v2** checksums for each track, and reports a per-track confidence (the number of independent rips that agree).

Drives read audio a few dozen to a few hundred samples early or late (the "read offset"), and cdparanoia does not correct for it, so an accurate rip would normally fail to match. RustyDisc therefore also matches tracks at every shift of up to ±2939 samples (v1 entries are searched cheaply on every track; for v2 entries the shift is discovered on a short track, nearest-to-zero first, and then applied to the rest). A match at a shift still proves the audio is identical, and when all tracks agree on the same shift it is reported as a hint of your drive's offset. By default the saved audio is **not** shifted, so it is exactly what the drive returned; turn on [read offset correction](audio-quality.md#read-offset-correction) to fix the offset in the files themselves.

What to expect:

- **Verified, confidence ≥ 2** — the rip matches at least two other people's rips.
- **Confidence 1** — matches one other rip; good, but weaker evidence.
- **No match** — a damaged read, or a different pressing than the database holds. Try the rip again, or compare with another drive.
- **Not in the database** — nothing to compare against; this says nothing about the rip.

The check needs internet access (use `--no-accuraterip` to skip it) and never fails a rip. In archive mode the full report is stored as `metadata/accuraterip.json`. The check is skipped when the disc's table of contents can't be read completely. The checksum maths is tested against the reference implementation and real disc IDs, but it has not yet been run against a large range of physical discs, so please report any mismatch that looks wrong.

## User permissions

No `sudo` is required for ripping if your user is in the `cdrom` group:

```bash
sudo usermod -aG cdrom $USER   # then log out and back in
```

## CD-RW vs CD-R

| Feature | CD-R | CD-RW |
|---------|------|-------|
| Red Book burn | Yes | Yes |
| Data CD burn | Yes | Yes |
| Blue Book / multisession | Yes | **No** |
| Can be erased and reused | No | Yes |
| Rippable | Yes | Yes |

## Write speed

Burns use the speed the drive picks unless you choose one. Slower writing is often kinder to audio discs: older players are fussier about discs burned fast, and cheap media can burn better below its rated speed.

- **Web UI:** the Burn page asks the drive when it opens (and when you switch between CD and DVD formats) and fills the *Write speed* menu with what it offers for the disc in it, for example 8x, 16x, 24x. **Auto** says what it will use ("Auto: the drive chooses (up to 24x)"), which is the fastest. Pick a slower speed if you want one; for audio CDs the nearest to 16x is marked as a good choice. Press **Refresh** after changing the disc. With no blank disc in the drive it can't list speeds and Auto is used.
- **Command line:** `rustydisc burn --speed 8 ...` (a multiple of the media's base speed; CD 1x = 176.4 kB/s, DVD 1x = 1385 kB/s). `rustydisc plan --speed 8` shows it in the plan. A disc graph file can carry `"speed": 8`; a `--speed` on the command line wins.

RustyDisc refuses a speed above what the drive reports for the disc in it, before writing anything. A speed between two the drive offers is accepted: drives treat the number as an upper limit and may settle on a nearby speed, and some quietly ignore it. Audio CDs pass it to `cdrdao --speed`; data CDs and DVDs pass it to `xorriso` as `speed=8c` (CD) or `speed=4d` (DVD), so the number can't be read as the wrong media's speed.
