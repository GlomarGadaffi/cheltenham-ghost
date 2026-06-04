//! shroud-proto: the generic frame envelope for the shroud substrate.
//!
//! Wire format (see [`PROTOCOL.md`](../../../PROTOCOL.md)):
//!
//! ```text
//!   ┌────────┬──────────┬─────────────────────┐
//!   │ type:1 │ len:2 BE │ payload: len bytes   │
//!   └────────┴──────────┴─────────────────────┘
//! ```
//!
//! This crate is deliberately **medium-agnostic**: it knows nothing about voice, PTT, or any
//! concrete frame type. The concrete `type` byte values (`Audio`, `PttStart`, …) live in the
//! capability crate (e.g. `shroud-speak`), not here — keeping the substrate reusable by a
//! future `shroud-text` / `shroud-drop`. Per `PROTOCOL.md`, unknown `type` values MUST be
//! ignored by the *application*, not treated as fatal; decoding here is therefore
//! type-agnostic and never rejects a frame because of its type byte.
//!
//! The crate performs no I/O. A stream reader is expected to length-delimit reads itself
//! (read the 3-byte header, then exactly `len` payload bytes) rather than treating one
//! `read()` as one frame.

#![forbid(unsafe_code)]

use core::fmt;

/// Bytes in a frame header: `type` (1) + `len` (2, big-endian).
pub const HEADER_LEN: usize = 3;

/// Maximum payload length, fixed by the 16-bit `len` field (`u16::MAX`).
pub const MAX_PAYLOAD_LEN: usize = u16::MAX as usize;

/// A single application frame: a `type` byte plus an opaque payload.
///
/// `frame_type` is intentionally a raw [`u8`]; interpreting it — and ignoring unknown
/// values per the protocol's forward-compatibility rule — is the caller's responsibility.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// Application frame type. Opaque to this crate.
    pub frame_type: u8,
    /// Frame payload; at most [`MAX_PAYLOAD_LEN`] bytes.
    pub payload: Vec<u8>,
}

/// Errors produced while encoding or decoding a [`Frame`].
///
/// Dependency-free on purpose: a foundational, no-I/O crate should expose a typed error a
/// caller can match on (e.g. "need more bytes" vs "malformed"), without dragging in an
/// application error type such as `anyhow`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameError {
    /// Payload is longer than the 16-bit `len` field can describe.
    PayloadTooLarge(usize),
    /// Input ended before a full header (`type` + `len`) was available.
    ShortHeader(usize),
    /// The header declared more payload bytes than are present in the buffer.
    ///
    /// Returned *before* any payload allocation, so a truncated or hostile header cannot
    /// trigger an over-allocation. A stream reader can treat this as "read more, retry".
    ShortPayload {
        /// Payload length the header claims.
        declared: usize,
        /// Payload bytes actually available after the header.
        available: usize,
    },
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FrameError::PayloadTooLarge(n) => {
                write!(f, "payload too large: {n} bytes exceeds maximum of 65535")
            }
            FrameError::ShortHeader(have) => {
                write!(f, "buffer too short for header: need 3, have {have}")
            }
            FrameError::ShortPayload {
                declared,
                available,
            } => write!(
                f,
                "buffer too short for payload: header declares {declared}, have {available}"
            ),
        }
    }
}

impl std::error::Error for FrameError {}

impl Frame {
    /// Construct a frame from a type byte and payload.
    ///
    /// No validation is performed here; over-length payloads are rejected at [`Frame::encode`]
    /// time so that constructing a value is always infallible.
    pub fn new(frame_type: u8, payload: Vec<u8>) -> Self {
        Self {
            frame_type,
            payload,
        }
    }

    /// Serialized length of this frame on the wire (header + payload).
    pub fn encoded_len(&self) -> usize {
        HEADER_LEN + self.payload.len()
    }

    /// Encode the frame into a freshly allocated buffer.
    pub fn encode(&self) -> Result<Vec<u8>, FrameError> {
        let mut out = Vec::with_capacity(self.encoded_len());
        self.encode_into(&mut out)?;
        Ok(out)
    }

    /// Encode the frame by appending to an existing buffer (useful for batching frames into
    /// one Noise transport message). On error the buffer is left unmodified.
    pub fn encode_into(&self, out: &mut Vec<u8>) -> Result<(), FrameError> {
        let len = self.payload.len();
        if len > MAX_PAYLOAD_LEN {
            return Err(FrameError::PayloadTooLarge(len));
        }
        out.reserve(self.encoded_len());
        out.push(self.frame_type);
        // `len <= u16::MAX` checked above, so the cast is lossless.
        out.extend_from_slice(&(len as u16).to_be_bytes());
        out.extend_from_slice(&self.payload);
        Ok(())
    }

