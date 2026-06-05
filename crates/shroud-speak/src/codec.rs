//! Opus codec wrapper for the voice path.
//!
//! Canonical internal audio format (matches the architecture doc): **PCM S16LE, mono, 8 kHz**,
//! Opus in VoIP/speech mode at ~16 kbps, **60 ms frames**. One frame in = one Opus packet out,
//! and one packet in = one frame out. The codec is the unit that the rest of the pipeline
//! (capture ring → encode → [Noise] → decode → playback ring) is built around.
//!
//! This module has no audio-hardware or network dependency, so it is fully unit-testable.

use anyhow::{ensure, Context, Result};
use audiopus::{
    coder::{Decoder, Encoder},
    Application, Bitrate, Channels, SampleRate,
};

/// Canonical sample rate.
pub const SAMPLE_RATE_HZ: u32 = 8_000;
/// Canonical frame duration.
pub const FRAME_MS: u32 = 60;
/// Samples in one frame: 8000 Hz × 60 ms = 480 (mono).
pub const SAMPLES_PER_FRAME: usize = (SAMPLE_RATE_HZ as usize * FRAME_MS as usize) / 1000;
/// Target encoder bitrate (speech).
pub const TARGET_BITRATE_BPS: i32 = 16_000;

/// Upper bound for an encoded Opus packet (libopus recommends up to 4000 bytes; ours are tiny).
const MAX_PACKET_BYTES: usize = 4_000;

/// Encodes 60 ms mono PCM frames into Opus packets.
pub struct VoiceEncoder {
    enc: Encoder,
}

impl VoiceEncoder {
    /// Build an encoder for the canonical format (8 kHz mono, VoIP mode, 16 kbps).
    pub fn new() -> Result<Self> {
        let mut enc = Encoder::new(SampleRate::Hz8000, Channels::Mono, Application::Voip)
            .context("create opus encoder")?;
        enc.set_bitrate(Bitrate::BitsPerSecond(TARGET_BITRATE_BPS))
            .context("set opus bitrate")?;
        Ok(Self { enc })
    }

    /// Encode exactly one [`SAMPLES_PER_FRAME`]-sample frame into an Opus packet.
    pub fn encode_frame(&mut self, pcm: &[i16]) -> Result<Vec<u8>> {
        ensure!(
            pcm.len() == SAMPLES_PER_FRAME,
            "encode expects exactly {SAMPLES_PER_FRAME} samples, got {}",
            pcm.len()
        );
        let mut packet = vec![0u8; MAX_PACKET_BYTES];
        let n = self
            .enc
            .encode(pcm, &mut packet)
            .context("opus encode")?;
        packet.truncate(n);
        Ok(packet)
    }
}

/// Decodes Opus packets back into 60 ms mono PCM frames.
pub struct VoiceDecoder {
    dec: Decoder,
}

impl VoiceDecoder {
    /// Build a decoder for the canonical format (8 kHz mono).
    pub fn new() -> Result<Self> {
        let dec = Decoder::new(SampleRate::Hz8000, Channels::Mono).context("create opus decoder")?;
        Ok(Self { dec })
    }

    /// Decode one Opus packet into a frame of [`SAMPLES_PER_FRAME`] samples.
    pub fn decode_frame(&mut self, packet: &[u8]) -> Result<Vec<i16>> {
        let mut pcm = vec![0i16; SAMPLES_PER_FRAME];
        let n = self
            .dec
            .decode(Some(packet), &mut pcm, false)
            .context("opus decode")?;
        pcm.truncate(n);
        Ok(pcm)
    }

    /// Conceal one lost frame (Opus packet-loss concealment) — emits [`SAMPLES_PER_FRAME`]
    /// samples of best-effort fill. Useful when a frame is dropped over the high-latency Tor
    /// circuit rather than stalling playback.
    pub fn decode_lost(&mut self) -> Result<Vec<i16>> {
        let mut pcm = vec![0i16; SAMPLES_PER_FRAME];
        let n = self
            .dec
            .decode(None::<&[u8]>, &mut pcm, false)
            .context("opus PLC decode")?;
        pcm.truncate(n);
        Ok(pcm)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Generate one frame of a sine wave at `freq` Hz, amplitude `amp` (0..=1).
    fn sine_frame(freq: f32, amp: f32, phase: &mut f32) -> Vec<i16> {
        let dt = 1.0 / SAMPLE_RATE_HZ as f32;
        (0..SAMPLES_PER_FRAME)
            .map(|_| {
                let s = (*phase * std::f32::consts::TAU).sin() * amp;
                *phase = (*phase + freq * dt).fract();
                (s * i16::MAX as f32) as i16
            })
            .collect()
    }

    fn rms(frame: &[i16]) -> f64 {
        let sumsq: f64 = frame.iter().map(|&s| (s as f64).powi(2)).sum();
        (sumsq / frame.len() as f64).sqrt()
    }

    #[test]
    fn frame_size_is_480() {
        assert_eq!(SAMPLES_PER_FRAME, 480);
    }

    #[test]
    fn encode_rejects_wrong_frame_size() {
        let mut enc = VoiceEncoder::new().unwrap();
        assert!(enc.encode_frame(&vec![0i16; SAMPLES_PER_FRAME - 1]).is_err());
        assert!(enc.encode_frame(&[]).is_err());
    }

    #[test]
    fn encode_decode_round_trip_preserves_a_tone() {
        // Drive a 300 Hz tone through encode→decode for several frames. Opus is lossy and has
        // warmup/delay, so we don't assert sample equality — we assert the codec compresses,
        // produces full-length frames, and after warmup reproduces comparable signal energy
        // (i.e. it's real audio, not silence or garbage).
        let mut enc = VoiceEncoder::new().unwrap();
        let mut dec = VoiceDecoder::new().unwrap();
        let mut phase = 0.0f32;

        let mut checked_steady_state = false;
        for i in 0..20 {
            let pcm = sine_frame(300.0, 0.5, &mut phase);
            let packet = enc.encode_frame(&pcm).unwrap();
            assert!(!packet.is_empty(), "packet must be non-empty");
            assert!(
                packet.len() < pcm.len() * 2,
                "packet ({}) should be smaller than raw PCM ({} bytes)",
                packet.len(),
                pcm.len() * 2
            );

            let out = dec.decode_frame(&packet).unwrap();
            assert_eq!(out.len(), SAMPLES_PER_FRAME, "decoder must emit a full frame");

            // After warmup, decoded energy should be in the same ballpark as the input.
            if i >= 8 {
                let ratio = rms(&out) / rms(&pcm);
                assert!(
                    (0.25..=4.0).contains(&ratio),
                    "frame {i}: decoded/input RMS ratio {ratio:.3} out of range (signal lost?)"
                );
                checked_steady_state = true;
            }
        }
        assert!(checked_steady_state);
    }

    #[test]
    fn packet_loss_concealment_emits_a_full_frame() {
        let mut enc = VoiceEncoder::new().unwrap();
        let mut dec = VoiceDecoder::new().unwrap();
        let mut phase = 0.0f32;
        // Prime the decoder so PLC has history to work from.
        for _ in 0..4 {
            let pcm = sine_frame(440.0, 0.5, &mut phase);
            dec.decode_frame(&enc.encode_frame(&pcm).unwrap()).unwrap();
        }
        let concealed = dec.decode_lost().unwrap();
        assert_eq!(concealed.len(), SAMPLES_PER_FRAME);
    }
}
