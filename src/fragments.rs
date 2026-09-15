//! # Fragment-Based Cache System
//!
//! This module implements the core fragment extraction logic for rawgrep's nowgrep-inspired
//! caching system. Fragments are small byte sequences used to quickly determine if a file
//! can be skipped without reading it.
//!
//! ## What Are Fragments?
//!
//! A **fragment** is a small (3- or 4-byte) sliding window extracted from text. For example,
//! with a 4-byte window:
//!
//! ```text
//! Pattern: "ERROR:"
//!
//! Windows: "ERRO"   [E, R, R, O] -> hash -> 0x12345678
//!           "RROR"  [R, R, O, R] -> hash -> 0x23456789
//!            "ROR:" [R, O, R, :] -> hash -> 0x34567890
//!
//! Result: 3 fragment hashes
//! ```
//!
//! Even different patterns benefit from previous searches if they share fragments.
//!
//! ## Fragment size
//!
//! The fragment is 4 bytes whenever the pattern (or, for alternations/regex literals, the
//! *shortest* literal involved) is at least 4 bytes long. Shorter patterns fall back to a
//! 3-byte fragment so they can still use the cache at all -- previously any pattern under
//! 4 bytes (e.g. `"foo"`, or a single multi-byte UTF-8 character like an em dash, which is
//! 3 bytes in UTF-8) produced zero fragments and got no benefit from this cache whatsoever.
//!
//! We do not go below 3 bytes. A back-of-envelope collision estimate (assuming uniform
//! random bytes, which is generous -- real source text has much lower effective entropy)
//! puts the odds of a *specific* fragment appearing in a file purely by chance at roughly
//! `positions_checked / 256^fragment_len`. Each byte removed from the fragment costs a factor
//! of 256x here. At 2 bytes, a 64KB file already has a >50% chance of a spurious "found"
//! signal, and by ~1MB it's essentially guaranteed -- i.e. the presence check almost never
//! says "definitely not here" so it stops being a useful filter at all. 3 bytes is still
//! meaningfully discriminating for typical source-file sizes, so that's the floor.
//!
//! See [`MIN_FRAGMENT_LEN`] and [`select_fragment_len`].
//!
//! ### Space Complexity
//!
//! - **Per pattern:** 4 bytes * num_fragments (typically 8-40 bytes)
//! - **Per file:**    num_fragments / 8 bytes for bitset (typically 1-100 bytes)
//! - **Total cache:** ~42 bytes per file + fragment overhead
//!   - 100 MB cache -> ~2.3M files tracked
//!
//! ## References
//!
//! Inspired by nowgrep's fragment-based filtering:
//! - <https://github.com/asbott/nowgrep>
//! - Similar to Bloom filters but with explicit tracking

use crate::util::prefetch_read;

use nohash_hasher::IntSet;

#[derive(Clone, Copy)]
pub enum FragmentLen { Three, Four }

impl FragmentLen {
    #[inline(always)]
    pub const fn from_fragment_len(fragment_len: usize) -> Self {
        match fragment_len {
            3 => FragmentLen::Three,
            4 => FragmentLen::Four,
            _ => unsafe { std::hint::unreachable_unchecked() }
        }
    }

    #[inline(always)]
    pub const fn as_usize(self) -> usize {
        match self {
            FragmentLen::Three => 3,
            FragmentLen::Four  => 4,
        }
    }
}

/// Shortest pattern length for which the fragment cache is worth using at all. Below this,
/// the false-positive rate of the presence check is too high (see module docs) to provide
/// any real filtering power, so callers should skip the fragment cache entirely rather than
/// use a degenerate 1- or 2-byte window.
pub const MIN_FRAGMENT_LEN: usize = 3;

pub const FRAGMENT_HASH_MULTIPLIER: u32 = 0x9e3779b9;

