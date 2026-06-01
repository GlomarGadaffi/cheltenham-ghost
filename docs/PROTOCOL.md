# Wire Protocol (draft)

Status: **draft, not frozen.** Frozen at M2 exit.

Everything below rides *inside* a Tor onion-service stream, so the transport is already
confidential and the responder is authenticated by its `.onion` address. This protocol
adds the application-layer security (mutual auth from the shared secret, forward secrecy)
and the message structure.

## Layers

```
  application frames  (this doc)
  ────────────────────────────────
  Noise transport     (snow; AEAD, forward secrecy, nonce sequencing)
  ────────────────────────────────
  Tor onion stream    (arti DataStream)
```

## Handshake

A Noise handshake keyed by the pre-shared secret. Candidate patterns:

- **`NNpsk0`** — simplest; both sides hold the PSK, no static keys. Mutual auth derives
  entirely from PSK knowledge. Good default for a two-party walkie-talkie.
- **`XKpsk2`** — if we later give each peer a long-term static key (identity beyond the
  shared secret), this adds responder identity hiding + initiator auth.

Decision deferred to M2; `NNpsk0` is the working assumption. PSK is derived from the
user's shared secret via HKDF (not used raw).

The old `CIPHER:` negotiation verb is **removed** — cipher choice is fixed by the Noise
suite (e.g. `Noise_NNpsk0_25519_ChaChaPoly_BLAKE2s`), not announced at runtime.

## Frame format

After the handshake, every Noise transport message carries exactly one application frame:

```
  ┌────────┬──────────┬────────────────────┐
  │ type:1 │ len:2 BE │ payload: len bytes  │
  └────────┴──────────┴────────────────────┘
```

- `type`  — u8 frame type (table below).
- `len`   — u16 big-endian payload length (0–65535; opus speech frames are far smaller).
- payload — type-dependent; for `Audio`, one encoded opus frame.

(The Noise layer supplies its own length framing on the wire; the `len` field is the
*inner* application length so a reader can validate before allocating.)

## Frame types

| type | name | payload | direction | notes |
| --- | --- | --- | --- | --- |
| 0x01 | `Hello` | onion addr (optional) | both | post-handshake greeting; replaces `ID:` |
| 0x02 | `PttStart` | — | both | speaker keyed down |
| 0x03 | `PttStop` | — | both | speaker keyed up |
| 0x04 | `Audio` | opus frame | both | only valid between PttStart/PttStop |
| 0x05 | `Ping` | u32 seq | both | liveness / RTT probe |
| 0x06 | `Pong` | u32 seq | both | echo of Ping seq |
| 0x07 | `Hangup` | — | both | graceful teardown |
| 0x08 | `Msg` | UTF-8 | both | short text (out-of-band coordination) |
| 0x10 | `Relay` | u8 ver | server→client | relay greeting; replaces `RELAY:1` |
| 0x11 | `Group` | varies | both | relay group control |

Unknown types MUST be ignored (forward-compat), not fatal.

## Replay & ordering

Handled by the Noise transport's per-message nonce counter — no application-level nonce
log (unlike TerminalPhone). Out-of-order or replayed Noise messages fail decryption and
the stream is torn down.

## Traffic-analysis resistance  *(M4)*

Open knobs, off by default until measured:

- **Padding:** pad `Audio` payloads to a fixed size so frame length carries no information
  about speech content. Costs bandwidth over Tor; quantify before enabling.
- **Cover traffic:** emit padded silent frames at a constant cadence so PTT timing
  (when someone is talking) isn't visible as a traffic envelope. Expensive; opt-in.
- **Constant-rate mode:** combine the two for a fixed-bitrate stream regardless of speech.

These are the features that distinguish "encrypted" from "metadata-resistant." Decide
per-deployment, document the tradeoffs, don't enable silently.

## Open questions

- [ ] `NNpsk0` vs `XKpsk2` — do peers have stable identities, or only the shared secret?
- [ ] Relay protocol: keep the N-caller bridge semantics; redesign the fan-out without FIFOs.
- [ ] Versioning: a `type`/version byte in `Hello` so the protocol can evolve.
