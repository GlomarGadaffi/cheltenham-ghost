# shroud-speak

Encrypted push-to-talk voice over Tor onion services, as a single self-contained binary.

**shroud** is the platform; **speak** is its first capability — voice. shroud-speak is a
ground-up Rust rewrite of **TerminalPhone** (a Bash orchestrator that shelled out to `tor`,
`socat`, `openssl`, `opusenc/opusdec`, `sox`, and ALSA tools). The rewrite collapses that
pipeline into one async process that owns every primitive in memory and **never shells out
and never writes audio to disk**.

The substrate (onion transport + Noise + framing) is deliberately medium-agnostic, so other
capabilities can bolt on later (`shroud-text`, `shroud-drop`, …) over the same spine rather
than forking it. Voice is just the first payload.

## What it is

- A walkie-talkie. Hold a key, talk; release, listen. Two parties, or N via relay.
- Addressed and transported entirely over Tor v3 onion services. No IPs exchanged.
- End-to-end encrypted *above* Tor with a Noise handshake keyed by a shared secret,
  so the circuit crypto is not the only thing standing between you and a listener.

## Why rewrite it

| Bash / TerminalPhone | shroud-speak |
| --- | --- |
| ~10 external binaries, FIFOs, fd juggling, `socat` | one static binary, in-process |
| audio chunks hit disk as `.tmp`/`.opus`/`.enc` | RAM-only ring buffers, zeroized keys |
| AES-256-CBC + ad-hoc HMAC + PBKDF2-per-chunk | Noise transport (AEAD, forward secrecy) |
| `AUDIO:<base64>\n` text framing (+33%, leaks length) | length-prefixed binary frames, paddable |
| external `tor` daemon + `torrc` + hostname file on disk | embedded [arti], optional ephemeral in-memory onion |
| `install_deps` + per-package-manager branches | nothing to install |

See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for the full mapping and rationale.

## Status

**Pre-alpha.** The M0 de-risking spike — hosting and self-dialing an onion service
in-process with no external `tor` — is implemented in [`crates/m0-spike`](crates/m0-spike)
and **compiles against arti 0.42** (`cargo run -p m0-spike`). It has **not** yet been run
end-to-end against the live Tor network, so the premise isn't fully proven. See
[`docs/ROADMAP.md`](docs/ROADMAP.md).

## Layout

```
shroud-speak/                 (repo)
  crates/
    m0-spike/       throwaway M0 spike: in-process onion + self-dial (compiles vs arti 0.42)
    shroud-core/    [M2/M3] substrate: arti onion transport + Noise + framing (medium-agnostic)
    shroud-proto/   [M2] generic frame envelope, no I/O — unit-testable in isolation
    shroud-speak/   [M3] the voice app: audio pipeline + voice frame types + front-end
  docs/
```

`shroud-core` is a library from day one, so the voice app — and anything bolted on later —
is a thin shell over the same engine. If a second capability ever appears, core/proto can be
promoted to their own repo or published as crates with no rework.

## Platforms (target)

Linux (musl static), macOS, Windows, Android/Termux. One codebase, `cargo build`.
Audio capture/playback abstracted via `cpal` (ALSA / CoreAudio / WASAPI / Oboe).

## Threat model

This is a security tool; read [`docs/THREAT_MODEL.md`](docs/THREAT_MODEL.md) before trusting
it with anything. Short version: it targets network-adversary confidentiality and metadata
resistance, **not** endpoint compromise. A rooted phone or a keylogger defeats it.

## Open Decisions

Resolved:
- [x] **Name** — `shroud-speak` (platform `shroud` + capability `speak`).
- [x] **`shroud-core` as a library from day one** — yes; everything else bolts onto it.

Still open:
- [ ] **Tor layer:** [arti] (recommended) vs. linking C-tor. Gated on confirming arti's
      onion-service vanguards / DoS hardening is compiled in and on (verified at M0).
- [ ] **Front-end shape:** TUI binary inside `shroud-speak` (M3 default) vs. headless daemon
      + thin clients (lets hardware be a first-class client; deferred, M5).
- [ ] **Repo visibility:** private through M0–M2, flip public at `v0.1.0`? Or public now?

## License

MIT — see [`LICENSE`](LICENSE). Inherited from TerminalPhone.

[arti]: https://gitlab.torproject.org/tpo/core/arti
