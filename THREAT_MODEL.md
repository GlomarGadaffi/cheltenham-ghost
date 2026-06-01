# Threat Model (draft)

A security tool that doesn't state what it does *not* protect against is a liability.
This document is the contract. It is a draft and will be revised as the design firms up.

## Assets

- **Content** of the voice conversation.
- **Metadata:** who is talking to whom, when, for how long, and from where.
- **The shared secret** (and any persisted onion key).

## In scope — what shroud-speak aims to defend against

1. **A network adversary** (ISP, local network, on-path observer) learning conversation
   content. *Mitigation:* Tor stream + Noise AEAD. Content is encrypted twice over.
2. **Endpoint location discovery via the network.** *Mitigation:* onion services — neither
   party learns the other's IP; the address is a public key, not a host.
3. **Impersonation / connecting to the wrong party.** *Mitigation:* the `.onion` authenticates
   the responder; the Noise PSK authenticates both ends mutually.
4. **Replay and tampering.** *Mitigation:* Noise transport nonce sequencing + AEAD.
5. **At-rest disclosure of the shared secret** on a powered-off device. *Mitigation:*
   `argon2` + AEAD encryption of the secret; key material `zeroize`d and page-locked in use.
6. **Forensic recovery of conversation audio.** *Mitigation:* audio never touches disk;
   RAM-only ring buffers; optional ephemeral onion (no persisted key at all).

## Out of scope — what shroud-speak does NOT protect against

State these plainly so nobody is misled:

1. **Endpoint compromise.** A rooted/jailbroken device, malware, a keylogger, or a screen
   recorder defeats shroud-speak completely. It protects the channel, not the machine.
2. **Coerced disclosure.** Rubber-hose / legal compulsion of the shared secret or device.
3. **Global passive adversary traffic analysis.** Tor's own threat model excludes an
   adversary who can watch the whole network; shroud-speak inherits that limit. Padding / cover
   traffic (M4) *raises the cost* of timing correlation but does not defeat a GPA.
4. **Voice biometrics.** The protocol hides *who is connected*; it does not disguise a
   *recognizable voice*. Voice effects (M4) are obfuscation, not anonymity — treat them
   as theater, not defense.
5. **Operational mistakes.** Exchanging the onion address or shared secret over an insecure
   channel, reusing secrets, leaving a session up. The tool can't fix opsec.
6. **Memory forensics of a running/suspended process.** Page-locking and zeroize reduce but
   do not eliminate exposure of keys in a live process to an attacker with that access.
   A live RAM capture of an unlocked device is endpoint compromise (see #1).

## Platform caveat

The anti-forensic posture is strongest on Linux/Termux where RAM-only operation and page
locking are clean. On other platforms, OS behaviors (swap, snapshots, crash dumps) can
undercut "never to disk" guarantees in ways outside the program's control. Document the
per-platform reality at M5; do not promise uniform guarantees across platforms.

## Trust assumptions

- The Tor network behaves per its own threat model (no GPA, honest-but-curious relays
  bounded by Tor's design).
- `arti`, `snow`, `audiopus`/libopus, and the Rust toolchain are not backdoored. (Reproducible
  builds + release signing at M5 let *others* verify the binary matches this source.)
- The user controls their endpoint. Everything rests on this; see out-of-scope #1.

## Non-goals as design discipline

shroud-speak is deliberately a **point-to-point walkie-talkie**, not a messenger. No user
directory, no presence, no message history, no accounts. Every feature that would create
durable metadata is a non-goal, because the cheapest metadata to protect is the kind that
was never generated.
