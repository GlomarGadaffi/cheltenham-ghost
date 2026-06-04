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

See [`ARCHITECTURE.md`](ARCHITECTURE.md) for the full mapping and rationale.

## Status

**Pre-alpha. M0 complete — transport only; no audio (M1) or crypto (M2) yet.**
Released as [`v0.0.1-alpha`](https://github.com/GlomarGadaffi/shroud-speak/releases/tag/v0.0.1-alpha)
(prerelease, M0 milestone). The core premise is proven on the **live Tor network**: a
`TorClient` bootstraps, hosts an onion service in-process, self-dials its own `.onion`, and
round-trips bytes both directions with **zero external `tor` process** — verified with an
ephemeral onion plus sustained traffic and a reconnect. Run it yourself:

```bash
cargo run -p shroud-core --example m0_spike      # SHROUD_M0_SECS tunes the sustained phase
```

`v0.1.0` remains reserved for the M3 voice call. See [`ROADMAP.md`](ROADMAP.md) for milestone
status and [`REVIEW.md`](REVIEW.md) for the design/code review and M0 runtime findings.

## Layout

```
shroud-speak/                 (repo)
  crates/
    shroud-core/    substrate: arti onion transport + Noise + generic framing (medium-agnostic)
    shroud-proto/   generic frame envelope, no I/O — unit-testable in isolation
    shroud-speak/   the voice app: audio pipeline + voice frame types + front-end
  docs/
```

`shroud-core` is a library from day one, so the voice app — and anything bolted on later —
is a thin shell over the same engine. If a second capability ever appears, core/proto can be
promoted to their own repo or published as crates with no rework.

## Platforms (target)

Linux (musl static), macOS, Windows, Android/Termux. One codebase, `cargo build`.
Audio capture/playback abstracted via `cpal` (ALSA / CoreAudio / WASAPI / Oboe).

## Threat model

This is a security tool; read [`THREAT_MODEL.md`](THREAT_MODEL.md) before trusting
it with anything. Short version: it targets network-adversary confidentiality and metadata
resistance, **not** endpoint compromise. A rooted phone or a keylogger defeats it.

## Open Decisions

Resolved:
- [x] **Name** — `shroud-speak` (platform `shroud` + capability `speak`).
- [x] **`shroud-core` as a library from day one** — yes; everything else bolts onto it.
- [x] **Tor layer:** [arti] — M0 proved in-process onion host + self-dial works (vanguards
      feature compiled in, on by default for HS circuits). C-tor fallback not needed. Caveat:
      arti 0.23 has no *true* in-memory ephemeral onion key ([arti#1186]); the M0 spike
      approximates it with a temp-dir state store wiped on exit.

Still open:
- [ ] **Front-end shape:** TUI binary inside `shroud-speak` (M3 default) vs. headless daemon
      + thin clients (lets hardware be a first-class client; deferred, M5).
- [ ] **Repo visibility:** private through M0–M2, flip public at `v0.1.0`? Or public now?
- [ ] **Asserting active vanguards at runtime** — arti 0.23 exposes no accessor; a stronger
      M0 check would read circuit-construction logs (issue #12).

## License

MIT — see [`LICENSE`](LICENSE). Inherited from TerminalPhone.

[arti]: https://gitlab.torproject.org/tpo/core/arti
[arti#1186]: https://gitlab.torproject.org/tpo/core/arti/-/issues/1186
