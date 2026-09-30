# Hardware notes

Drive behaviour, permissions, AccurateRip and CD-R vs CD-RW.

[← Back to the README](../README.md)

---

## Secure ripping

Audio extraction uses **cdparanoia** in its full paranoia mode: it reads sectors with overlap, compares the results, re-reads on disagreement, and corrects jitter, so scratched or marginal discs still come out as close to bit-perfect as the drive allows.

## AccurateRip

After extraction, and before encoding, every track is checked against the [AccurateRip](http://www.accuraterip.com/) database, which holds checksums of the same pressing ripped by other people. RustyDisc looks the disc up by its table of contents, computes the **v1 and v2** checksums for each track, and reports a per-track confidence (the number of independent rips that agree).

Drives read audio a few dozen to a few hundred samples early or late (the "read offset"), and cdparanoia does not correct for it, so an accurate rip would normally fail to match. RustyDisc therefore also matches tracks at every shift of up to ±2939 samples (v1 entries are searched cheaply on every track; for v2 entries the shift is discovered on a short track, nearest-to-zero first, and then applied to the rest). A match at a shift still proves the audio is identical, and when all tracks agree on the same shift it is reported as a hint of your drive's offset. The saved audio is **not** shifted, so it is exactly what the drive returned.

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