/// Hash a 4-byte fragment to u32. For fragments shorter than 4 bytes, the caller is expected
/// to zero-pad the unused trailing bytes (see [`extract_pattern_fragments_with_fragment`]) so
/// this always operates on a consistent 4-byte value.
#[inline(always)]
pub const fn hash_fragment(frag: [u8; 4]) -> u32 {
    // SAFETY: 0x9e3779b9 is odd, so multiplication by it is a
    // bijection on Z/2^32Z (odd constants are units mod 2^32). This means
    // hash_fragment can NEVER produce a collision for distinct 4-byte inputs.
    //
    // FragmentCache relies on this: it treats hash equality as fragment
    // identity (see find_fragment_index / add_fragment) with no fallback
    // verification. If this constant is ever changed, it MUST remain odd,
    // or FragmentCache's collision-free assumption breaks silently.
    u32::from_le_bytes(frag).wrapping_mul(FRAGMENT_HASH_MULTIPLIER)
}

#[inline(always)]
pub const fn hash_fragment_u32(frag: u32) -> u32 {
    frag.wrapping_mul(FRAGMENT_HASH_MULTIPLIER)
}

/// Byte mask that zeroes out everything past `fragment_len` bytes in a little-endian u32, so a
/// masked 4-byte load from a buffer can stand in for a genuine `fragment_len`-byte fragment.
/// `fragment_len` must be in `1..=4`; anything else is treated as a full 4-byte fragment.
#[inline(always)]
pub const fn fragment_mask_u32(fragment_len: usize) -> u32 {
    match fragment_len {
        1 => 0x0000_00FF,
        2 => 0x0000_FFFF,
        3 => 0x00FF_FFFF,
        _ => 0xFFFF_FFFF,
    }
}

/// Pick a single fragment length usable across every literal in `patterns`, or `None`
/// if the fragment cache should be skipped entirely for this query.
///
/// This has to be the *minimum* over every literal, not the maximum or an average: if pattern
/// A is 3 bytes and pattern B is 8 bytes, and we picked a 4-byte fragment, then A -- being
/// shorter than the fragment -- would contribute zero fragments. The presence check would then
/// only ever be vouching for B, and "no fragments found" could incorrectly skip a file that
/// actually contains a match via A. So every literal must be at least `fragment_len` bytes, and
/// the only way to guarantee that is to key off the shortest one. If even the shortest
/// literal is under [`MIN_FRAGMENT_LEN`], the whole query bails out of the fragment cache
/// (returns `None`) rather than silently degrading to a fragment size we know is unreliable.
pub fn select_fragment_len<'a>(patterns: impl IntoIterator<Item = &'a [u8]>) -> Option<usize> {
    let shortest = patterns.into_iter().map(<[u8]>::len).min()?;

    if shortest < MIN_FRAGMENT_LEN {
        None
    } else {
        Some(shortest.min(4))
    }
}

#[inline(always)]
pub fn ascii_lowercase_u32_le(w: u32) -> u32 {
    let bytes = w.to_le_bytes().map(|b| b.to_ascii_lowercase());
    u32::from_le_bytes(bytes)
}

/// Determine stride for file fragment extraction based on file size.
///
/// Balances extraction speed vs accuracy using adaptive sampling:
/// - Small files  (<=64KB):   Scan all bytes (stride=1) for completeness
/// - Medium files (64KB-1MB): Sample every 8th byte for efficiency
/// - Large files  (>1MB):     Sample every 64th byte to avoid bottleneck
///
/// # Rationale
/// Fragment extraction happens off the critical path (after search completes).
/// Sampling is sufficient because if a 4-byte fragment exists in a file,
/// we'll likely find it even with sparse sampling.
///
/// @Heuristic @Tune
#[inline(always)]
pub const fn stride_heuristic(buf_len: usize) -> usize {
    match buf_len {
        0..=65536       => 1,  // 100% coverage for small files
        65537..=1048576 => 8,  // 12.5% coverage for medium files
        _               => 64, // 1.56% coverage for large files
    }
}

