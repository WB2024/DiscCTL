# Rusty Stick

The full guide to writing music to USB sticks: layouts, tokens, conversion, conflicts, reformatting, and the command line.

[← Back to the README](../README.md)

---


Rusty Stick puts a music collection on a USB stick, **filed the way you want it**, converted first if you like, and only if it will fit. It is in the web UI (sidebar: **Rusty Stick**) and on the command line (`rustydisc stick`).

<p align="center"><img src="../Images/Screenshots/stick-organise.png" alt="Rusty Stick: choose the layout, conflict rules and how to treat music already on the stick" width="820"></p>

1. **Pick the stick.** Every mounted USB stick is listed with its size, free space and filesystem. Only USB and removable devices are offered, so your big data disks can't be chosen by mistake.
2. **Pick the music:** any mix of folders, individual files and `.m3u`/`.m3u8` playlists. The same track chosen twice is written once.
3. **Pick the layout.** Choose a preset or write your own pattern from the tokens below, with a live preview of what your first tracks will look like:

| Preset | Result |
|---|---|
| A–Z › Artist › Album › Disc › Tracks | `B/Beatles/Abbey Road/01 - Come Together.mp3` |
| Artist › Album › Disc › Tracks | `Beatles/Abbey Road/01 - Come Together.mp3` |
| Artist › Album › Tracks (disc in the track number) | `Beatles/White Album/2-05 - Song.mp3` |
| Artist › Year - Album › Tracks | `Beatles/1969 - Abbey Road/01 - Come Together.mp3` |
| Artist - Album › Tracks | `Beatles - Abbey Road/01 - Come Together.mp3` |
| One folder | `Beatles - Come Together.mp3` |

Tokens: `{initial}` `{albumartist}` `{artist}` `{album}` `{year}` `{genre}` `{disc}` `{discfolder}` `{track}` `{dtrack}` `{title}`. `{discfolder}` is "Disc 2" only for albums that really have several discs, so single-disc albums don't get a pointless `Disc 1` folder. `{dtrack}` puts the disc in the track number (`2-05`) for multi-disc albums. Tags are used where present; otherwise names are guessed from the folders (`Artist/Album/CD 2/03 - Song.flac`).

4. **Convert first (optional)**, the same way as for discs: only files that would shrink are converted, tags are kept, and the embedded cover picture can be kept too (car stereos and phones show it).
5. **See the plan.** It says how many files, how big they'll be after conversion, and how full the stick will be. If it doesn't fit, one click applies a size that does (MP3 320 / 256 / 192 / 160 / 128, Opus 96). It counts what files really take on the stick (whole clusters and directory entries) and leaves a safety margin.
6. **Write.** Files are written in sorted order (many car stereos and players play a folder in the order the files were written, not alphabetically), each to a temporary name and renamed when complete, then flushed with `sync`. Afterwards you can eject the stick from the page.

Quality-of-life details: cover art is copied into each album folder (`cover.jpg`); files already on the stick are skipped, so you can top a stick up or resume after an interruption; names are made safe for FAT, exFAT and NTFS automatically (`: * ? " < > |`, trailing dots, reserved names); files over FAT32's 4 GB limit are flagged; "The Beatles" is filed under B (switchable); you can write into a subfolder such as `Music`; a dry run works everything out without writing; and "empty the stick first" needs you to type the folder's name.

<p align="center"><img src="../Images/Screenshots/stick-write.png" alt="A finished write to the stick" width="820"></p>

```bash
# See the plan (JSON): a music folder and a playlist, converted to MP3 256k
rustydisc stick --target /run/media/you/STICK --folder ~/Music --playlist "Road Trip.m3u8" \
  --preset initial-artist-album --transcode mp3:256 --plan

# Write it
rustydisc stick --target /run/media/you/STICK --folder ~/Music --transcode mp3:256 \
  --layout "{albumartist}/{year} - {album}/{discfolder}/{track} - {title}" --subfolder Music
```

