//! Short random ids. The generator is a xorshift seeded by the host
//! (wasm has no clock in `std`), so ids are unique, not secret.

use std::sync::atomic::{AtomicU64, Ordering};

static STATE: AtomicU64 = AtomicU64::new(0x9E37_79B9_7F4A_7C15);

/// Mixes host entropy (time, `Math.random`, ...) into the generator.
pub fn seed(entropy: u64) {
    STATE.fetch_xor(entropy | 1, Ordering::Relaxed);
}

fn next() -> u64 {
    let mut x = STATE.load(Ordering::Relaxed);
    loop {
        let mut y = x;
        y ^= y << 13;
        y ^= y >> 7;
        y ^= y << 17;
        match STATE.compare_exchange_weak(x, y, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return y,
            Err(cur) => x = cur,
        }
    }
}

pub fn new_id() -> String {
    const ALPHABET: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut n = next();
    (0..12)
        .map(|_| {
            let c = ALPHABET[(n % 36) as usize] as char;
            n /= 36;
            c
        })
        .collect()
}