/// Extract fragment hashes from a search pattern using a `fragment_len`-byte sliding fragment
/// (`fragment_len` should be in `MIN_FRAGMENT_LEN..=4`, typically from [`select_fragment_len`]).
///
/// Fragments shorter than 4 bytes are zero-padded on the right before hashing, so they hash
/// identically to how [`check_fragment_presence`] hashes a masked 4-byte buffer load with the
/// same `fragment_len`.
#[inline]
pub fn extract_pattern_fragments_with_len(pattern: &[u8], fragment_len: usize) -> Vec<u32> {  // @Memory @Speed: This function is called pretty much ONCE in the whole program, so it's probably fine to allocate here.
    debug_assert!((1..=4).contains(&fragment_len));

    if pattern.len() < fragment_len {
        return Vec::new();
    }

    // for N bytes, we get N-(fragment_len-1) overlapping fragment_len-byte fragments
    let mut fragments = Vec::with_capacity(pattern.len().saturating_sub(fragment_len - 1));
    let mut seen = IntSet::default();

    for fragment in pattern.windows(fragment_len) {
        let mut frag = [0u8; 4];
        frag[..fragment_len].copy_from_slice(fragment);
        let hash = hash_fragment(frag);

        if seen.insert(hash) {
            fragments.push(hash);
        }
    }

    fragments
}

/// `fragment_len` must match whatever fragment length `fragment_hashes` was extracted with (see
/// [`extract_pattern_fragments_with_fragment`]) -- it controls how many trailing bytes of each
/// masked 4-byte buffer load are ignored.
#[inline]
pub fn check_fragment_presence(
    buf: &[u8],
    fragment_hashes: &[u32],
    fragment_presence_scratch: &mut [u64],
    fragment_index: &IntSet<u32>,
    fragment_len: usize,
    case_insensitive: bool,
) {
    let num_frags = fragment_hashes.len();

    if num_frags == 0 {
        return;
    }

    if buf.len() < 4 {
        return;
    }

    let mask = fragment_mask_u32(fragment_len);

    #[cfg(target_arch = "x86_64")] {
        if is_x86_feature_detected!("avx2") && buf.len() >= 32 {
            return unsafe {
                if case_insensitive {
                    check_fragment_presence_avx2_ci(buf, fragment_hashes, fragment_presence_scratch, mask)
                } else {
                    check_fragment_presence_avx2(buf, fragment_hashes, fragment_presence_scratch, mask)
                }
            }
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        if std::arch::is_aarch64_feature_detected!("neon") && buf.len() >= 16 {
            return unsafe {
                if case_insensitive {
                    check_fragment_presence_neon_ci(buf, fragment_hashes, fragment_presence_scratch, mask)
                } else {
                    check_fragment_presence_neon(buf, fragment_hashes, fragment_presence_scratch, mask)
                }
            }
        }
    }

    check_fragment_presence_scalar(
        buf,
        fragment_hashes,
        fragment_presence_scratch,
        fragment_index,
        mask,
        case_insensitive,
    )
}

/// Scalar fallback for fragment presence checking
#[inline]
pub fn check_fragment_presence_scalar(
    buf: &[u8],
    fragment_hashes: &[u32],
    fragment_presence_scratch: &mut [u64],
    fragment_index: &IntSet<u32>,
    mask: u32,
    case_insensitive: bool,
) {
    let num_frags = fragment_hashes.len();
    let stride = stride_heuristic(buf.len());

    let mut found_count = 0;

    let mut i = 0;
    while i + 4 <= buf.len() {
        // Address-only dependency
        let ahead = i + stride;
        if ahead + 4 <= buf.len() {
            prefetch_read(unsafe { buf.as_ptr().add(ahead) });
        }

        let mut raw = u32::from_le_bytes([buf[i], buf[i+1], buf[i+2], buf[i+3]]);
        if case_insensitive {
            raw = ascii_lowercase_u32_le(raw);
        }
        raw &= mask;

        let hash = hash_fragment_u32(raw);

        if fragment_index.contains(&hash) {
            for (idx, &frag_hash) in fragment_hashes.iter().enumerate() {
                if frag_hash == hash {
                    let (word, bit) = (idx / 64, 1u64 << (idx % 64));
                    if fragment_presence_scratch[word] & bit == 0 {
                        fragment_presence_scratch[word] |= bit;
                        found_count += 1;
                        if found_count == num_frags { return; }
                    }

                    break;
                }
            }
        }

        i += stride;
    }
}

