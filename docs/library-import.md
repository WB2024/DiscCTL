# Importing into your music library

[← Back to the README](../README.md)

Rips land in the rips folder. **Import** moves finished albums into your real music library, filed by a **MusicBrainz Picard naming script**, so the library keeps the exact layout you already have.

## Set it up

In **Settings → Music library**:

1. **Library folder**: an absolute path RustyDisc can write to (in Docker, mount it read-write; see below). `RUSTYDISC_LIBRARY_DIR` sets the default.
2. **Naming script**: paste your Picard file naming script. It runs on every file's tags and returns the path (folders and file name, without the extension). A live preview shows the path it makes for an example track (or a real one from the Import page). Leave the built-in script if you like it.
3. **Defaults** for how files are imported.

Then open **Import**, tick the rips, check the plan, and import.

## The naming script

RustyDisc runs the script itself; it isn't approximated by a template. It understands `%variables%`, `$functions()` (with lazy `$if` / `$if2`), `\` escapes, and Picard's rule that line breaks and the indentation after them are ignored. Functions: `noop set get unset if if2 and or not eq ne gt gte lt lte add sub mul div mod len left right substr num upper lower title trim replace rreplace firstalphachar truncate in startswith endswith swapprefix`. A function it doesn't know is reported as an error, never silently wrong.

Variables come from the file's tags under Picard's names: `title artist artists albumartist album artistsort albumartistsort tracknumber totaltracks discnumber totaldiscs discsubtitle date originaldate originalyear genre label catalognumber barcode isrc composer lyricist producer work language script media releasestatus releasetype releasecountry asin compilation musicbrainz_albumid musicbrainz_albumartistid musicbrainz_artistid musicbrainz_trackid musicbrainz_releasetrackid musicbrainz_releasegroupid`, plus `_releasecomment`, `_extension` and `_length`. A `/` inside a tag value becomes `_`, so a tag can never add a folder to the path. The result is split into folders, cleaned of stray spaces, control characters and `.` / `..`, and the file extension is added.

## What gets imported

| Option | Default |
|---|---|
| **How:** move, copy, or hard link | move |
| **Cover picture** (`cover.jpg`) | yes |
| **Every other file** (logs, cue sheets, booklets) | no |
| **Delete what's left** in the rip folder, and the folder | no |
| **If a file is already in the library:** skip, replace, replace if higher quality / lower quality / newer, keep both | skip |

Moving across disks copies first and only then removes the original, and every file is written under a temporary name and renamed, so an interrupted import never leaves a half-written track in your library. Skipped tracks are never deleted from the rip folder. A finished import leaves an `imported.json` in the rip folder, so the list shows what's done.

## Tags written when ripping

Import relies on the tags in the files, so ripping now writes everything MusicBrainz knows, not just title and artist. One extra lookup per rip fetches the full release, and each file gets, in its format's native fields (UFID for the recording ID in MP3, Vorbis comments in FLAC and Ogg, freeform atoms in M4A):

- **IDs:** recording, release track, release, release group, track and album artists, work
- **Names:** artist, album artist, all credited artists, and sort names
- **Release:** date, original date, disc number and total, disc subtitle, label, catalogue number, barcode, ASIN, status, type, country, script, language, media format, genres, and the release comment
- **Track:** ISRCs, work, composers, lyricists, writers, arrangers, conductors, producers, mixers, engineers, remixers and performers (with instrument)

If MusicBrainz can't be reached the rip carries on with the basic tags. Rips made before this version have only the basic tags; the naming script's fallbacks apply for what's missing.

## Importing through Lidarr

If Lidarr manages your library, choose **Import with → Lidarr** (or make it the default in Settings). Since every rip carries its MusicBrainz release and release group IDs, Lidarr is told exactly which album it is; nothing is guessed. For each rip RustyDisc:

1. Finds the album in Lidarr by its release group (asking MusicBrainz first if the rip only knows its release). Older rips without IDs are matched by name, and only when the match is exact.
2. Adds the artist and album to Lidarr if they aren't there, **unmonitored**, so nothing is searched for or downloaded.
3. Asks Lidarr how it matches each file to its tracks, and shows you that before you commit.
4. Has Lidarr import the matched files, by moving or copying, with Lidarr's own naming, quality and rules. Unmatched files stay in the rip folder.
5. Optionally deletes what is left in the rip folder, but only when every file was imported.

Wrong album? Paste its MusicBrainz release or release group link under the plan and re-check.

Set it up in **Settings → Lidarr**: the address, the API key (Lidarr → Settings → General), which root folder, quality profile and metadata profile new artists get (**Test connection** fills these lists in), and a **path mapping** if Lidarr sees the rips folder under a different path than RustyDisc does. If both containers mount the disk at the same place there is nothing to map. The key is stored on the server and never sent back to the browser.

On the command line: `rustydisc import-lidarr --rip <dir> --url http://lidarr:8686` (key in `RUSTYDISC_LIDARR_KEY`), with `--plan`, `--dry-run`, `--mode move|copy`, `--match FOLDER=MBID` and `--delete-leftovers`.

## Docker

The library folder must be writable inside the container. Simplest is to give RustyDisc the same view of your disk that your other tools have:

```yaml
    volumes:
      - /mnt/Main20TB:/data                 # rips and library on one filesystem, so moves are instant
    environment:
      RUSTYDISC_RIPS_DIR: /data/CD Rips
      RUSTYDISC_LIBRARY_DIR: /data/Media/Audio/Music
```

Keeping the rips and the library on one filesystem means a "move" is a rename: instant, with no extra space.

## Command line

```bash
rustydisc import --rip "/rips/Artist - Album (2001)" --library /music
rustydisc import --rip ~/rips/A --rip ~/rips/B --library /music --script-file my-picard-script.txt \
  --mode copy --include-other --on-conflict higher-quality --plan
```

`--mode move|copy|hardlink`, `--no-cover`, `--include-other`, `--delete-leftovers`, `--on-conflict skip|replace|higher-quality|lower-quality|newer|keep-both`, `--plan` (print the plan as JSON), `--dry-run`.

## Covers and tags in the Library

Open a rip in the **Library** to fix it up after the fact:

- **Add / change cover art**: choose a JPEG or PNG in your browser (up to 25 MB). Choose whether to **embed** it in every audio file (replacing any picture already inside) and whether to **save** it as `cover.jpg` / `cover.png` in the album folder (replacing the current one). On the **Rip** page you can do the same before ripping: **Upload cover art** is used instead of looking one up, and honours the same save/embed settings.
- **Edit album tags** writes the fields you change to every track; fields you leave alone stay as they are, and clearing a field removes it.
- **Tags** on each track shows and edits that file's tags: title, artist, track and disc numbers, dates, genre, label, sort names, credits and all the MusicBrainz IDs, plus a list of every tag in the file (edit, remove, or add your own).

Editing changes tags only: file names, and the saved `metadata/musicbrainz.json` of an archive rip, stay as they were. Pictures can't be embedded in WAV files.
