// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Random bytes generated from firmware entropy.

use abi::Error;
use chacha20::{
    ChaCha20,
    cipher::{KeyIvInit, StreamCipher},
};
use spin::Mutex;

static GENERATOR: Mutex<Option<ChaCha20>> = Mutex::new(None);

/// Initialize once before starting user programs. Missing firmware entropy leaves
/// random reads unavailable so callers can use a device entropy source.
pub fn initialize(seed: Option<&crate::boot::RandomSeed>) {
    if let Some(seed) = seed {
        let mut generator = GENERATOR.lock();
        assert!(generator.is_none(), "random generator already initialized");
        *generator = Some(ChaCha20::new((&seed.0).into(), (&[0; 12]).into()));
    }
}

pub(crate) fn fill(output: &mut [u8]) -> Result<(), Error> {
    let _interrupts = crate::hardware::cpu::InterruptGuard::new();
    let mut generator = GENERATOR.lock();
    let generator = generator.as_mut().ok_or(Error::InvalidState)?;
    output.fill(0);
    generator.apply_keystream(output);
    // Rekey after each request to protect earlier output if state is exposed
    // and keep each stream below ChaCha20's counter limit.
    let mut key = [0; 32];
    generator.apply_keystream(&mut key);
    *generator = ChaCha20::new((&key).into(), (&[0; 12]).into());
    Ok(())
}
