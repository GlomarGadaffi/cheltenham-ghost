//! shroud-core: substrate containing the Arti onion client and Noise handshake.
//!
//! M2 secure transport lives in [`transport`]; the arti onion client is still scaffolding
//! (see `examples/m0_spike.rs` for the proven M0 onion path).

pub mod transport;

pub use transport::{NoiseSession, NoiseTransport, Psk, TransportError, PSK_LEN};

#[derive(Default)]
pub struct ShroudClient;

impl ShroudClient {
    pub fn new() -> Self {
        Self
    }
}
