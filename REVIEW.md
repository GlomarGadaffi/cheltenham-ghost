# shroud-speak — Design & Code Review

A multi-disciplinary review of the pre-alpha design and the M0 scaffold, produced by
three specialist passes (security/crypto, code, architecture). Everything here is advisory;
the project is at the design stage and the review is calibrated to a solo-ish pre-alpha, not
an enterprise process. File:line references point at the state of the repo at review time.

> **One-line verdict:** unusually disciplined for a pre-alpha — honest threat model,
> risk-ordered roadmap, clean crate intent. The highest-leverage move is not code: it's
> nailing the **core ↔ capability seam** (keep voice/PTT *out* of `shroud-core`) and
> closing a handful of **decide-before-M2** protocol questions. The lowest-risk code win
> (the `shroud-proto` envelope) has been implemented as part of this review.

---

## Top items, ranked by leverage

1. **Pin down the `core ↔ speak` seam now (no code required).** Docs currently put the
   PTT/session FSM *inside* `shroud-core` (`ARCHITECTURE.md:56`). That makes the
   "medium-agnostic substrate" secretly voice-shaped, so a future `shroud-text` forks
   anyway — the exact outcome the architecture exists to prevent. Decide: **core owns**
   connection lifecycle + Noise transport + authenticated frame I/O + secret memory;
   **speak owns** PTT, the half-duplex FSM, opus, cpal, and the concrete frame-type table.
   This single decision also makes the "library from day one" deferral genuinely free.

2. **Noise pattern vs PSK reality (decide before M2).** `NNpsk0` (`PROTOCOL.md:23,29`) makes
   a possibly-passphrase-derived PSK the *sole* application-layer authenticator, with an
   online guessing oracle and KCI exposure. Prefer **`XKpsk2`** (static-key auth survives a
   weak PSK), or write the precise justification for staying on `NNpsk0`. Note the initiator
   already knows the responder's `.onion` (its HS ed25519 identity) — but do **not** hand-roll
   a cross-use of that key as a Noise X25519 static; keep them separate keys.

3. **Fully specify & freeze PSK derivation (decide before M2).** Two sharp edges:
   - The **raw-key bypass** (`PROTOCOL.md:29`) must be an *explicit, versioned discriminator*,
     never inferred from the input's shape (e.g. "looks like hex ⇒ raw key"). In-band type
     confusion silently downgrades a passphrase to zero stretching. Fail closed on mismatch.
   - **Argon2id salt has no home here**: there's no server and both peers must derive the
     *identical* PSK with no prior channel. A random per-user salt is impossible; an in-band
     salt is attacker-malleable. Pick a **deterministic salt** (e.g. domain-separated
     protocol constant + an out-of-band session label) and freeze the **cost parameters as
     protocol constants** (both ends must agree). Benchmark on the weakest target (Termux).

4. **Decide the 2-party-vs-relay scope of the frozen wire format (decide before M2).** Noise
   is a 2-party protocol; its nonce sequencing gives replay/ordering guarantees only between
   the two endpoints of one session. The `Relay`/`Group` frames (`PROTOCOL.md:63-64`) break
   that: a relay can drop/reorder/replay, and with only a shared PSK there is **no speaker
   attribution** — any group member or the relay can inject audio as anyone. There is also
   **no relay trust model in `THREAT_MODEL.md` at all.** Either freeze **2-party-only** and
   mark `Group`/`Relay` reserved, or design the group trust model before freezing those types.

5. **Bake traffic-analysis mitigations into the frame format even though they ship at M4.**
   Padding/cover traffic are deferred and off by default (`PROTOCOL.md:74-85`), but frame
   **length** leaks speech content (a known voice-over-encryption attack), and the defaults
   leak call start/stop, duration, and turn-taking to an observer of *either* endpoint.
   Reserve a padding bucket scheme (or default to **CBR Opus**) and put a **version byte in
   `Hello`** now so M4 isn't a wire-format break. Then reconcile the threat model, which
   currently lists metadata as a *defended* asset (`THREAT_MODEL.md:9`) while defaults leak it.

---

## Security / cryptographic findings

