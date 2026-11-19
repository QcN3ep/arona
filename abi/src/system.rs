// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Global scheduler measurements without scheduling policy.

/// Integral of running and ready non-idle threads, in thread nanoseconds.
/// Reads preserve the accumulated value, which saturates at u128::MAX.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RunnableIntegral {
    pub sample_ns: u64,
    pub integral: u128,
    pub runnable_count: u64,
}

impl RunnableIntegral {
    pub const SIZE: usize = 32;
    pub fn encode(self) -> [u8; Self::SIZE] {
        let mut bytes = [0; Self::SIZE];
        for (output, value) in bytes.as_chunks_mut::<8>().0.iter_mut().zip([
            self.sample_ns,
            self.integral as u64,
            (self.integral >> 64) as u64,
            self.runnable_count,
        ]) {
            *output = value.to_le_bytes();
        }
        bytes
    }
    pub fn decode(bytes: &[u8; Self::SIZE]) -> Self {
        let words = bytes.as_chunks::<8>().0;
        Self {
            sample_ns: u64::from_le_bytes(words[0]),
            integral: u128::from(u64::from_le_bytes(words[1]))
                | (u128::from(u64::from_le_bytes(words[2])) << 64),
            runnable_count: u64::from_le_bytes(words[3]),
        }
    }
}