    /// Decode exactly one frame from the front of `input`, returning the frame and the number
    /// of bytes consumed.
    ///
    /// `input` may contain trailing bytes (e.g. the start of the next frame); those are not
    /// consumed, so a stream reader can loop: decode, advance by `consumed`, repeat. The `len`
    /// field is validated against the bytes actually available **before** any payload
    /// allocation. [`FrameError::ShortHeader`] / [`FrameError::ShortPayload`] mean "need more
    /// bytes", not "malformed".
    pub fn decode(input: &[u8]) -> Result<(Frame, usize), FrameError> {
        if input.len() < HEADER_LEN {
            return Err(FrameError::ShortHeader(input.len()));
        }
        let frame_type = input[0];
        let declared = u16::from_be_bytes([input[1], input[2]]) as usize;
        let available = input.len() - HEADER_LEN;
        if available < declared {
            return Err(FrameError::ShortPayload {
                declared,
                available,
            });
        }
        let end = HEADER_LEN + declared;
        let payload = input[HEADER_LEN..end].to_vec();
        Ok((Frame::new(frame_type, payload), end))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_typical() {
        let frame = Frame::new(0x04, vec![1, 2, 3, 4, 5]);
        let bytes = frame.encode().expect("encode");
        // type(1) + len(2) + payload(5)
        assert_eq!(bytes, vec![0x04, 0x00, 0x05, 1, 2, 3, 4, 5]);
        let (decoded, consumed) = Frame::decode(&bytes).expect("decode");
        assert_eq!(decoded, frame);
        assert_eq!(consumed, bytes.len());
    }

    #[test]
    fn len_is_big_endian() {
        // 0x0102 = 258-byte payload; high byte must come first.
        let frame = Frame::new(0x10, vec![0xAB; 258]);
        let bytes = frame.encode().expect("encode");
        assert_eq!(&bytes[0..3], &[0x10, 0x01, 0x02]);
        let (decoded, consumed) = Frame::decode(&bytes).expect("decode");
        assert_eq!(decoded, frame);
        assert_eq!(consumed, HEADER_LEN + 258);
    }

    #[test]
    fn empty_payload_round_trips() {
        let frame = Frame::new(0x02, Vec::new()); // e.g. PttStart: a type with no payload
        let bytes = frame.encode().expect("encode");
        assert_eq!(bytes, vec![0x02, 0x00, 0x00]);
        let (decoded, consumed) = Frame::decode(&bytes).expect("decode");
        assert_eq!(decoded, frame);
        assert_eq!(consumed, HEADER_LEN);
    }

    #[test]
    fn max_payload_round_trips() {
        let frame = Frame::new(0xFF, vec![0x7E; MAX_PAYLOAD_LEN]);
        assert_eq!(frame.encoded_len(), HEADER_LEN + MAX_PAYLOAD_LEN);
        let bytes = frame.encode().expect("encode");
        assert_eq!(&bytes[0..3], &[0xFF, 0xFF, 0xFF]);
        let (decoded, consumed) = Frame::decode(&bytes).expect("decode");
        assert_eq!(decoded, frame);
        assert_eq!(consumed, bytes.len());
    }

    #[test]
    fn oversize_payload_is_rejected() {
        let frame = Frame::new(0x01, vec![0u8; MAX_PAYLOAD_LEN + 1]);
        assert_eq!(
            frame.encode().unwrap_err(),
            FrameError::PayloadTooLarge(MAX_PAYLOAD_LEN + 1)
        );
    }

    #[test]
    fn encode_into_leaves_buffer_unmodified_on_error() {
        let mut buf = vec![0xDE, 0xAD];
        let frame = Frame::new(0x01, vec![0u8; MAX_PAYLOAD_LEN + 1]);
        assert!(frame.encode_into(&mut buf).is_err());
        assert_eq!(buf, vec![0xDE, 0xAD], "buffer must be untouched on error");
    }

    #[test]
    fn short_header_reports_bytes_available() {
        assert_eq!(Frame::decode(&[]).unwrap_err(), FrameError::ShortHeader(0));
        assert_eq!(
            Frame::decode(&[0x04, 0x00]).unwrap_err(),
            FrameError::ShortHeader(2)
        );
    }

    #[test]
    fn short_payload_does_not_allocate_and_reports_gap() {
        // Header claims 10 bytes, only 3 present.
        let input = [0x04, 0x00, 0x0A, 1, 2, 3];
        assert_eq!(
            Frame::decode(&input).unwrap_err(),
            FrameError::ShortPayload {
                declared: 10,
                available: 3,
            }
        );
    }

    #[test]
    fn trailing_bytes_are_not_consumed() {
        let frame = Frame::new(0x05, vec![9, 9]);
        let mut bytes = frame.encode().expect("encode");
        bytes.extend_from_slice(&[0xFF, 0xFF, 0xFF]); // start of a "next" frame
        let (decoded, consumed) = Frame::decode(&bytes).expect("decode");
        assert_eq!(decoded, frame);
        assert_eq!(consumed, HEADER_LEN + 2);
        assert_eq!(&bytes[consumed..], &[0xFF, 0xFF, 0xFF]);
    }

    #[test]
    fn streaming_decode_of_multiple_frames() {
        let frames = [
            Frame::new(0x01, vec![]),
            Frame::new(0x04, vec![10, 20, 30]),
            Frame::new(0x08, b"hi".to_vec()),
        ];
        let mut stream = Vec::new();
        for f in &frames {
            f.encode_into(&mut stream).expect("encode");
        }

        let mut out = Vec::new();
        let mut rest = stream.as_slice();
        while !rest.is_empty() {
            let (frame, consumed) = Frame::decode(rest).expect("decode");
            out.push(frame);
            rest = &rest[consumed..];
        }
        assert_eq!(out, frames);
    }

    #[test]
    fn unknown_type_byte_is_preserved_not_rejected() {
        // 0x7F is not in PROTOCOL.md's table; the envelope must still round-trip it,
        // leaving the "ignore unknown types" decision to the application layer.
        let frame = Frame::new(0x7F, vec![1, 2, 3]);
        let bytes = frame.encode().expect("encode");
        let (decoded, _) = Frame::decode(&bytes).expect("decode");
        assert_eq!(decoded.frame_type, 0x7F);
        assert_eq!(decoded.payload, vec![1, 2, 3]);
    }
}