//
// Case folding primitives.
//
//
// Both expose a single function that lowercases every byte in a full SIMD
// register, ASCII-only, with no cross-byte or cross-lane dependency: byte
// value in, byte value out, independently per lane.
//
// That memoryless property is what makes it safe to apply to a register built from
// *overlapping* sliding windows (see the fragment_load closures below), the
// fold doesn't know or care which logical 4-byte window a given byte
// belongs to, and a byte's lowercase form never depends on its neighbors or
// its position, so folding the raw packed bytes once and then slicing them
// back into windows gives exactly the same answer as folding each window
// individually first.
//

#[cfg(target_arch = "x86_64")]
pub(crate) mod simd_fold {
    use std::arch::x86_64::*;

    /// Lowercase every byte in a 256-bit AVX2 vector, ASCII-only.
    /// Byte-for-byte equivalent to mapping `u8::to_ascii_lowercase` over the 32 lanes independently.
    ///
    /// AVX2 has no unsigned byte compare ... only signed `_mm256_cmpgt_epi8`,
    /// so an unsigned range check on `0..=255` has to be emulated: XOR every
    /// byte with 0x80 first.
    ///
    /// That flip is a monotonic bijection from the unsigned ordering `[0, 255]`
    /// onto the signed ordering `[-128, 127]`, so comparing the biased bytes with
    /// 'signed >' reproduces an unsigned `>` on the originals.
    #[target_feature(enable = "avx2")]
    #[allow(unsafe_op_in_unsafe_fn)]
    pub unsafe fn ascii_lowercase_avx2(v: __m256i) -> __m256i {
        let bias = _mm256_set1_epi8(0x80u8 as i8);
        let biased = _mm256_xor_si256(v, bias);

        // byte >= 'A' (0x41)  <=>  biased > biased(0x40)
        let ge_a = _mm256_cmpgt_epi8(biased, _mm256_set1_epi8((0x40u8 ^ 0x80u8) as i8));

        // byte <= 'Z' (0x5A)  <=>  biased(0x5B) > biased
        let le_z = _mm256_cmpgt_epi8(_mm256_set1_epi8((0x5Bu8 ^ 0x80u8) as i8), biased);

        let is_upper = _mm256_and_si256(ge_a, le_z);
        let lower_bit = _mm256_and_si256(is_upper, _mm256_set1_epi8(0x20));
        _mm256_or_si256(v, lower_bit)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[inline]
        fn run(bytes32: [u8; 32]) -> [u8; 32] {
            unsafe {
                let v = _mm256_loadu_si256(bytes32.as_ptr() as *const __m256i);
                let folded = ascii_lowercase_avx2(v);
                let mut out = [0u8; 32];
                _mm256_storeu_si256(out.as_mut_ptr() as *mut __m256i, folded);
                out
            }
        }

        #[inline]
        fn reference(bytes: [u8; 32]) -> [u8; 32] {
            bytes.map(|b| b.to_ascii_lowercase())
        }

        #[test]
        fn exhaustive_per_lane_with_boundary_neighbors() {
            // All 256 values in each of the 32 lane positions, other lanes
            // held at adversarial boundary values right outside the
            // 'A'-'Z' range (and 0x00/0xFF), to catch any lane-crossing
            // mistake in the bias/compare trick.
            let adversarial: [u8; 8] = [0x00, 0xFF, 0x40, 0x5B, 0x60, 0x7B, 0x41, 0x5A];

            for lane in 0..32 {
                for v in 0..=255u8 {
                    for &other in &adversarial {
                        let mut bytes = [other; 32];
                        bytes[lane] = v;
                        assert_eq!(run(bytes), reference(bytes), "lane {lane} value {v:#04x}");
                    }
                }
            }
        }

        #[test]
        fn large_random_sample() {
            let mut rng = 0x9E3779B97F4A7C15u64;
            let mut next_byte = || {
                rng ^= rng << 13;
                rng ^= rng >> 7;
                rng ^= rng << 17;
                (rng & 0xFF) as u8
            };

            for _ in 0..2_000_000u32 {
                let bytes: [u8; 32] = std::array::from_fn(|_| next_byte());
                assert_eq!(run(bytes), reference(bytes));
            }
        }
    }
}

