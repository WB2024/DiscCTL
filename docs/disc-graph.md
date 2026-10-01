# Disc graph & errors

The JSON format that describes a disc, and how errors are reported.

[← Back to the README](../README.md)

---

# Disc Graph JSON

All disc definitions share a common JSON schema. This is the intermediate representation that every input path converges to.

```json
{
  "format": "bluebook",
  "label": "My Album",
  "sessions": [
    {
      "type": "audio",
      "tracks": [
        "/music/track01.wav",
        "/music/track02.wav"
      ],
      "cd_text": {
        "title": "My Album",
        "artist": "Artist Name"
      },
      "track_titles": [
        { "title": "Track One" },
        { "title": "Track Two" }
      ]
    },
    {
      "type": "data",
      "source_dir": "/music/extras",
      "filesystem": "iso9660",
      "joliet": true,
      "rock_ridge": true
    }
  ]
}
```

**Burn from a JSON graph:**
```bash
rustydisc burn --input disc.json --device /dev/sr0
rustydisc plan --input disc.json
rustydisc validate disc.json
```

A ripped archive's `metadata/disc.json` is a valid DiscInfo export — future versions will support `rustydisc burn` directly from this file to reconstruct the original disc.

---

# Error Handling

All errors are emitted to stderr as structured JSON:

```json
{
  "error": "SESSION_ORDER_INVALID",
  "message": "Data session cannot precede audio session in BlueBook format",
  "recoverable": false
}
```

```json
{
  "error": "BACKEND_ERROR",
  "message": "cdparanoia is not installed. Run: sudo apt install cdparanoia",
  "recoverable": false
}
```

```json
{
  "error": "DISC_ALREADY_FINALIZED",
  "message": "Disc on /dev/sr0 is already finalized. Insert a blank disc or use `rustydisc recover --blank fast` for CD-RW.",
  "recoverable": true
}
```

`recoverable: true` means you can fix the issue and retry the same command. `recoverable: false` means there is a problem with your input that must be corrected.

Exit codes:
- `0` — success
- `1` — error (details on stderr as JSON)

## Write speed

An optional top-level `"speed"` sets the write speed as an "x" multiple (`8` means 8x), from 1 to 100. Leave it out and the drive chooses. A `--speed` given to `burn` or `plan` overrides it.

```json
{ "format": "redbook", "label": "Mix", "speed": 8, "sessions": [ { "type": "audio", "tracks": ["01.flac", "02.flac"] } ] }
```
