# Architecture

## Principle

One async process (tokio) owns every primitive in memory. No subprocesses, no FIFOs,
no temp files, no `torrc` on disk. The Bash version's entire IPC layer — named pipes,
`socat`, file-descriptor channels (fd 3/4/9), flag-file polling — has no analogue here
and is simply absent; its job is done by in-process channels and a real socket API.

```
  ┌─────────────────────────────────────────────────────────────┐
  │                        shroud-speak process                         │
  │                                                               │
  │   cpal capture ─▶ ring buf ─▶ opus enc ─▶ Noise/AEAD ─┐       │
  │                                                       ▼       │
  │                                              arti DataStream  │
  │                                              (onion service)  │
  │                                                       ▲       │
  │   cpal playback ◀─ ring buf ◀─ opus dec ◀─ Noise/AEAD ┘       │
  │                                                               │
  │   control: PTT state, PING/HANGUP, session FSM (tokio mpsc)   │
  └─────────────────────────────────────────────────────────────┘
```

Nothing in the audio path touches the filesystem.

## Crate mapping (primitive → in-process replacement)

| Primitive (TerminalPhone) | Crate | Notes |
| --- | --- | --- |
| `tor` daemon, torrc, hostname file | `arti-client` | `TorClient::launch_onion_service`; key in the arti keystore. *No true in-memory ephemeral key in arti 0.23 ([arti#1186](https://gitlab.torproject.org/tpo/core/arti/-/issues/1186)); M0 approximates it with a temp-dir state store wiped on exit.* |
| `socat` SOCKS4A / TCP-LISTEN, `mkfifo` | `arti-client` streams | `DataStream` read/write halves; no SOCKS hop |
| `openssl enc` (AES-CBC) + `openssl dgst` (HMAC) | `snow` (Noise) | AEAD transport; replaces encrypt-then-MAC entirely |
| `openssl ... -pbkdf2` (secret at rest) | `argon2` | memory-hard KDF; PBKDF2 was the weak link |
| `/dev/urandom`, `od`, `tr` | `getrandom` / `OsRng` | |
| `opusenc` / `opusdec` | `audiopus` | libopus binding, statically linkable |
| `arecord`/`aplay`/`rec`/`play`/`ffmpeg`/`termux-microphone-record` | `cpal` | single cross-platform capture+playback abstraction |
| `sox` voice effects | `fundsp` or hand-rolled | pitch / overdrive / flanger / echo / highpass / tremolo as DSP nodes |
| `stty` raw, ANSI, `tput` | `crossterm` | raw mode + key events + alt-screen |
| `qrencode` | `qrcode` | onion address → terminal QR |
| config `source`, PID files, `$$` temps | in-memory structs | optional persisted config parsed, never `eval`'d |
| key zeroization (none) | `zeroize` + `mlock` | secrets wiped on drop, pages locked |
| `install_deps`, package-manager branches | — | deleted; no runtime deps |

## Audio pipeline

Canonical internal format stays **PCM S16LE, mono, 8 kHz** (matches TerminalPhone; good
for speech over a high-latency circuit). Opus configured `--speech`-equivalent, ~16 kbps,
60 ms frames. The pipeline is two tokio tasks plus a control task:

- **TX task:** `cpal` input callback fills a lock-free ring (`rtrb`/`ringbuf`); a frame
  pump pulls 60 ms windows, optional DSP, opus-encode, Noise-encrypt, frame, write to stream.
  Gated by PTT state (no PTT ⇒ pump idles, mic stays open but frames are dropped).
- **RX task:** read frames from stream, Noise-decrypt, opus-decode, push to playback ring
  drained by the `cpal` output callback.
- **Control task:** owns the session FSM and the PTT signal; fans control verbs over `mpsc`.

Latency budget is the thing to watch: Tor adds 100–500 ms; keep local buffering minimal and
size rings for one or two frames, not seconds.

## Transport & crypto

- **Tor layer (arti):** host the onion service in-process; the same `TorClient` dials the
  peer's `.onion`. The onion address *is* the server's public key, so connecting to the
  right address authenticates the responder at the Tor layer.
- **App layer (Noise):** a `snow` handshake (candidate patterns `NNpsk0` or `XKpsk2`) keyed
  by the shared secret gives mutual auth, forward secrecy, and a clean key schedule on top
  of Tor. This is the principled replacement for TerminalPhone's hand-rolled
  cipher-announce + PBKDF2-per-chunk + HMAC-with-nonce-log scheme. Replay protection comes
  from the Noise transport's nonce sequencing — the nonce-log file disappears.
- **arti restricted discovery (client auth)** can optionally gate who can even *reach* the
  service, complementing (not replacing) the Noise PSK. Decide in M2.

## Wire protocol

Length-prefixed binary frames over the Noise transport (see [`PROTOCOL.md`](PROTOCOL.md)):

```
[ type : u8 ] [ len : u16-be ] [ payload : len bytes ]
```

Frame types map onto the old text verbs: `Audio`, `Id`, `Cipher`(→ negotiated in handshake,
likely dropped), `PttStart`, `PttStop`, `Ping`, `Hangup`, `Msg`, plus relay verbs
`Relay`, `Group`. Audio payload is the AEAD ciphertext of one opus frame. Optional fixed-size
padding for traffic-analysis resistance is a frame-layer concern, decided in PROTOCOL.

## UI / process model  *(open decision)*

Two viable shapes:

1. **TUI monolith** — `crossterm` + `ratatui`. Closest to TerminalPhone, runs over SSH,
   minimal attack surface, single binary. PTT via key events. Simplest path to M3.
2. **Headless daemon + thin clients** — engine exposes a local control socket (UDS on
   unix, named pipe on Windows). PTT becomes a control message. This makes physical
   hardware (a pocket-dial-class device, a gatekeeper door station) a first-class client
   of the same engine, and lets a TUI, CLI, or GUI all be thin front-ends. More work,
   strictly more flexible, and aligned with the broader hardware direction.

Recommendation (settled): `shroud-core` is a library from day one, so the voice front-end
— and anything bolted on later — is a thin shell over the same engine. That defers the
TUI-vs-daemon decision to M3/M5 at no cost; M0–M2 are identical either way.

## Crate layout

`shroud` is the platform, `speak` is the first capability. The substrate is medium-agnostic
on purpose: voice is one payload type, and a later `shroud-text` / `shroud-drop` reuses the
same onion+Noise+framing spine rather than forking it.

```
shroud-speak/
  Cargo.toml            # workspace
  crates/
    shroud-core/        # SUBSTRATE: arti onion transport, Noise, session plumbing — medium-agnostic, no UI
    shroud-proto/       # generic frame envelope (type/len/payload) + (de)serialization, no I/O
    shroud-speak/       # THE VOICE APP: cpal audio pipeline, voice frame types, front-end (bin)
  docs/
```

(Single-crate to start is fine; split when `core` stabilizes. The workspace stub in the
root `Cargo.toml` reflects this target.)