#[cfg(target_arch = "aarch64")]
pub(crate) mod simd_fold {
    use std::arch::aarch64::*;

    /// Same contract as the AVX2 version above, but simpler.
    /// NEON has native unsigned byte compares (`vcgeq_u8`, `vcleq_u8`),
    /// so there's no need for AVX2's sign-bit-flip trick.
    /// The range check on 'A'..='Z' is a direct unsigned `>=`/`<=`.
    #[target_feature(enable = "neon")]
    #[allow(unsafe_op_in_unsafe_fn)]
    pub unsafe fn ascii_lowercase_neon(v: uint8x16_t) -> uint8x16_t {
        let ge_a = vcgeq_u8(v, vdupq_n_u8(b'A'));
        let le_z = vcleq_u8(v, vdupq_n_u8(b'Z'));
        let is_upper = vandq_u8(ge_a, le_z);
        let lower_bit = vandq_u8(is_upper, vdupq_n_u8(0x20));
        vorrq_u8(v, lower_bit)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[inline]
        fn run(bytes16: [u8; 16]) -> [u8; 16] {
            unsafe {
                let v = vld1q_u8(bytes16.as_ptr());
                let folded = ascii_lowercase_neon(v);
                let mut out = [0u8; 16];
                vst1q_u8(out.as_mut_ptr(), folded);
                out
            }
        }

        #[inline]
        fn reference(bytes: [u8; 16]) -> [u8; 16] {
            bytes.map(|b| b.to_ascii_lowercase())
        }

        #[test]
        fn exhaustive_per_lane_with_boundary_neighbors() {
            // Mirrors the AVX2 exhaustive test exactly, just with 16 lanes
            // instead of 32. NEON's compares are unsigned so there's no
            // bias trick to get wrong, but the range boundaries ('@'/'A'
            // and 'Z'/'[') are still the values most likely to expose an
            // off-by-one in `vcgeq_u8`/`vcleq_u8` usage.
            let adversarial: [u8; 8] = [0x00, 0xFF, 0x40, 0x5B, 0x60, 0x7B, 0x41, 0x5A];

            for lane in 0..16 {
                for v in 0..=255u8 {
                    for &other in &adversarial {
                        let mut bytes = [other; 16];
                        bytes[lane] = v;
                        assert_eq!(run(bytes), reference(bytes), "lane {lane} value {v:#04x}");
                    }
                }
            }
        }

        #[test]
        fn large_random_sample() {
            let mut rng = 0x2545F4914F6CDD1Du64;
            let mut next_byte = || {
                rng ^= rng << 13;
                rng ^= rng >> 7;
                rng ^= rng << 17;
                (rng & 0xFF) as u8
            };

            for _ in 0..2_000_000u32 {
                let bytes: [u8; 16] = std::array::from_fn(|_| next_byte());
                assert_eq!(run(bytes), reference(bytes));
            }
        }
    }
}


// Hashes one window scalar-style, folding case first when this instantiation is the CI variant.
#[inline(always)]
fn tail_hash_at<const CASE_INSENSITIVE: bool>(buf: &[u8], offset: usize, mask: u32) -> u32 {
    let mut raw = u32::from_le_bytes([
        buf[offset], buf[offset + 1], buf[offset + 2], buf[offset + 3],
    ]);

    if CASE_INSENSITIVE {
        raw = ascii_lowercase_u32_le(raw);
    }

    raw &= mask;
    hash_fragment_u32(raw)
}

