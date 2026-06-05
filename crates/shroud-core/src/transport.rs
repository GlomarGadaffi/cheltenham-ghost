//! M2 secure transport: a Noise handshake + AEAD transport carrying `shroud-proto` frames.
//!
//! This is the application-layer crypto that rides *inside* a Tor onion stream (or, for
//! testing, any `AsyncRead + AsyncWrite`). It provides mutual authentication from a shared
//! secret, forward secrecy, and replay/reorder rejection via the Noise transport's nonce
//! sequencing — replacing TerminalPhone's hand-rolled cipher + PBKDF2 + HMAC-nonce-log.
//!
//! Layering (see `PROTOCOL.md`):
//! ```text
//!   shroud-proto frame   [type:u8][len:u16-be][payload]   (this is the plaintext)
//!   Noise transport      snow; AEAD, forward secrecy, nonce sequencing
//!   wire framing         [u16-be len][noise message bytes]
//!   (carrier)            Tor onion stream / TCP / any AsyncRead+AsyncWrite
//! ```
//!
//! ## Decisions applied from the design review (see `REVIEW.md`)
//! - **PSK source is explicit, never inferred** (review S2): the caller picks
//!   [`Psk::from_raw`] (a 32-byte CSPRNG key) or [`Psk::from_passphrase`]; we never guess a
//!   mode from the input's shape, which would let a passphrase silently skip Argon2id.
//! - **Deterministic, domain-separated salt** (review S3): there is no server and no channel
//!   before the handshake, so both peers must derive the *same* PSK from the same passphrase
//!   with no prior communication. A random salt is impossible; we derive the salt
//!   deterministically from a protocol-version domain tag plus an out-of-band session label.
//! - **Argon2id parameters are frozen protocol constants** — both peers must agree.
//!
//! ## Operational requirements (the caller MUST uphold these — agency review S4 / D-1)
//! This module provides the crypto and framing, not the session policy. A caller wiring it
//! to a real carrier MUST:
//! - **Bound every await with a timeout.** `handshake_*` and `recv_frame` block on the peer;
//!   wrap them in `tokio::time::timeout` and cap concurrent in-flight handshakes, or a stalled
//!   peer parks the task (slowloris / connection-exhaustion DoS). Not enforced here because no
//!   listener ships yet.
//! - **Treat any decrypt/recv error as fatal.** On `Err` from `recv_frame`/`open_frame`, drop
//!   the [`NoiseSession`] and perform a full re-handshake. Never reuse a session across a
//!   reconnect (would risk nonce/key misuse); `into_inner` consumes the transport to make
//!   reuse awkward by design. Bound the reconnect loop.
//! - **Choose a unique, hard-to-guess `label`** for [`Psk::from_passphrase`] — the label *is*
//!   the Argon2 salt (see that method's docs).
//!
//! ## Not yet decided / known limits
//! - Pattern is **`NNpsk0`** (the documented working assumption). The review recommends
//!   `XKpsk2` (static-key auth survives a weak PSK); that is a deliberate future upgrade and
//!   is intentionally *not* implemented in this first slice. With `NNpsk0` the PSK is the sole
//!   authenticator, so a reachable responder is an online passphrase-guessing oracle throttled
//!   only by the Argon2id cost (and the timeouts above) — keep that cost high.
//! - **Key material inside `snow` is not zeroized.** `snow` 0.9 implements no `Drop`/`Zeroize`,
//!   so the derived session keys (and snow's internal PSK copy) live un-wiped for the session.
//!   Only this crate's [`Psk`] is zeroized. Pair with `mlock` / core-dump disabling at M3 for
//!   the anti-forensic posture.
//! - One `shroud-proto` frame maps to one Noise message, so a frame's encoded size must fit
//!   one Noise message (<= 65519 plaintext bytes). Audio frames are ~150 bytes; oversize
//!   frames return [`TransportError::FrameTooLarge`].

use shroud_proto::Frame;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use zeroize::Zeroizing;

/// Noise suite. Cipher choice is fixed here, not negotiated on the wire.
const NOISE_PARAMS: &str = "Noise_NNpsk0_25519_ChaChaPoly_BLAKE2s";

/// Length of a pre-shared key in bytes.
pub const PSK_LEN: usize = 32;

/// Maximum Noise message on the wire (snow limit), and the resulting max plaintext.
const NOISE_MAX_MSG: usize = 65535;
const NOISE_TAG_LEN: usize = 16;
const NOISE_MAX_PLAINTEXT: usize = NOISE_MAX_MSG - NOISE_TAG_LEN;

