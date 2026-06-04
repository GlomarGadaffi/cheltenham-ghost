# Changelog

All notable changes to this project are documented here. This project is **pre-alpha**;
versions before `v0.1.0` are milestone markers, not stable releases.

## [v0.0.1-alpha] — M0 milestone (2026-06-04)

**Status: pre-alpha. M0 (in-process onion transport) only — NO audio, NO Noise crypto yet.
Not for real-world use.** See `ROADMAP.md`; `v0.1.0` is reserved for the M3 voice call.

### M0 proven on the live Tor network
- The in-process onion-service premise works end to end with **zero external `tor` process**:
  a `TorClient` bootstraps, hosts an onion service, self-dials its own `.onion`, and round-trips
  bytes both directions. This is the M0 exit criterion and the one milestone the roadmap says
  could kill the project.
- **Ephemeral onion (S8):** arti state/keystore live in a temp dir wiped on exit → a fresh
  `.onion` every run, nothing persisted across sessions. (True in-memory keys await arti #1186;
  on Linux/Termux a tmpfs `TMPDIR` keeps the key RAM-only.)
- **Sustained + reconnect (A4):** the spike holds one stream open with periodic keepalive
  traffic (`SHROUD_M0_SECS`, default 60s; M0 target ≥ 300s) and then re-dials on a fresh
  stream. Verified: 11 keepalives over 61s + a clean reconnect.

### Added
- `shroud-proto`: the generic `[type:u8][len:u16-be][payload]` frame envelope — `encode` /
  `encode_into` / `decode`, a typed dependency-free `FrameError`, length validation before
  allocation, and an 11-test unit suite. (Previously a stub.)
- `REVIEW.md`: consolidated security / code / architecture review + live-Tor runtime validation.
- `.gitignore` (`target/`) and a committed `Cargo.lock` (binary + reproducible-build goal).

### Fixed
- `m0_spike.rs`: removed the non-existent `TorClient::config()` vanguards call (arti 0.23) and
  corrected stream teardown. **Key finding:** Tor streams have *no half-close* — an `END` cell
  closes both directions, and dropping an accepted stream sends `END/MISC`
  (`CloseStreamBehavior::default`), which a reader treats as an error. The real bug was a
  missing `flush()`. Correct pattern: read → write → flush → drop (no `shutdown`).
- Windows/static build: forced `libsqlite3-sys` `bundled` so arti's transitive `rusqlite`
  compiles SQLite from source instead of linking a system `sqlite3.lib`.

### Known limitations / next
- No audio pipeline (M1) and no Noise transport (M2) yet — this release is transport-only.
- Decide-before-M2 items in `REVIEW.md`: Noise pattern (`XKpsk2` vs `NNpsk0`), Argon2id salt +
  parameters, 2-party-vs-relay wire-format scope, traffic-analysis defaults.
- A *forced circuit rebuild* test (stronger than a fresh re-dial) is still to add for M0.