macro_rules! impl_fragment_presence_scanner {
    (
        fn_name: $fn_name:ident,
        cfg: $cfg_arch:literal,

        feature: $feature:literal,
        lanes: $lanes:expr,
        min_stride: $min_stride:expr,

        case_insensitive: $case_insensitive:expr,
        hashes_ty: $hashes_ty:ty,

        fragment_load: |$data_ptr:ident, $mask_val:ident| $load_block:block,
        splat: |$scalar:ident| $splat_block:block,
        any_match: |$ha:ident, $hb:ident| $match_block:block,
    ) => {
        /// # Safety
        /// Don't fuck up
        #[cfg(target_arch = $cfg_arch)]
        #[target_feature(enable = $feature)]
        #[allow(unsafe_op_in_unsafe_fn)]
        pub unsafe fn $fn_name(buf: &[u8], fragment_hashes: &[u32], fragment_presence_scratch: &mut [u64], mask: u32) {
            let num_frags = fragment_hashes.len();
            let stride = stride_heuristic(buf.len()).max($min_stride);
            let buf_len = buf.len();

            //
            // Each outer iteration reads `$lanes` overlapping 4-byte
            // windows starting at `offset` (offset+0, offset+1, ..., offset+lanes-1).
            //
            // The last of those windows starts at offset + lanes - 1
            // and itself reads 4 bytes, so the furthest byte touched is
            // `offset + lanes + 2`. i.e. the iteration needs
            // `offset + (lanes + 3) <= buf_len` bytes remaining.
            //
            let read_extent = $lanes + 3;

            if crate::util::likely(num_frags <= 64) {
                //
                // Fast path: register-only bitmask
                //

                let mut found_mask: u64 = 0;
                let all_found_mask: u64 = if num_frags == 64 { u64::MAX } else { (1u64 << num_frags) - 1 };

                let mut offset = 0;
                while offset + read_extent <= buf_len {
                    let $data_ptr = buf.as_ptr().add(offset);
                    let $mask_val = mask;
                    let hashes: $hashes_ty = $load_block;

                    let ahead = offset + stride;
                    if ahead + 4 <= buf_len {
                        prefetch_read(buf.as_ptr().add(ahead));
                    }

                    for (frag_idx, &frag_hash) in fragment_hashes.iter().enumerate() {
                        let bit = 1u64 << frag_idx;
                        if found_mask & bit != 0 {
                            continue;
                        }

                        let $scalar = frag_hash;
                        let pattern: $hashes_ty = $splat_block;
                        let $ha = hashes;
                        let $hb = pattern;
                        if $match_block {
                            found_mask |= bit;
                        }
                    }

                    if found_mask == all_found_mask {
                        break;
                    }

                    offset += stride;
                }

                //
                // Tail: sweep whatever window-starts the batching above
                // left unvisited (up to lanes - 1 of them), one at a time.
                //
                if found_mask != all_found_mask {
                    while offset + 4 <= buf_len {
                        let hash = tail_hash_at::<$case_insensitive>(buf, offset, mask);
                        for (frag_idx, &frag_hash) in fragment_hashes.iter().enumerate() {
                            let bit = 1u64 << frag_idx;
                            if found_mask & bit != 0 {
                                continue;
                            }

                            if frag_hash == hash {
                                found_mask |= bit;
                                break;
                            }
                        }

                        if found_mask == all_found_mask {
                            break;
                        }

                        offset += 1;
                    }
                }


                fragment_presence_scratch[0] = found_mask;
            } else {
                //
                // Fallback: >64 fragments, can't fit a register mask
                //

                let mut remaining = num_frags;

                let mut offset = 0;
                while offset + read_extent <= buf_len {
                    let $data_ptr = buf.as_ptr().add(offset);
                    let $mask_val = mask;
                    let hashes: $hashes_ty = $load_block;

                    for (frag_idx, &frag_hash) in fragment_hashes.iter().enumerate() {
                        debug_assert!(frag_idx / 64 < fragment_presence_scratch.len());

                        if *fragment_presence_scratch.get_unchecked(frag_idx / 64) & (1u64 << (frag_idx % 64)) != 0 {
                            continue;
                        }

                        let $scalar = frag_hash;
                        let pattern: $hashes_ty = $splat_block;
                        let $ha = hashes;
                        let $hb = pattern;
                        if $match_block {
                            *fragment_presence_scratch.get_unchecked_mut(frag_idx / 64) |= 1u64 << (frag_idx % 64);
                            remaining -= 1;
                        }
                    }

                    if remaining == 0 {
                        break;
                    }

                    offset += stride;
                }

                if remaining != 0 {
                    // @Cutnpaste from above

                    //
                    // Tail: sweep whatever window-starts the batching above
                    // left unvisited (up to lanes - 1 of them), one at a time.
                    //

                    while offset + 4 <= buf_len {
                        let hash = tail_hash_at::<$case_insensitive>(buf, offset, mask);
                        for (frag_idx, &frag_hash) in fragment_hashes.iter().enumerate() {
                            if *fragment_presence_scratch.get_unchecked(frag_idx / 64) & (1u64 << (frag_idx % 64)) != 0 {
                                continue;
                            }

                            if frag_hash == hash {
                                *fragment_presence_scratch.get_unchecked_mut(frag_idx / 64) |= 1u64 << (frag_idx % 64);
                                remaining -= 1;
                                break;
                            }
                        }

                        if remaining == 0 {
                            break;
                        }

                        offset += 1;
                    }
                }
            }
        }
    };
}

