//! shroud-speak: the voice capability (the "speak" in shroud-speak).
//!
//! The audio pipeline and voice front-end live here; the medium-agnostic onion + Noise spine
//! is in `shroud-core`. M1 builds the audio path (capture → encode → decode → playback) in
//! memory; M3 wires it onto the secure transport.

pub mod audio;
pub mod codec;