| # | Sev | Finding | Where |
|---|-----|---------|-------|
| S1 | High | `NNpsk0` makes a weak/passphrase PSK the only app-layer authenticator (online oracle, KCI). Prefer `XKpsk2`. | `PROTOCOL.md:23,29` |
| S2 | High | Raw-key bypass is an auth-downgrade footgun if mode is inferred, not explicit/versioned. | `PROTOCOL.md:29` |
| S3 | High | Argon2id salt + cost params unspecified and constrained (no server, no pre-channel). Must be deterministic + frozen. | `PROTOCOL.md:29` |
| S4 | High | Reconnection resets the Noise nonce; specify fresh ephemerals per handshake, fatal/non-resumable decrypt failure, full re-handshake on reconnect, bounded reconnect-loop DoS. | `PROTOCOL.md:69-72` |
| S5 | High | N-party relay breaks the 2-party nonce assumption; no speaker attribution; no relay trust model. | `PROTOCOL.md:63-64,90`; `THREAT_MODEL.md` |
| S6 | Med | "Encrypted twice over" is strength-language; reframe as "two layers, different properties" and name what Noise adds that Tor doesn't (FS, mutual auth, HS-impersonation resistance). | `README.md:20-21`; `THREAT_MODEL.md:15` |
| S7 | Med | Metadata listed as a protected asset but defaults (no padding/cover) leak timing/length; tighten the claim. | `THREAT_MODEL.md:9` |
| S8 | Med | Onion key at rest: spec says the *shared secret* is encrypted at rest but is silent on the *onion key* (impersonation asset). Specify encryption or make ephemeral-onion the recommended default. | `THREAT_MODEL.md:11,21-24` |
| S9 | Med | M0 prints the vanguards state but never *asserts* it; an M0 "pass" can have hardening silently off. | `m0_spike.rs:26-28` |
| S10 | Note | Realistic adversary is "observes both endpoints," cheaper than a full GPA; state it. | `THREAT_MODEL.md:33-35` |
| S11 | Note | Secret bytes can be copied by `String`/`Vec` realloc before zeroize; use `Zeroizing`/`secrecy` from the moment a passphrase exists. | `THREAT_MODEL.md:41-50` |
| S12 | Low | No pre-M5 build integrity story; add "build from source; binaries unverifiable before M5." | `THREAT_MODEL.md:56-57` |
| S13 | Low | Make "no application frame before handshake completion" a normative MUST; put the version byte in `Hello`. | `PROTOCOL.md:55,66,91` |

**Top 5 to resolve before freezing the protocol at M2:** S1, S2+S3, S5, S7+S13 (TA + versioning), S4+S8.

---

## Code findings (M0 scaffold)

| # | Sev | Finding | Where |
|---|-----|---------|-------|
| C1 | High | A single `read()` is treated as a whole message and `Ok(0)`/EOF is unhandled. **⚠️ Correction (proven at runtime): the suggested "half-close + `read_to_end`" is WRONG for Tor** — Tor streams have no half-close (an `END` cell tears down both directions) and dropping a stream sends `END/MISC`, which `read_to_end` surfaces as an error. Correct fix applied: flush the echo, then a single `read` on the client (the Data cell arrives ahead of the END). Real framing belongs in `shroud-proto`. See **Runtime validation** below. | `m0_spike.rs:60,110` |
| C2 | High | Echo side never `flush()`es (relies on drop-flush). **This was the actual root cause** of the live `END/MISC` failure: without flush the buffered echo is discarded before the stream drops. **(Fixed + verified on live Tor.)** | `m0_spike.rs:67-69` |
| C3 | High | arti deps pinned to `0.23.0` (comment says "unpinned"). **Correction: `0.23.0` does exist and resolves** (0.43.0 is current); cargo built all `tor-*` + `arti-client` at 0.23.0. `Cargo.lock` is now generated. Still: reconcile the "unpinned" comment and commit the lockfile (done in this branch). | `Cargo.toml:21,25-29` |
| C4 | Med | `shroud-proto` declared `anyhow` but didn't use it; a leaf codec wants a typed error, not `anyhow`. **(Fixed in this review.)** | `shroud-proto/Cargo.toml:9` |
| C5 | Med | `clippy::new_without_default` on `ShroudClient::new`. **(Fixed in this review.)** | `shroud-core/src/lib.rs:7-11` |
| C6 | Med | No `[lints]` table / `#![forbid(unsafe_code)]`; cheap to set now, and `mlock` later is the one place unsafe legitimately appears — make it a conscious exception. | workspace |
| C7 | Med | `Frame` had no (de)serialization despite being the crate's whole purpose; decode must validate `len` before allocating and treat the type byte generically. **(Implemented in this review.)** | `shroud-proto/src/lib.rs` |
| C8 | Med | `experimental-api` is semver-exempt and load-bearing (`onion_name`, `handle_rend_requests`, `Connected::new_empty`, `vanguards()`); incompatible with loose version ranges and with shipping a stable v0.1.0. | `Cargo.toml:25` |
| C9 | Low | Retry loop sleeps on non-retryable errors; `tracing_subscriber` is init'd then unused (all output is `println!`); add per-attempt `timeout`. | `m0_spike.rs:11,85-100` |
| C10 | Low | `expect("Stream should be populated")` is structurally-unreachable dead code; restructure with labeled-loop `break value` or a helper returning `Result<DataStream>`. | `m0_spike.rs:102` |
| C11 | Nit | `tracing-subscriber` is a dep of the *library* `shroud-core`; subscriber init belongs to the binary. Library should depend on `tracing` only. | `shroud-core/Cargo.toml` |
| C12 | Nit | Consider edition 2024; `authors` missing email; `tor-cell`/`tor-proto` may belong in `[dev-dependencies]` (only the example uses them). | `Cargo.toml` |

