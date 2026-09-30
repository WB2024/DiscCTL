# Web UI & Docker

Running RustyDisc as a web app on a headless server, in Docker, on Proxmox, with an optional login.

[← Back to the README](../README.md)

---

`rustydisc serve` runs a web interface for everything the CLI does — scan and rip discs, burn Audio / Data / Enhanced CDs, browse and play your rips, verify archives, recover or blank discs — so a headless machine with the drive attached can be driven from any browser on the network.

```bash
rustydisc serve --rips-dir /srv/Music/CDRips --media-dir /srv/burn-sources
# → http://<host>:8080
```

Want to look around first? `rustydisc serve --mock` simulates a drive, with no hardware needed.

The **Tools** page covers disc recovery and CD-RW blanking, and checks that the external programs RustyDisc relies on are installed.

| Flag | Env var | Default | |
|---|---|---|---|
| `--bind` | `RUSTYDISC_BIND` | `0.0.0.0:8080` | listen address |
| `--device` | `RUSTYDISC_DEVICE` | `/dev/sr0` | default drive |
| `--rips-dir` | `RUSTYDISC_RIPS_DIR` | `./rips` | rip output + library |
| `--media-dir` | `RUSTYDISC_MEDIA_DIR` | `./media` | burn sources |
| `--config-dir` | `RUSTYDISC_CONFIG_DIR` | `./config` | where settings are stored |
| `--mock` | `RUSTYDISC_MOCK` | off | simulate a drive (no hardware needed) |

Only one job may use the drive at a time; long jobs stream live progress to every open browser. The login is optional; see [Security](#security-optional-login) below.

## Docker

```bash
docker compose up -d --build     # http://<host>:8080
```

The image bundles everything RustyDisc needs (`cdparanoia`, `cdrdao`, `xorriso`, `wodim`, `ffmpeg`, `eject`). `docker-compose.yml` passes `/dev/sr0` (and `/dev/sg0`, needed for burning) into the container, adds the `SYS_RAWIO` capability, and mounts `./rips` (your output), `./media` (burn sources, read-only) and `./config` (your settings, including the fanart.tv key). Edit the device names and volume paths to match your machine.

**Prebuilt image:** `wb20244/rustydisc` on Docker Hub. [`compose.dockge.yaml`](../compose.dockge.yaml) is a ready-to-paste stack for Dockge (or any Compose host) that uses it instead of building.

**On Proxmox:** Docker usually runs inside a VM or LXC, so pass the drive into that guest first. For a VM, use SATA or USB passthrough; for an LXC, allow and bind the `/dev/sr0` and `/dev/sg*` device nodes.

## Security (optional login)

 by default the UI has no login, so keep it on a trusted network. To require one, either open **Settings → Security** and choose a user name and password (at least 8 characters), or set `RUSTYDISC_AUTH_USER` and `RUSTYDISC_AUTH_PASSWORD` (`--auth-user` / `--auth-password`) which take priority and can't be changed from the UI. With a login on, every page, API call, file download and live-progress stream needs a session cookie; passwords are stored only as Argon2 hashes in `/config/settings.json` (mode 600), sessions last a week and end on log-out or a password change, and 8 wrong attempts from one address lock it out for 5 minutes. It is plain HTTP, so on an untrusted network put a TLS reverse proxy in front.

## Converted files

When Rusty Stick or a Data disc converts audio to fit (say FLAC to MP3), the converted files can be thrown away or kept for next time. Choose in **Settings → Converted files**:

- **Delete right after use** (default): nothing is kept.
- **Keep for N days** after they were last used: any job that needs the same conversion of the same file reuses it instantly.
- **Keep until I clear them.**

A **size limit** (default 20 GB, 0 for none) removes the least recently used files first, and **Clear now** empties the cache. Files are matched on the source file's path, size and modification time plus the conversion, so an edited file or a different bitrate is never served a stale copy. Files used in the last 15 minutes are never removed, so a running job is safe.

They live in `cache` next to the settings by default; set another folder in Settings, or with `RUSTYDISC_CACHE_DIR` (in Docker, mount a volume there if you convert a lot). Temporary files while a job runs go to `/tmp`, or use `--stage-dir` on the command line.

From the command line: `--convert-cache <dir>` with `--cache-days <n>` and `--cache-max-gb <n>` on `rustydisc stick` and `rustydisc burn`.
