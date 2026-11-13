// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Seeded randomness and a keyed TCP initial sequence construction.

use core::hash::Hasher;
use rand_chacha::ChaCha20Rng;
use rand_core::{RngCore, SeedableRng};
use runtime::{ipc::WaitMode, sync::Mutex, thread};
use siphasher::sip::SipHasher24;

static RANDOM: Mutex<Option<Random>> = Mutex::new(None);

struct Random {
    generator: ChaCha20Rng,
    isn_key: [u64; 2],
}

/// # Safety
///
/// seed must reference 32 initialized unpredictable bytes for this call. The
/// bridge calls this once, before starting lwIP or exposing any socket.
#[unsafe(no_mangle)]
unsafe extern "C" fn arona_lwip_seed(seed: *const u8) {
    // SAFETY: The bootstrap caller provides the complete seed for this call.
    let mut generator = ChaCha20Rng::from_seed(unsafe { seed.cast::<[u8; 32]>().read() });
    let isn_key = [generator.next_u64(), generator.next_u64()];
    let mut state = RANDOM.lock(WaitMode::Forever).unwrap();
    assert!(state.is_none());
    *state = Some(Random { generator, isn_key });
}

#[unsafe(no_mangle)]
extern "C" fn arona_lwip_random() -> u32 {
    RANDOM
        .lock(WaitMode::Forever)
        .unwrap()
        .as_mut()
        .expect("unseeded lwIP RNG")
        .generator
        .next_u32()
}

/// # Safety
///
/// local and remote must each reference length readable address bytes, where
/// length is 4 for IPv4 or 16 for IPv6. Both addresses have the same family. The
/// RNG must already be seeded and the call must originate in the protocol thread.
#[unsafe(no_mangle)]
unsafe extern "C" fn arona_lwip_isn(
    local: *const u8,
    local_port: u16,
    remote: *const u8,
    remote_port: u16,
    length: usize,
) -> u32 {
    let [first, second] = RANDOM
        .lock(WaitMode::Forever)
        .unwrap()
        .as_ref()
        .expect("unseeded lwIP ISN")
        .isn_key;
    let mut hash = SipHasher24::new_with_keys(first, second);
    hash.write_u8(length as u8);
    // SAFETY: The protocol call provides complete addresses of the stated length.
    unsafe {
        hash.write(core::slice::from_raw_parts(local, length));
        hash.write(core::slice::from_raw_parts(remote, length));
    }
    hash.write(&local_port.to_be_bytes());
    hash.write(&remote_port.to_be_bytes());
    // REFERENCE: https://www.rfc-editor.org/rfc/rfc6528#section-3
    // The keyed tuple hash is combined with a clock advancing every four microseconds.
    (hash.finish() as u32).wrapping_add((thread::now_ns().unwrap() / 4_000) as u32)
}