| Flag | Description |
|---|---|
| `--target <dir>` | The stick's mount point |
| `--folder`, `--file`, `--playlist` | Sources; repeat any of them |
| `--preset <id>` / `--layout <pattern>` | `initial-artist-album`, `artist-album` (default), `artist-album-flat`, `artist-year-album`, `artist-album-onefolder`, `flat` — or your own pattern |
| `--transcode <spec>` | e.g. `mp3:256`, `aac:256`, `opus:128`, `flac` |
| `--subfolder <name>` | Write into this folder instead of the root |
| `--no-covers`, `--no-keep-art`, `--no-skip-existing` | Turn the extras off |
| `--windows-names auto\|on\|off` | File-name rules (auto looks at the stick's filesystem) |
| `--keep-the` | File "The Beatles" under T |
| `--clear --confirm-clear <name>` | Empty the destination first (the name must match) |
| `--plan`, `--dry-run`, `--force` | Show the plan / write nothing / write even if it won't fit |

## Identify, reformat and tidy what's already on the stick

Selecting a stick **identifies** it: its name, filesystem (with what that means, e.g. FAT32's 4 GB file limit), UUID, partition table, how much music is on it (albums, artists, formats, lossless count) and how it is organised. If the folders follow one of the built-in layouts, RustyDisc says so and can **use that layout** for what you add.

- **Inspect** opens a browser over the stick's contents, from the root down to single tracks, with each track's title, artist, album and quality (format and bitrate).
- **Reformat…** erases the stick and creates exFAT, FAT32, ext4 or NTFS with a name you choose. Choose **the whole stick** to remove every partition (sticks often ship with a small extra one) and make a single new partition filling the space, with an MBR (most compatible) or GPT partition table; or format just one partition. It only works on USB/removable devices that RustyDisc itself lists, and you must type the device name (e.g. `sdb`) to confirm. The stick is mounted again afterwards.
- **Music already on the stick, filed differently:** leave it, or **reorganise** it into the layout you chose. Files are moved (renamed, never copied), album cover pictures go with them, and emptied folders are removed.
- **If a track is already on the stick** (matched by artist, album, disc, track and title, so a different folder or format still counts): skip it, replace it if the new file is **higher quality** (lossless beats lossy; lossy is compared by bitrate, adjusted for codec), replace it if it is **lower quality** (to save space), replace it if it is **newer**, always replace, or keep both. The plan lists every decision and the space that replacing frees.

On the command line: `--on-conflict skip|replace|higher-quality|lower-quality|newer|keep-both` and `--existing leave|reorganize`.

## Making sticks visible to RustyDisc

The stick has to be **mounted** where RustyDisc runs.

- **Desktop / normal install:** nothing to do; sticks are mounted for you (under `/run/media/<you>` or `/media/<you>`) and listed automatically. Unmounted sticks show a **Mount** button (it uses `udisksctl`).
- **Docker on a headless server:** either
  - mount the stick on the host and share a folder with the container: add `- /mnt/usb:/usb:rslave` to the volumes and set `RUSTYDISC_STICK_DIRS: /usb` (sticks mounted inside `/usb` are listed; you can also list folders under **Settings → Rusty Stick**), or
  - let RustyDisc mount sticks itself, hot-plug style, by giving the container `cap_add: [SYS_ADMIN, MKNOD]` and `device_cgroup_rules: ["b 8:* rwm"]`. Unmounted sticks then get a **Mount** button.
  See the commented lines in `docker-compose.yml`. These capabilities are powerful, so use them only on a network you trust, and consider turning on the login (**Settings → Security**).
- **Anywhere else** (a network share, an SD card reader, a folder you just want to fill): list its absolute path under **Settings → Rusty Stick**.

Notes: this has been tested against folders and simulated sticks, not yet against a range of real USB sticks. FAT32 sticks work but are the fussiest (4 GB file limit, slow with many tiny files); exFAT and ext4 are better for big libraries.

