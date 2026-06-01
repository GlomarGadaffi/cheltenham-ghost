//! shroud-proto: generic frame envelope serialization/deserialization.
//!
//! (Placeholder stubs, will be fully implemented and unit-tested in Milestone M2)

pub struct Frame {
    pub frame_type: u8,
    pub payload: Vec<u8>,
}

impl Frame {
    pub fn new(frame_type: u8, payload: Vec<u8>) -> Self {
        Self { frame_type, payload }
    }
}