//
//
//
// ------------------------------------ avx2 kernel
//
//
//

impl_fragment_presence_scanner! {
    fn_name: check_fragment_presence_avx2,
    cfg: "x86_64",
    feature: "avx2",
    lanes: 8,
    min_stride: 8,
    case_insensitive: false,
    hashes_ty: std::arch::x86_64::__m256i,

    fragment_load: |data_ptr, mask_val| {
        use std::arch::x86_64::*;

        let w0 = (data_ptr.add(0) as *const u32).read_unaligned();
        let w1 = (data_ptr.add(1) as *const u32).read_unaligned();
        let w2 = (data_ptr.add(2) as *const u32).read_unaligned();

        let w3 = (data_ptr.add(3) as *const u32).read_unaligned();
        let w4 = (data_ptr.add(4) as *const u32).read_unaligned();
        let w5 = (data_ptr.add(5) as *const u32).read_unaligned();
        let w6 = (data_ptr.add(6) as *const u32).read_unaligned();
        let w7 = (data_ptr.add(7) as *const u32).read_unaligned();
        let fragments = _mm256_set_epi32(
            w7 as i32, w6 as i32, w5 as i32, w4 as i32,
            w3 as i32, w2 as i32, w1 as i32, w0 as i32,
        );

        // Zero out the trailing bytes beyond the fragment (no-op when fragment_len == 4)
        // *before* multiplying, so this matches hash_fragment_u32(raw & mask) exactly.
        let masked = _mm256_and_si256(fragments, _mm256_set1_epi32(mask_val as i32));

        // Hash multiplier constant: 0x9e3779b9 (golden ratio).
        // Let's pray LLVM's LICM hoists the mask/multiplier broadcasts out the loop.
        _mm256_mullo_epi32(masked, _mm256_set1_epi32(0x9e3779b9_u32 as i32))
    },

    splat: |scalar| {
        std::arch::x86_64::_mm256_set1_epi32(scalar as i32)
    },

    any_match: |a, b| {
        use std::arch::x86_64::*;
        _mm256_movemask_epi8(_mm256_cmpeq_epi32(a, b)) != 0
    },
}

impl_fragment_presence_scanner! {
    fn_name: check_fragment_presence_avx2_ci,
    cfg: "x86_64",
    feature: "avx2",
    lanes: 8,
    min_stride: 8,
    case_insensitive: true,
    hashes_ty: std::arch::x86_64::__m256i,

    fragment_load: |data_ptr, mask_val| {
        use std::arch::x86_64::*;

        // @Cutnpaste from check_fragment_presence_avx2

        let w0 = (data_ptr.add(0) as *const u32).read_unaligned();
        let w1 = (data_ptr.add(1) as *const u32).read_unaligned();
        let w2 = (data_ptr.add(2) as *const u32).read_unaligned();

        let w3 = (data_ptr.add(3) as *const u32).read_unaligned();
        let w4 = (data_ptr.add(4) as *const u32).read_unaligned();
        let w5 = (data_ptr.add(5) as *const u32).read_unaligned();
        let w6 = (data_ptr.add(6) as *const u32).read_unaligned();
        let w7 = (data_ptr.add(7) as *const u32).read_unaligned();
        let fragments = _mm256_set_epi32(
            w7 as i32, w6 as i32, w5 as i32, w4 as i32,
            w3 as i32, w2 as i32, w1 as i32, w0 as i32,
        );

        //
        // Fold BEFORE masking/hashing, folding the whole register at once
        // is equivalent to folding each of the 8 overlapping u32 windows
        // individually first.
        //
        // (see the `simd_fold` module doc comment for why overlap doesn't matter here).
        //

        let folded = simd_fold::ascii_lowercase_avx2(fragments);
        let masked = _mm256_and_si256(folded, _mm256_set1_epi32(mask_val as i32));
        _mm256_mullo_epi32(masked, _mm256_set1_epi32(0x9e3779b9_u32 as i32))
    },

    splat: |scalar| {
        std::arch::x86_64::_mm256_set1_epi32(scalar as i32)
    },

    any_match: |a, b| {
        use std::arch::x86_64::*;
        _mm256_movemask_epi8(_mm256_cmpeq_epi32(a, b)) != 0
    },
}

