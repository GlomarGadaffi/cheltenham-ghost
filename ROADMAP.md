# Roadmap

Milestones are ordered by **risk retired**, not by feature appeal. Each one ends in
something runnable. Nothing past M0 is worth building until M0 proves the premise.

| Milestone | Status |
| --- | --- |
| **M0** — in-process onion transport | ✅ **Done** — released `v0.0.1-alpha`, verified on live Tor |
| M1 — audio loopback | Not started |
| M2 — secure transport | Not started (`shroud-proto` codec landed early) |
| M3 — vertical slice / first call (`v0.1.0`) | Not started |
| M4 — hardening & parity | Not started |
| M5 — platform reach | Not started |

## M0 — Premise spike: in-process onion, no external tor  *(highest risk)* — ✅ DONE

Prove the whole architecture is possible. Verified on the live Tor network; see the
runnable example at `crates/shroud-core/examples/m0_spike.rs`
(`cargo run -p shroud-core --example m0_spike`) and the runtime write-up in `REVIEW.md`.

- [x] `arti-client`: bootstrap a `TorClient`.
- [x] `launch_onion_service` → obtain a `.onion` address at runtime.
- [x] From the *same* client, dial that `.onion` and accept the inbound stream.
- [x] Round-trip arbitrary bytes both directions over the stream.
- [x] vanguards / DoS-hardening feature compiled in (on by default for HS circuits).
      *Caveat:* arti 0.23 exposes no runtime accessor to **assert** active vanguards
      (issue #12); a stronger check would read circuit-construction logs.
- [x] **Ephemeral onion (S8):** fresh `.onion` per session via a temp-dir state store
      wiped on exit. True in-memory keys await [arti#1186].
- [x] **Sustained + reconnect (A4):** long-lived stream with periodic keepalives
      (`SHROUD_M0_SECS`, default 60s) + a fresh re-dial. A *forced circuit rebuild*
      test (stronger than re-dial) remains to add.

**Exit criterion (met):** bytes echo through a self-hosted onion with zero external
processes.

**M0 learnings (see `REVIEW.md`):** Tor streams have **no half-close** — an `END` cell
closes both directions, and dropping an accepted stream sends `END/MISC`
(`CloseStreamBehavior::default`), so close cleanly with read→write→**flush**→drop.
Windows/static builds need `libsqlite3-sys` `bundled` (arti's transitive `rusqlite`).

[arti#1186]: https://gitlab.torproject.org/tpo/core/arti/-/issues/1186

## M1 — Audio loopback, in memory

Prove the real-time path without the network.

- [ ] `cpal` capture → ring → `audiopus` encode → decode → ring → `cpal` playback.
- [ ] Verify on at least two backends (ALSA + one of CoreAudio/WASAPI).
- [ ] Measure end-to-end local latency; tune frame size and ring depth.
- [ ] PTT gating via `crossterm` key events (hold-to-talk, release-to-listen).

**Exit criterion:** hold a key, hear your own voice with acceptable latency; no temp files.

## M2 — Secure transport

Prove the crypto layer in isolation.

- [ ] `shroud-proto`: frame types + length-prefixed (de)serialization, fully unit-tested.
- [ ] `snow` Noise handshake over a plain TCP socket (PSK pattern chosen + documented).
- [ ] AEAD transport carrying framed messages; replay handling validated.
- [ ] `argon2` secret-at-rest; `zeroize` + page-locking for key material.
- [ ] Decision: arti restricted-discovery on top, or Noise PSK alone.

**Exit criterion:** two local processes complete a handshake and exchange authenticated,
encrypted, framed messages; tampering and replay are rejected.

## M3 — Vertical slice: a real 1:1 call

Compose M0+M1+M2 into the actual product.

- [ ] `shroud-core` provides the tor stream + Noise + session plumbing (medium-agnostic).
- [ ] `shroud-speak` builds the voice app on it: audio pipeline + voice frames + front-end.
- [ ] Front-end (in `shroud-speak`): listen / call / settings / status, PTT in-call.
- [ ] Onion address display + `qrcode` terminal QR.
- [ ] Clean teardown: zeroize, drop streams, no residue.

**Exit criterion:** two machines on different networks hold a push-to-talk call over Tor.
This is the first taggable release (`v0.1.0`) and the candidate moment to go public.

## M4 — Hardening & parity

Reach feature parity with TerminalPhone where it still makes sense.

- [ ] Voice effects as DSP nodes (`fundsp`).
- [ ] Traffic-analysis resistance: fixed-size padded frames, optional cover traffic.
- [ ] Ephemeral onion mode (fresh in-memory key per session, never persisted).
- [ ] Snowflake / bridge support via arti config (if censorship-circumvention is in scope).
- [ ] Relay mode (N-caller bridge) — port the topology, not the FIFO mechanics.

## M5 — Platform reach

- [ ] musl static Linux build; macOS + Windows binaries; CI matrix.
- [ ] Android/Termux: `cargo-ndk`, Oboe backend via `cpal`, mic-permission story.
- [ ] Reproducible builds + release signing (this matters for a security tool).
- [ ] Decision revisited: ship daemon + thin clients to enable hardware front-ends.

## Out of scope (for now)

- Federation / directory of users — onion addresses are exchanged out of band, by design.
- Group video, file transfer, text chat as a primary feature.
- Mobile GUI app — only if M5 Android demand justifies it.

## De-risking note

The ordering is deliberate: M0 is the only milestone that can kill the project, so it
comes first and cheap. M1 and M2 are independent and could be done in parallel by one
person context-switching, but both must land before M3 means anything.
