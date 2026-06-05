//! M1 audio pipeline pieces: device discovery, sample-rate conversion, and a hardware-free
//! self-test that drives synthetic audio through the *real* codec and measures latency.
//!
//! The live capture→playback loop (which needs a mic, speakers, and a human to verify) lives
//! in `examples/m1_loopback.rs`; the reusable, testable bits live here.

use crate::codec::{VoiceDecoder, VoiceEncoder, FRAME_MS, SAMPLES_PER_FRAME, SAMPLE_RATE_HZ};
use anyhow::Result;
use std::time::Instant;

/// List available cpal input/output devices (best-effort; empty if no host/devices).
pub fn list_devices() -> Vec<String> {
    use cpal::traits::{DeviceTrait, HostTrait};
    let host = cpal::default_host();
    let mut out = Vec::new();
    if let Ok(inputs) = host.input_devices() {
        for d in inputs {
            if let Ok(n) = d.name() {
                out.push(format!("input:  {n}"));
            }
        }
    }
    if let Ok(outputs) = host.output_devices() {
        for d in outputs {
            if let Ok(n) = d.name() {
                out.push(format!("output: {n}"));
            }
        }
    }
    out
}

/// Minimal linear resampler for mono `i16`, used to bridge a device's native rate to the
/// codec's canonical 8 kHz and back. Not hi-fi — adequate for speech and for the demo. A
/// production build should use a proper resampler (the architecture's A7 note).
pub fn resample_linear(input: &[i16], from_hz: u32, to_hz: u32) -> Vec<i16> {
    if from_hz == to_hz || input.len() < 2 {
        return input.to_vec();
    }
    let ratio = to_hz as f64 / from_hz as f64;
    let out_len = ((input.len() as f64) * ratio).round().max(1.0) as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let src = i as f64 / ratio;
        let i0 = src.floor() as usize;
        let i1 = (i0 + 1).min(input.len() - 1);
        let frac = src - i0 as f64;
        let s = input[i0] as f64 * (1.0 - frac) + input[i1] as f64 * frac;
        out.push(s.round().clamp(i16::MIN as f64, i16::MAX as f64) as i16);
    }
    out
}

/// Result of [`run_self_test`].
#[derive(Debug, Clone)]
pub struct SelfTestReport {
    pub frames: usize,
    pub total_packet_bytes: usize,
    pub avg_packet_bytes: f64,
    pub avg_encode_us: f64,
    pub avg_decode_us: f64,
    /// Wall-clock seconds of audio represented by `frames` (frames × 60 ms).
    pub audio_secs: f64,
    /// audio_secs ÷ processing_secs. > 1 means the codec runs faster than real time.
    pub realtime_factor: f64,
}

impl std::fmt::Display for SelfTestReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "frames={} audio={:.2}s | packet avg={:.1}B (~{:.1} kbps) | encode={:.1}\u{b5}s decode={:.1}\u{b5}s/frame | realtime\u{d7}{:.0}",
            self.frames,
            self.audio_secs,
            self.avg_packet_bytes,
            self.avg_packet_bytes * 8.0 / (FRAME_MS as f64 / 1000.0) / 1000.0,
            self.avg_encode_us,
            self.avg_decode_us,
            self.realtime_factor,
        )
    }
}

/// Drive `frames` of synthetic speech-like audio through encode→decode and measure throughput.
/// Hardware-free: proves the codec + frame cadence work and that encode/decode beat real time.
pub fn run_self_test(frames: usize) -> Result<SelfTestReport> {
    let mut enc = VoiceEncoder::new()?;
    let mut dec = VoiceDecoder::new()?;
    let mut phase = 0.0f32;
    let mut total_packet = 0usize;
    let mut enc_us = 0u128;
    let mut dec_us = 0u128;

    for _ in 0..frames {
        let pcm: Vec<i16> = (0..SAMPLES_PER_FRAME)
            .map(|_| {
                let s = (phase * std::f32::consts::TAU).sin() * 0.5;
                phase = (phase + 350.0 / SAMPLE_RATE_HZ as f32).fract();
                (s * i16::MAX as f32) as i16
            })
            .collect();

        let t = Instant::now();
        let packet = enc.encode_frame(&pcm)?;
        enc_us += t.elapsed().as_micros();
        total_packet += packet.len();

        let t = Instant::now();
        let _out = dec.decode_frame(&packet)?;
        dec_us += t.elapsed().as_micros();
    }

    let audio_secs = frames as f64 * FRAME_MS as f64 / 1000.0;
    let proc_secs = (enc_us + dec_us) as f64 / 1_000_000.0;
    Ok(SelfTestReport {
        frames,
        total_packet_bytes: total_packet,
        avg_packet_bytes: total_packet as f64 / frames as f64,
        avg_encode_us: enc_us as f64 / frames as f64,
        avg_decode_us: dec_us as f64 / frames as f64,
        audio_secs,
        realtime_factor: if proc_secs > 0.0 {
            audio_secs / proc_secs
        } else {
            f64::INFINITY
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resample_upsample_and_downsample_lengths() {
        let frame = vec![0i16; SAMPLES_PER_FRAME]; // 480 @ 8k
        // 8k -> 48k should ~6x the samples.
        let up = resample_linear(&frame, 8_000, 48_000);
        assert!((up.len() as i64 - 2880).abs() <= 1, "got {}", up.len());
        // 48k -> 8k should ~1/6 the samples.
        let down = resample_linear(&up, 48_000, 8_000);
        assert!((down.len() as i64 - 480).abs() <= 1, "got {}", down.len());
        // Same rate is a passthrough.
        assert_eq!(resample_linear(&frame, 8_000, 8_000).len(), frame.len());
    }

    #[test]
    fn resample_preserves_a_constant_signal() {
        let flat = vec![1000i16; 100];
        let out = resample_linear(&flat, 16_000, 8_000);
        assert!(out.iter().all(|&s| (s - 1000).abs() <= 1), "constant signal must survive");
    }

    #[test]
    fn self_test_runs_and_beats_real_time() {
        let r = run_self_test(50).unwrap();
        assert_eq!(r.frames, 50);
        assert!(r.avg_packet_bytes > 0.0, "packets must be non-empty");
        assert!(
            r.avg_packet_bytes < (SAMPLES_PER_FRAME * 2) as f64,
            "opus must compress vs raw PCM"
        );
        assert!(
            r.realtime_factor > 1.0,
            "encode+decode must run faster than real time, got x{:.1}",
            r.realtime_factor
        );
    }
}