// --- Argon2id parameters (frozen protocol constants; both peers MUST agree) ---
const ARGON2_MEM_KIB: u32 = 64 * 1024; // 64 MiB
const ARGON2_TIME_COST: u32 = 3;
const ARGON2_LANES: u32 = 1;
/// Domain separator mixed into the deterministic salt; bump the version to rotate.
const PSK_SALT_DOMAIN: &[u8] = b"shroud-speak/psk-salt/v1:";

/// Errors from the secure transport.
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("noise error: {0}")]
    Noise(#[from] snow::Error),
    #[error("frame codec error: {0}")]
    Frame(#[from] shroud_proto::FrameError),
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("argon2 error: {0}")]
    Kdf(String),
    #[error("frame too large for one Noise message: {0} > {NOISE_MAX_PLAINTEXT}")]
    FrameTooLarge(usize),
    #[error("{0} trailing byte(s) after the frame in one Noise message")]
    TrailingBytes(usize),
    #[error("transport closed by peer")]
    Closed,
}

type Result<T> = std::result::Result<T, TransportError>;

/// A 32-byte pre-shared key. The *source* is chosen explicitly by the caller (review S2).
///
/// No `Debug`/`Clone` by design: the key bytes can't leak into logs or be copied carelessly,
/// and they are zeroized on drop. (Note: once handed to `snow`, the derived session keys are
/// *not* zeroized — see the module-level "known limits".)
pub struct Psk(Zeroizing<[u8; PSK_LEN]>);

impl Psk {
    /// Use a raw high-entropy 32-byte key directly (e.g. CSPRNG output). No KDF stretching —
    /// only correct for genuinely random keys.
    pub fn from_raw(key: [u8; PSK_LEN]) -> Self {
        Psk(Zeroizing::new(key))
    }

    /// Derive the PSK from a human passphrase via Argon2id.
    ///
    /// Both peers must pass the *same* `passphrase` and `label` to derive the same key.
    ///
    /// **`label` is the Argon2 salt.** Because there is no server and no channel before the
    /// handshake, the salt must be reproducible from shared inputs, so it is derived
    /// deterministically from `label` (review S3). Consequences the caller MUST heed:
    /// - Use a **unique label per conversation** — reusing a label reuses the salt.
    /// - Make the label **hard to guess / high-entropy** (e.g. both onion addresses + a date
    ///   or nonce). A constant or guessable label collapses the salt to one value and lets an
    ///   attacker precompute a dictionary against all users sharing it — defeating Argon2's
    ///   purpose. The Argon2 work factor is the only thing protecting a weak passphrase.
    ///
    /// Returns an error if `passphrase` or `label` is empty.
    pub fn from_passphrase(passphrase: &str, label: &str) -> Result<Self> {
        use argon2::{Algorithm, Argon2, Params, Version};
        use sha2::{Digest, Sha256};

        if passphrase.is_empty() || label.is_empty() {
            return Err(TransportError::Kdf(
                "passphrase and label must be non-empty".to_string(),
            ));
        }

        // Deterministic, domain-separated salt. There is no server to store a random salt and
        // no channel before the handshake, so the salt must be reproducible from shared
        // inputs. Argon2 still provides the memory-hard work factor; the constant-per-label
        // salt means identical (passphrase, label) pairs derive identical PSKs by design.
        let mut h = Sha256::new();
        h.update(PSK_SALT_DOMAIN);
        h.update(label.as_bytes());
        let salt = h.finalize();

        let params = Params::new(ARGON2_MEM_KIB, ARGON2_TIME_COST, ARGON2_LANES, Some(PSK_LEN))
            .map_err(|e| TransportError::Kdf(e.to_string()))?;
        let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);

        let mut key = Zeroizing::new([0u8; PSK_LEN]);
        argon
            .hash_password_into(passphrase.as_bytes(), &salt[..16], key.as_mut_slice())
            .map_err(|e| TransportError::Kdf(e.to_string()))?;
        Ok(Psk(key))
    }

    fn bytes(&self) -> &[u8] {
        &self.0[..]
    }
}

/// The post-handshake AEAD session: pure crypto, no I/O. Encrypts/decrypts `shroud-proto`
/// frames. Replay/reorder is rejected by the underlying Noise nonce sequence.
pub struct NoiseSession {
    noise: snow::TransportState,
}

