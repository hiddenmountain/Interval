//! Transformation pipeline.
//!
//! Implements all transforms from spec section 9:
//! - Deterministic: reverse, invert, retrograde, rotate, stretch, compress,
//!   transpose, shift_oct, subset, interleave, mirror
//! - Seeded: humanize, vary
//!
//! Each transform is a pure function that takes a pattern AST and returns
//! a new pattern AST. Transforms are composed via the pipe operator `|`.
//!
//! The seeded PRNG is xorshift64, implemented directly (not via `rand`)
//! to guarantee output stability across dependency versions.

/// xorshift64 PRNG — deterministic pseudorandom number generator.
///
/// Given a mutable state, produces the next u64 in the sequence.
/// State is updated in place. State must never be zero — the caller
/// should map seed=0 to a nonzero default before first use.
pub fn xorshift64(state: &mut u64) -> u64 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    x
}

/// Initialize PRNG state from a seed value.
/// Maps 0 to a nonzero default to avoid the xorshift64 fixed point.
pub fn seed_state(seed: u64) -> u64 {
    if seed == 0 {
        0x517c_c1b7_2722_0a95
    } else {
        seed
    }
}

/// Generate a uniform f64 in [0.0, 1.0) from the PRNG state.
pub fn xorshift64_f64(state: &mut u64) -> f64 {
    let v = xorshift64(state);
    (v >> 11) as f64 / (1u64 << 53) as f64
}

/// Generate a uniform f64 in [-1.0, 1.0) from the PRNG state.
pub fn xorshift64_symmetric(state: &mut u64) -> f64 {
    xorshift64_f64(state) * 2.0 - 1.0
}

/// Derive a per-track seed from the global seed and the track index.
///
/// FNV-1a over the little-endian bytes of the global seed followed by the
/// little-endian bytes of the track index. Implemented directly (no external
/// crate) for the same version-stability reasons as xorshift64: the derived
/// sequence must never change across dependency versions.
pub fn fnv1a_derive(seed: u64, index: usize) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325; // FNV offset basis
    for byte in seed.to_le_bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3); // FNV prime
    }
    for byte in (index as u64).to_le_bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_xorshift64_deterministic() {
        let mut s1 = seed_state(42);
        let mut s2 = seed_state(42);

        let seq1: Vec<u64> = (0..10).map(|_| xorshift64(&mut s1)).collect();
        let seq2: Vec<u64> = (0..10).map(|_| xorshift64(&mut s2)).collect();
        assert_eq!(seq1, seq2);
    }

    #[test]
    fn test_xorshift64_different_seeds() {
        let mut s1 = seed_state(1);
        let mut s2 = seed_state(2);

        let v1 = xorshift64(&mut s1);
        let v2 = xorshift64(&mut s2);
        assert_ne!(v1, v2);
    }

    #[test]
    fn test_seed_zero_mapped() {
        let state = seed_state(0);
        assert_ne!(state, 0);
    }

    #[test]
    fn test_xorshift64_nonzero() {
        let mut state = seed_state(42);
        for _ in 0..1000 {
            let v = xorshift64(&mut state);
            assert_ne!(v, 0, "xorshift64 should not produce zero");
        }
    }

    #[test]
    fn test_xorshift64_f64_range() {
        let mut state = seed_state(42);
        for _ in 0..100 {
            let v = xorshift64_f64(&mut state);
            assert!((0.0..1.0).contains(&v), "f64 out of range: {v}");
        }
    }

    #[test]
    fn test_xorshift64_symmetric_range() {
        let mut state = seed_state(42);
        for _ in 0..100 {
            let v = xorshift64_symmetric(&mut state);
            assert!((-1.0..1.0).contains(&v), "symmetric out of range: {v}");
        }
    }

    #[test]
    fn test_fnv1a_derive_reference_values() {
        // Expected values computed independently (Python) from the exact
        // FNV-1a algorithm specified in spec §11.1: offset basis
        // 0xcbf29ce484222325, prime 0x100000001b3, little-endian bytes of
        // seed then index.
        assert_eq!(fnv1a_derive(0, 0), 0x8820_1fb9_60ff_6465);
        assert_eq!(fnv1a_derive(1, 0), 0x3922_09f1_4dea_4c24);
        assert_eq!(fnv1a_derive(1, 1), 0x581c_d0fa_58d9_9645);
        assert_eq!(fnv1a_derive(42, 3), 0x6159_eb6c_9c61_706c);
        assert_eq!(fnv1a_derive(0xDEAD_BEEF, 7), 0xf34e_d8dc_b1eb_83dc);
    }

    #[test]
    fn test_fnv1a_derive_distinct_per_index() {
        // Different track indices must derive different seeds — this is the
        // whole point of per-track derivation (tracks must not mutate in
        // lockstep).
        let seeds: Vec<u64> = (0..8).map(|i| fnv1a_derive(42, i)).collect();
        for i in 0..seeds.len() {
            for j in (i + 1)..seeds.len() {
                assert_ne!(seeds[i], seeds[j], "indices {i} and {j} collide");
            }
        }
    }
}