---

## Architecture findings

| # | Sev | Finding | Where |
|---|-----|---------|-------|
| A1 | High | Core ↔ capability seam: move PTT/FSM/real-time ring policy *out* of core (see Top item #1). The "generic envelope" (`ARCHITECTURE.md:114`) vs "voice verb table" (`PROTOCOL.md:53`) is an internal contradiction — proto owns the envelope, speak owns the type constants. | `ARCHITECTURE.md:56,114` |
| A2 | High | Backpressure across the Tor seam is unresolved. Make it an explicit invariant: **lossy, drop-oldest** TX; the cpal callback must never block; latency must never grow to buffer. | `ARCHITECTURE.md:53-59` |
| A3 | High | The session FSM is the least-designed part and is the M3 integration point that M0/M1/M2 never exercise. Sketch a one-page state diagram in M2 (glare when both key down; symmetric host+dial vs caller/callee; Ping/Pong timeout vs Hangup). | `ARCHITECTURE.md:56`; `PROTOCOL.md:58-61` |
| A4 | High | arti onion-*service* maturity is the real M0 risk; the spike proves a one-shot 31-byte echo. Strengthen M0 exit: hold a stream ≥5 min with periodic traffic + survive a forced circuit rebuild; verify vanguards are *active*, not just configured. | `ROADMAP.md:14`; `m0_spike.rs` |
| A5 | Med | `audiopus`/libopus + CMake/MSVC (and musl-static + Android NDK cross-compile) is a deferred build-chain landmine; add a tiny "does it build/link on Windows/musl/NDK" spike *next to* M0, not at M1. | `Cargo.toml:37`; `ROADMAP.md:69-70` |
| A6 | Med | Daemon mode (if chosen at M5) adds a new local control-socket trust boundary needing peer-cred/permission auth + a `THREAT_MODEL` line. | `ARCHITECTURE.md:93-97` |
| A7 | Med | 8 kHz-mono canonical format is rarely device-native; a resampler is an unlisted dependency and latency contributor. | `ARCHITECTURE.md:47-48` |
| A8 | Low | `shroud-proto` frame codec is pure/zero-risk and need not wait for M2 — build it anytime as fill-in work. **(Done in this review.)** | `ROADMAP.md:35` |

**Decide NOW (cheap, no code):** A1 seam; A2 lossy-drop invariant; A4 stronger M0 exit; reconcile the two doc/manifest contradictions (`Cargo.toml:21` vs `:25`; `ARCHITECTURE.md:114` vs `PROTOCOL.md:53`).

---

## What this review changed in the tree

- **Implemented `shroud-proto`**: the generic `[type:u8][len:u16-be][payload]` envelope with
  `encode` / `encode_into` / `decode`, a typed dependency-free `FrameError`, length validation
  *before* allocation, and a full unit-test suite (round-trip, empty/max payload, oversize
  rejection, short header/payload, trailing-byte `consumed` accounting, multi-frame streaming,
  unknown-type-byte preservation). Removed the unused `anyhow` dependency. (Addresses C4, C7, A8.)
- **`shroud-core`**: `#[derive(Default)]` on `ShroudClient` to clear `new_without_default`. (C5.)
- **`shroud-core/Cargo.toml`**: added `libsqlite3-sys = { features = ["bundled"] }` so arti's
  transitive `rusqlite` compiles SQLite from source instead of linking a system `sqlite3.lib`
  (required on Windows; also the right call for the self-contained static-binary goal). (C3-adjacent.)
- **`m0_spike.rs`**: fixed the non-existent `tor_client.config()` vanguards check (C8) and the
  echo teardown so the spike completes a real round-trip on live Tor (C1, C2 — see below).
- **`Cargo.lock`**: now generated and committed (C3).

## Runtime validation (M0) — verified on live Tor

Toolchain installed (Rust 1.96.0 MSVC + CMake; MSVC C++/SDK already present) and the full
workspace was built and tested end to end:

- `cargo build --workspace --all-targets` — ✅ (~469 crates incl. ring/sqlite/zstd/lzma C deps)
- `cargo test --workspace` — ✅ 11/11 `shroud-proto` tests, clippy clean
- **M0 spike on the real Tor network** — ✅ in-process onion service hosted, self-dialed, and
  bytes echoed both directions with **zero external `tor` process**:
  ```
  ONION SERVICE HOSTED!  Address: mcgxzhkv…oad.onion
  Service Received:         Hello through Tor onion service!
  Client Received Response: Echo: Hello through Tor onion service!
  M0 spike successful!
  ```

### Correction to C1/C2 — Tor stream close semantics (learned by running it)

The review originally recommended a TCP-style "half-close + `read_to_end`" (Option B). **Running
the spike disproved this.** From `tor-proto` 0.23 source:

- **No half-close.** A Tor `END` cell terminates the stream in *both* directions; calling
  `shutdown()` then reading fails with `stream channel disappeared without END cell`.
- **Drop ⇒ `MISC`.** `CloseStreamBehavior::default()` is `SendEnd(End::new_misc())`, so dropping
  any *accepted* stream sends `END/MISC`. The reader treats any reason ≠ `DONE` as an error
  (`data.rs:294-295`). A clean `DONE` is only reachable via `close_pending` on a *not-yet-accepted*
  stream — not available in the normal accept→stream→drop flow.
- **Real fix:** server `read → write → flush → drop` (no `shutdown`); client `write → flush →
  single read` (the echo Data cell arrives ahead of the `END`). The missing `flush()` (C2) was the
  true root cause. Length-framed protocols (`shroud-proto`) should read by length and tolerate the
  `MISC` end explicitly rather than relying on a clean EOF.

### Runtime follow-ups — addressed
- **S8 (ephemeral onion) — done.** The spike now runs arti's state/keystore in a `tempfile`
  temp dir wiped on exit, minting a fresh `.onion` per session (verified: the address changed
  between runs). Caveat: arti 0.23 has no *true* in-memory onion key (`launch_onion_service`
  can't override KeyMgr/StateMgr — arti #1186), so the key briefly touches a temp dir; on
  Linux/Termux a tmpfs `TMPDIR` keeps it RAM-only. A fully in-memory key awaits arti.
- **A4 (sustained + reconnect) — done.** The spike holds one stream open with periodic
  keepalive traffic (`SHROUD_M0_SECS`, default 60, M0 target ≥ 300) and then re-dials on a
  fresh stream. Verified on live Tor: 11 keepalives over 61s + a clean reconnect. A *forced
  circuit rebuild* (vs. a fresh dial) is still a stronger test to add later.

Everything else above is advice, not edits — the protocol and architecture decisions are the
maintainer's to make.

---

# M2 secure transport — implementation + review

`shroud-core::transport` now implements the M2 crypto slice: a Noise (`snow`) handshake +
AEAD transport carrying `shroud-proto` frames, with Argon2id PSK derivation. Pure crypto +
framing, no audio/network — **9 unit tests pass, clippy clean**. Reviewed by the security and
code agents (findings + resolutions below).

**Design choices implemented:** `Noise_NNpsk0_25519_ChaChaPoly_BLAKE2s`; explicit PSK source
(`Psk::from_raw` vs `from_passphrase`, never inferred — S2); deterministic domain-separated
Argon2id salt from a session `label` (S3); frozen Argon2id params (64 MiB / t=3 / p=1);
replay/reorder rejection via snow's sequential nonce (S4), test-verified against snow's source.

**Verified correct by review (no change needed):** NNpsk0 message ordering + `psk(0,…)`
placement; all buffer sizing against snow's actual bounds; every `as u16` cast is provably
lossless; the `FrameTooLarge` guard is load-bearing (a max proto frame *can* exceed 65519);
role separation; no panics; explicit-only raw-key bypass.

**Addressed in this slice (from the M2 review):**
- Corrected docs: `snow` 0.9 does **not** zeroize derived session keys (only `Psk` is wiped) —
  documented as a known limit; pair with `mlock`/core-dump disabling at M3 (K-1).
- `recv_frame` now returns `TransportError::Closed` on clean peer EOF (distinct from I/O fault) (H2).
- `open_frame` rejects authenticated trailing bytes — enforces one-frame-per-message (M1).
- `from_passphrase` rejects empty passphrase/label and documents that **the label is the salt**
  (must be unique + hard-to-guess, or precomputation defeats Argon2) (A-1/S3).
- `debug_assert` the `u16` framing invariants (L5); added tests for the `FrameTooLarge`
  boundary, empty payload, and short-message cases.

**Deferred — must-fix before M2 *exit* (not this slice):**
1. **Timeouts + concurrency bounds** on handshake/`recv_frame` — no listener ships yet, so this
   belongs at the M3 accept layer; documented as a caller MUST in the module docs (D-1).
2. **Session-failure invariant**: any decrypt error is fatal → drop session + full re-handshake;
   bound the reconnect loop. Documented as a caller MUST; add a reconnect test at M3 (S4/R-2).
3. **`XKpsk2`** remains the recommended upgrade over `NNpsk0` (PSK is the sole authenticator;
   a reachable responder is an online passphrase-guessing oracle throttled only by Argon2 cost) (S1).
4. Benchmark Argon2id on the weakest target (Termux) before freezing the cost params (S3).