impl NoiseSession {
    fn new(noise: snow::TransportState) -> Self {
        Self { noise }
    }

    /// Encrypt one frame into a Noise message (its own bytes; no wire length prefix).
    pub fn seal_frame(&mut self, frame: &Frame) -> Result<Vec<u8>> {
        let plaintext = frame.encode()?;
        if plaintext.len() > NOISE_MAX_PLAINTEXT {
            return Err(TransportError::FrameTooLarge(plaintext.len()));
        }
        let mut out = vec![0u8; plaintext.len() + NOISE_TAG_LEN];
        let n = self.noise.write_message(&plaintext, &mut out)?;
        out.truncate(n);
        Ok(out)
    }

    /// Decrypt one Noise message back into a frame. A tampered, replayed, or reordered
    /// message fails AEAD/nonce verification and returns an error.
    pub fn open_frame(&mut self, message: &[u8]) -> Result<Frame> {
        // The plaintext is at most `message.len()` (ciphertext minus the AEAD tag); snow
        // returns the true length in `n`. A message shorter than the tag fails cleanly in
        // `read_message` rather than panicking.
        let mut plaintext = vec![0u8; message.len()];
        let n = self.noise.read_message(message, &mut plaintext)?;
        let (frame, consumed) = Frame::decode(&plaintext[..n])?;
        // Enforce the "exactly one frame per Noise message" invariant: authenticated trailing
        // bytes are an encoder bug or a smuggling attempt, not silently ignored.
        if consumed != n {
            return Err(TransportError::TrailingBytes(n - consumed));
        }
        Ok(frame)
    }
}

/// A secure transport bound to a byte stream `S`: sends/receives `shroud-proto` frames,
/// each as one length-prefixed Noise message.
pub struct NoiseTransport<S> {
    stream: S,
    session: NoiseSession,
}

impl<S: AsyncRead + AsyncWrite + Unpin> NoiseTransport<S> {
    /// Send one frame: seal it, then write `[u16-be len][noise message]` and flush.
    pub async fn send_frame(&mut self, frame: &Frame) -> Result<()> {
        let msg = self.session.seal_frame(frame)?;
        // `seal_frame` guarantees `msg.len() <= NOISE_MAX_MSG`, so the u16 cast is lossless.
        debug_assert!(msg.len() <= NOISE_MAX_MSG);
        self.stream.write_u16(msg.len() as u16).await?;
        self.stream.write_all(&msg).await?;
        self.stream.flush().await?;
        Ok(())
    }

    /// Receive one frame: read the length-prefixed Noise message, then open it.
    ///
    /// A clean peer disconnect (EOF at a message boundary) returns [`TransportError::Closed`]
    /// so callers can distinguish "peer hung up" from a real I/O fault without string-matching.
    pub async fn recv_frame(&mut self) -> Result<Frame> {
        let len = match self.stream.read_u16().await {
            Ok(len) => len as usize,
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                return Err(TransportError::Closed)
            }
            Err(e) => return Err(e.into()),
        };
        let mut msg = vec![0u8; len];
        self.stream.read_exact(&mut msg).await?;
        self.session.open_frame(&msg)
    }

    /// Consume the transport, returning the underlying stream (e.g. for clean teardown).
    pub fn into_inner(self) -> S {
        self.stream
    }
}

fn builder(psk: &Psk) -> Result<snow::Builder<'_>> {
    let params: snow::params::NoiseParams = NOISE_PARAMS.parse()?;
    Ok(snow::Builder::new(params).psk(0, psk.bytes()))
}

/// Run the Noise handshake as the **initiator** over `stream`, returning a ready transport.
pub async fn handshake_initiator<S: AsyncRead + AsyncWrite + Unpin>(
    mut stream: S,
    psk: &Psk,
) -> Result<NoiseTransport<S>> {
    let mut hs = builder(psk)?.build_initiator()?;
    let mut buf = vec![0u8; NOISE_MAX_MSG];

    // NNpsk0 msg 1 (-> psk, e)
    let n = hs.write_message(&[], &mut buf)?;
    write_wire(&mut stream, &buf[..n]).await?;

    // NNpsk0 msg 2 (<- e, ee)
    let msg = read_wire(&mut stream).await?;
    hs.read_message(&msg, &mut buf)?;

    let session = NoiseSession::new(hs.into_transport_mode()?);
    Ok(NoiseTransport { stream, session })
}

