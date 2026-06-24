# tor-voice

in-process Tor (arti) + Opus codec + Noise protocol = encrypted voice-over-Tor endpoint. no daemons, no config files, no socket hijacking. real async I/O (tokio) owns all primitives. RX/TX audio via CPAL (cross-platform), encrypt with Noise, transit over arti's onion service.

## features

- **arti-native onion service** — no `tor` daemon, no torrc, no hostname file. `TorClient::launch_onion_service` hosts the keypair in-memory.
- **audio codec** — Opus (libopus), 8 kHz mono, 16 kbps, 60 ms frames
- **transport security** — Noise protocol (snow crate) with PSK; replaces TerminalPhone's hand-rolled cipher
- **key derivation** — Argon2 (memory-hard KDF), not PBKDF2
- **key hygiene** — secrets zeroed on drop, pages optionally mlock'd
- **cross-platform audio** — CPAL abstracts across Linux ALSA, macOS CoreAudio, Windows WASAPI, Termux
- **optional DSP** — pitch shift, overdrive, flanger, echo, highpass, tremolo (hand-rolled or fundsp)
- **QR code** — terminal QR of onion address for peer scanning
- **session FSM** — PTT state, PING/HANGUP, peer authentication all in tokio channels

## architecture

single tokio process; no subprocesses, FIFOs, or temp files.

```
┌─────────────────────────────────────┐
│  CPAL capture → ring buf → Opus enc │
│         ↓                            │
│   Noise/AEAD encrypt → arti stream  │
│         ↑                            │
│  CPAL playback ← ring buf ← Opus dec│
│                                     │
│  PTT FSM, control verbs (mpsc)      │
└─────────────────────────────────────┘
```

**TX task:** 60 ms frames, optional voice DSP, Opus encode, Noise encrypt, arti write.
**RX task:** arti read, Noise decrypt, Opus decode, ring buf → playback callback.
**Control task:** PTT, PING, HANGUP over mpsc channels.

latency: Tor adds 100–500 ms; keep local buffers minimal (1–2 frames).

## usage

```bash
cargo build --release
./target/release/tor-voice
# → prints onion address, QR code
# → listens on the onion service

# From a peer:
./target/release/tor-voice --peer <onion-address>
# → connects, runs PTT terminal session
```

## threat model & secrets

- **Tor authentication:** onion address IS the server public key (v3 style)
- **Noise PSK:** shared secret for mutual authentication + forward secrecy
- **Noise pattern:** NNpsk0 or XKpsk2 (TBD; see THREAT_MODEL.md)
- **key zeroization:** `zeroize` + `mlock` on all secrets
- **replay protection:** Noise transport nonce sequencing (no nonce-log file)

see ARCHITECTURE.md, THREAT_MODEL.md, and PROTOCOL.md for details.

## notes

replacement for TerminalPhone (Bash prototype). Rust + tokio + arti is production-grade; Bash version relied on socat, openssl, opusenc CLI, and file-descriptor juggling.