//
//
// ------------------------------------ aarch64 kernel
//
//

impl_fragment_presence_scanner! {
    fn_name: check_fragment_presence_neon,
    cfg: "aarch64",
    feature: "neon",
    lanes: 4,
    min_stride: 4,
    case_insensitive: false,
    hashes_ty: std::arch::aarch64::uint32x4_t,

    fragment_load: |data_ptr, mask_val| {
        use std::arch::aarch64::*;
        let w0 = (data_ptr.add(0) as *const u32).read_unaligned();
        let w1 = (data_ptr.add(1) as *const u32).read_unaligned();
        let w2 = (data_ptr.add(2) as *const u32).read_unaligned();
        let w3 = (data_ptr.add(3) as *const u32).read_unaligned();
        let fragments = vld1q_u32([w0, w1, w2, w3].as_ptr());
        let masked = vandq_u32(fragments, vdupq_n_u32(mask_val));
        vmulq_u32(masked, vdupq_n_u32(0x9e3779b9))
    },

    splat: |scalar| {
        std::arch::aarch64::vdupq_n_u32(scalar)
    },

    any_match: |a, b| {
        use std::arch::aarch64::*;

        //
        // vceqq_u32 gives an all-ones/all-zeros mask per lane;
        // vmaxvq_u32 horizontally reduces the 4 lanes to a single u32,
        // which is nonzero if at least one lane matched.
        //

        vmaxvq_u32(vceqq_u32(a, b)) != 0
    },
}

impl_fragment_presence_scanner! {
    fn_name: check_fragment_presence_neon_ci,
    cfg: "aarch64",
    feature: "neon",
    lanes: 4,
    min_stride: 4,
    case_insensitive: true,
    hashes_ty: std::arch::aarch64::uint32x4_t,

    fragment_load: |data_ptr, mask_val| {
        use std::arch::aarch64::*;

        let w0 = (data_ptr.add(0) as *const u32).read_unaligned();
        let w1 = (data_ptr.add(1) as *const u32).read_unaligned();
        let w2 = (data_ptr.add(2) as *const u32).read_unaligned();
        let w3 = (data_ptr.add(3) as *const u32).read_unaligned();

        let words: [u32; 4] = [w0, w1, w2, w3];
        let fragments = vld1q_u32(words.as_ptr());

        //
        // Reinterpret the 4 packed u32 lanes as 16 packed bytes,
        // fold case byte-wise, then reinterpret back.
        //

        let folded_bytes = crate::simd_fold::ascii_lowercase_neon(vreinterpretq_u8_u32(fragments));
        let folded = vreinterpretq_u32_u8(folded_bytes);
        let masked = vandq_u32(folded, vdupq_n_u32(mask_val));
        vmulq_u32(masked, vdupq_n_u32(0x9e3779b9u32))
    },

    splat: |scalar| {
        std::arch::aarch64::vdupq_n_u32(scalar)
    },

    any_match: |a, b| {
        use std::arch::aarch64::*;
        vmaxvq_u32(vceqq_u32(a, b)) != 0
    },
}