/// Run the Noise handshake as the **responder** over `stream`, returning a ready transport.
pub async fn handshake_responder<S: AsyncRead + AsyncWrite + Unpin>(
    mut stream: S,
    psk: &Psk,
) -> Result<NoiseTransport<S>> {
    let mut hs = builder(psk)?.build_responder()?;
    let mut buf = vec![0u8; NOISE_MAX_MSG];

    // NNpsk0 msg 1 (-> psk, e)
    let msg = read_wire(&mut stream).await?;
    hs.read_message(&msg, &mut buf)?;

    // NNpsk0 msg 2 (<- e, ee)
    let n = hs.write_message(&[], &mut buf)?;
    write_wire(&mut stream, &buf[..n]).await?;

    let session = NoiseSession::new(hs.into_transport_mode()?);
    Ok(NoiseTransport { stream, session })
}

async fn write_wire<S: AsyncWrite + Unpin>(stream: &mut S, bytes: &[u8]) -> Result<()> {
    debug_assert!(bytes.len() <= NOISE_MAX_MSG);
    stream.write_u16(bytes.len() as u16).await?;
    stream.write_all(bytes).await?;
    stream.flush().await?;
    Ok(())
}

async fn read_wire<S: AsyncRead + Unpin>(stream: &mut S) -> Result<Vec<u8>> {
    let len = stream.read_u16().await? as usize;
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf).await?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Frame type bytes (canonical table lives in PROTOCOL.md).
    const HELLO: u8 = 0x01;
    const AUDIO: u8 = 0x04;

    /// Synchronous in-memory NNpsk0 handshake, returning both transport-mode sessions.
    /// Lets the crypto be tested without any async stream.
    fn handshake_in_memory(init_psk: &Psk, resp_psk: &Psk) -> Result<(NoiseSession, NoiseSession)> {
        let mut init = builder(init_psk)?.build_initiator()?;
        let mut resp = builder(resp_psk)?.build_responder()?;
        let mut buf = vec![0u8; NOISE_MAX_MSG];
        let mut tmp = vec![0u8; NOISE_MAX_MSG];

        let n = init.write_message(&[], &mut buf)?;
        resp.read_message(&buf[..n], &mut tmp)?;
        let n = resp.write_message(&[], &mut buf)?;
        init.read_message(&buf[..n], &mut tmp)?;

        Ok((
            NoiseSession::new(init.into_transport_mode()?),
            NoiseSession::new(resp.into_transport_mode()?),
        ))
    }

    fn test_psk() -> Psk {
        Psk::from_raw([7u8; PSK_LEN])
    }

    #[test]
    fn passphrase_derivation_is_deterministic_and_label_separated() {
        let a = Psk::from_passphrase("correct horse battery staple", "alice:bob:2026").unwrap();
        let b = Psk::from_passphrase("correct horse battery staple", "alice:bob:2026").unwrap();
        assert_eq!(a.bytes(), b.bytes(), "same passphrase+label must derive the same PSK");

        let c = Psk::from_passphrase("correct horse battery staple", "alice:bob:OTHER").unwrap();
        assert_ne!(a.bytes(), c.bytes(), "different label must derive a different PSK");

        let d = Psk::from_passphrase("different passphrase", "alice:bob:2026").unwrap();
        assert_ne!(a.bytes(), d.bytes(), "different passphrase must derive a different PSK");
    }

    #[test]
    fn seal_open_round_trips_both_directions() {
        let psk = test_psk();
        let (mut init, mut resp) = handshake_in_memory(&psk, &psk).unwrap();

        let f1 = Frame::new(HELLO, b"hi".to_vec());
        let opened = resp.open_frame(&init.seal_frame(&f1).unwrap()).unwrap();
        assert_eq!(opened, f1);

        let f2 = Frame::new(AUDIO, vec![1, 2, 3, 4]);
        let opened = init.open_frame(&resp.seal_frame(&f2).unwrap()).unwrap();
        assert_eq!(opened, f2);
    }

    #[test]
    fn wrong_psk_fails_handshake() {
        let err = handshake_in_memory(&Psk::from_raw([1u8; PSK_LEN]), &Psk::from_raw([2u8; PSK_LEN]));
        assert!(err.is_err(), "mismatched PSKs must fail the handshake");
    }

    #[test]
    fn tampered_ciphertext_is_rejected() {
        let psk = test_psk();
        let (mut init, mut resp) = handshake_in_memory(&psk, &psk).unwrap();

        let mut msg = init.seal_frame(&Frame::new(AUDIO, vec![9; 32])).unwrap();
        let last = msg.len() - 1;
        msg[last] ^= 0x01; // flip a bit in the AEAD tag
        assert!(resp.open_frame(&msg).is_err(), "tampered ciphertext must fail AEAD");
    }

    #[test]
    fn replay_and_reorder_are_rejected() {
        // The Noise transport uses a strict sequential nonce: the receiver expects message
        // nonce N at step N. A failed decrypt does NOT advance the receiver nonce, so both
        // out-of-order delivery and replay are rejected (review S4).
        let psk = test_psk();
        let (mut init, mut resp) = handshake_in_memory(&psk, &psk).unwrap();

        let m1 = init.seal_frame(&Frame::new(AUDIO, vec![1])).unwrap(); // sender nonce 0
        let m2 = init.seal_frame(&Frame::new(AUDIO, vec![2])).unwrap(); // sender nonce 1

        // Reorder: m2 (nonce 1) arrives while the receiver still expects nonce 0 -> rejected.
        assert!(resp.open_frame(&m2).is_err(), "out-of-order message must be rejected");
        // In-order m1 (nonce 0) -> accepted; receiver nonce advances to 1.
        assert_eq!(resp.open_frame(&m1).unwrap(), Frame::new(AUDIO, vec![1]));
        // Replay m1: receiver now expects nonce 1, m1 used nonce 0 -> rejected.
        assert!(resp.open_frame(&m1).is_err(), "replayed message must be rejected");
    }

    #[test]
    fn empty_payload_frame_round_trips() {
        let psk = test_psk();
        let (mut init, mut resp) = handshake_in_memory(&psk, &psk).unwrap();
        let f = Frame::new(0x02, Vec::new()); // e.g. PttStart: a 3-byte plaintext, smallest real msg
        let opened = resp.open_frame(&init.seal_frame(&f).unwrap()).unwrap();
        assert_eq!(opened, f);
    }

    #[test]
    fn frame_too_large_boundary() {
        use shroud_proto::HEADER_LEN;
        let psk = test_psk();
        let (mut init, _resp) = handshake_in_memory(&psk, &psk).unwrap();

        // Encoded size == NOISE_MAX_PLAINTEXT (65519) must seal; one byte more must be rejected
        // *before* snow, with FrameTooLarge.
        let ok_payload = NOISE_MAX_PLAINTEXT - HEADER_LEN; // 65516
        assert!(init.seal_frame(&Frame::new(0x04, vec![0u8; ok_payload])).is_ok());

        let big = Frame::new(0x04, vec![0u8; ok_payload + 1]);
        match init.seal_frame(&big) {
            Err(TransportError::FrameTooLarge(n)) => assert_eq!(n, NOISE_MAX_PLAINTEXT + 1),
            other => panic!("expected FrameTooLarge, got {other:?}"),
        }
    }

    #[test]
    fn short_message_fails_cleanly() {
        let psk = test_psk();
        let (_init, mut resp) = handshake_in_memory(&psk, &psk).unwrap();
        // A message shorter than the AEAD tag must error, not panic.
        assert!(resp.open_frame(&[0u8; 4]).is_err());
        assert!(resp.open_frame(&[]).is_err());
    }

    #[tokio::test]
    async fn async_transport_round_trip_over_duplex() {
        // Full async path: handshake + length-prefixed framing + proto, over an in-memory duplex.
        let (a, b) = tokio::io::duplex(64 * 1024);

        let init = tokio::spawn(async move {
            let psk = Psk::from_passphrase("shared secret", "session-42").unwrap();
            let mut t = handshake_initiator(a, &psk).await.unwrap();
            t.send_frame(&Frame::new(HELLO, b"hello".to_vec())).await.unwrap();
            let reply = t.recv_frame().await.unwrap();
            assert_eq!(reply, Frame::new(AUDIO, vec![42, 42]));
        });

        let resp = tokio::spawn(async move {
            let psk = Psk::from_passphrase("shared secret", "session-42").unwrap();
            let mut t = handshake_responder(b, &psk).await.unwrap();
            let got = t.recv_frame().await.unwrap();
            assert_eq!(got, Frame::new(HELLO, b"hello".to_vec()));
            t.send_frame(&Frame::new(AUDIO, vec![42, 42])).await.unwrap();
        });

        init.await.unwrap();
        resp.await.unwrap();
    }
}
