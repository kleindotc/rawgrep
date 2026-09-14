use rawgrep::fragments::*;

use nohash_hasher::IntSet;

#[cfg(test)]
mod simd_tests {
    use super::*;

    // Builds the (hashes, index, fragment_len) triple for `pattern` and
    // runs it through the public dispatch function. For case-insensitive
    // queries, lowercases `pattern` first, mirroring what the real
    // caller does (the pattern side lowercases once up front; only the
    // buffer side needs to fold on the fly while scanning).
    fn presence_bits(buf: &[u8], pattern: &[u8], case_insensitive: bool) -> (Vec<bool>, usize) {
        let fragment_len = select_fragment_len(std::iter::once(pattern)).unwrap();
        let lowered;
        let pattern_for_extraction = if case_insensitive {
            lowered = pattern.to_ascii_lowercase();
            lowered.as_slice()
        } else {
            pattern
        };
        let hashes = extract_pattern_fragments_with_len(pattern_for_extraction, fragment_len);
        let index: IntSet<u32> = hashes.iter().copied().collect();
        let mut scratch = vec![0u64; hashes.len().div_ceil(64)];
        check_fragment_presence(buf, &hashes, &mut scratch, &index, fragment_len, case_insensitive);
        let bits: Vec<bool> = (0..hashes.len())
            .map(|i| scratch[i / 64] & (1u64 << (i % 64)) != 0)
            .collect();
        (bits, fragment_len)
    }

    // === Baseline regression tests, case-sensitive path (unchanged) ===

    #[test]
    fn baseline_finds_literal_present_in_large_buffer_via_avx2() {
        let mut buf = vec![b'x'; 40];
        buf[10..16].copy_from_slice(b"needle");
        let (bits, _) = presence_bits(&buf, b"needle", false);
        assert!(bits.iter().all(|&b| b), "every fragment of a present literal should be found");
    }

    #[test]
    fn baseline_correctly_reports_absent_in_large_buffer_via_avx2() {
        let buf = vec![b'x'; 40];
        let (bits, _) = presence_bits(&buf, b"needle", false);
        assert!(bits.iter().all(|&b| !b), "no fragment should be found in an unrelated buffer");
    }

    #[test]
    fn baseline_finds_literal_present_in_small_buffer_via_scalar() {
        let buf = b"..needle..";
        let (bits, _) = presence_bits(buf, b"needle", false);
        assert!(bits.iter().all(|&b| b));
    }

    #[test]
    fn baseline_case_sensitive_does_not_match_different_case() {
        let mut buf = vec![b'x'; 40];
        buf[10..16].copy_from_slice(b"NEEDLE");
        let (bits, _) = presence_bits(&buf, b"needle", false);
        assert!(bits.iter().all(|&b| !b));
    }

    // === Case-insensitive, small buffer (scalar path) ===

    #[test]
    fn case_insensitive_finds_uppercase_via_scalar() {
        let buf = b"..NEEDLE..";
        let (bits, _) = presence_bits(buf, b"needle", true);
        assert!(bits.iter().all(|&b| b));
    }

    #[test]
    fn case_insensitive_finds_mixed_case_via_scalar() {
        let buf = b"..NeEdLe..";
        let (bits, _) = presence_bits(buf, b"needle", true);
        assert!(bits.iter().all(|&b| b));
    }

    #[test]
    fn case_insensitive_still_correctly_reports_absent() {
        let buf = b"totally unrelated text here";
        let (bits, _) = presence_bits(buf, b"needle", true);
        assert!(bits.iter().all(|&b| !b));
    }

    #[test]
    fn case_insensitive_large_buffer_uses_simd_ci_kernel() {
        let mut buf = vec![b'x'; 40];
        buf[10..16].copy_from_slice(b"NEEDLE");
        let (bits, _) = presence_bits(&buf, b"needle", true);
        assert!(bits.iter().all(|&b| b));
    }

    #[test]
    fn case_insensitive_large_buffer_absent_via_simd_ci_kernel() {
        let buf = vec![b'x'; 40];
        let (bits, _) = presence_bits(&buf, b"needle", true);
        assert!(bits.iter().all(|&b| !b));
    }

    #[test]
    fn case_insensitive_mixed_case_large_buffer() {
        let mut buf = vec![b'-'; 50];
        buf[20..26].copy_from_slice(b"NeEdLe");
        let (bits, _) = presence_bits(&buf, b"needle", true);
        assert!(bits.iter().all(|&b| b));
    }

    // Bytes immediately outside 'A'..='Z' ('@', '[', and their lowercase
    // analogues '`'/'{') exercised through the *whole* scanner on a
    // buffer large enough to force the SIMD path.
    // Embeds a pattern built from these boundary bytes plus letters,
    // checks it's found when case-swapped, and checks a near-miss
    // (boundary byte shifted by one, which should NOT be treated as the same letter)
    // is not falsely matched.
    #[test]
    fn case_insensitive_boundary_bytes_in_real_scan() {
        let pattern: &[u8] = b"A@Zx[a"; // includes 'A','Z' (fold) and '@','[' (must NOT fold)
        let mut buf = vec![b'.'; 48];
        buf[15..15 + pattern.len()].copy_from_slice(b"a@zX[A"); // case-swapped letters, boundary bytes untouched
        let (bits, _) = presence_bits(&buf, pattern, true);
        assert!(bits.iter().all(|&b| b), "case-swapped letters with untouched boundary bytes should match");

        // Now corrupt one boundary byte ('@' -> '`', both non-letters,
        // but different values) and confirm that specific window's
        // hash is no longer treated as present just because it's
        // "close" to a letter boundary.
        let mut buf2 = vec![b'.'; 48];
        buf2[15..15 + pattern.len()].copy_from_slice(b"a`zX[A");
        let (bits2, _) = presence_bits(&buf2, pattern, true);
        assert!(!bits2.iter().all(|&b| b), "corrupting a boundary byte must not still read as a full match");
    }

    // === Direct, backend-specific invocation: bypasses is_x86_feature_detected!/
    // is_aarch64_feature_detected! at the dispatch layer and calls the
    // generated kernel directly, so a passing test here can only mean
    // that *specific* kernel is correct (not "some kernel or other"). ===

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn avx2_ci_direct_invocation_finds_and_reports_absent() {
        if !is_x86_feature_detected!("avx2") {
            return;
        }
        let pattern = b"needle";
        let fragment_len = select_fragment_len(std::iter::once(pattern.as_slice())).unwrap();
        let hashes = extract_pattern_fragments_with_len(pattern, fragment_len);
        let mask = fragment_mask_u32(fragment_len);

        let mut present_buf = vec![b'x'; 40];
        present_buf[10..16].copy_from_slice(b"NeEdLE");
        let mut scratch = vec![0u64; hashes.len().div_ceil(64)];
        unsafe { check_fragment_presence_avx2_ci(&present_buf, &hashes, &mut scratch, mask) };
        assert!((0..hashes.len()).all(|i| scratch[i / 64] & (1u64 << (i % 64)) != 0));

        let absent_buf = vec![b'x'; 40];
        let mut scratch2 = vec![0u64; hashes.len().div_ceil(64)];
        unsafe { check_fragment_presence_avx2_ci(&absent_buf, &hashes, &mut scratch2, mask) };
        assert!((0..hashes.len()).all(|i| scratch2[i / 64] & (1u64 << (i % 64)) == 0));
    }

    #[cfg(target_arch = "aarch64")]
    #[test]
    fn neon_ci_direct_invocation_finds_and_reports_absent() {
        if !std::arch::is_aarch64_feature_detected!("neon") {
            return;
        }
        let pattern = b"needle";
        let fragment_len = select_fragment_len(std::iter::once(pattern.as_slice())).unwrap();
        let hashes = extract_pattern_fragments_with_len(pattern, fragment_len);
        let mask = fragment_mask_u32(fragment_len);

        let mut present_buf = vec![b'x'; 24];
        present_buf[5..11].copy_from_slice(b"NeEdLE");
        let mut scratch = vec![0u64; (hashes.len() + 63) / 64];
        unsafe { check_fragment_presence_neon_ci(&present_buf, &hashes, &mut scratch, mask) };
        assert!((0..hashes.len()).all(|i| scratch[i / 64] & (1u64 << (i % 64)) != 0));

        let absent_buf = vec![b'x'; 24];
        let mut scratch2 = vec![0u64; (hashes.len() + 63) / 64];
        unsafe { check_fragment_presence_neon_ci(&absent_buf, &hashes, &mut scratch2, mask) };
        assert!((0..hashes.len()).all(|i| scratch2[i / 64] & (1u64 << (i % 64)) == 0));
    }

    #[cfg(target_arch = "aarch64")]
    #[test]
    fn neon_case_sensitive_direct_invocation_matches_avx2_semantics() {
        // The NEON case-sensitive kernel didn't exist in this file
        // before (only referenced by the dispatcher); this confirms it
        // behaves like its AVX2 counterpart: finds an exact-case
        // literal, and does NOT fold a different-case occurrence.
        if !std::arch::is_aarch64_feature_detected!("neon") {
            return;
        }
        let pattern = b"needle";
        let fragment_len = select_fragment_len(std::iter::once(pattern.as_slice())).unwrap();
        let hashes = extract_pattern_fragments_with_len(pattern, fragment_len);
        let mask = fragment_mask_u32(fragment_len);

        let mut exact_buf = vec![b'x'; 24];
        exact_buf[5..11].copy_from_slice(b"needle");
        let mut scratch = vec![0u64; (hashes.len() + 63) / 64];
        unsafe { check_fragment_presence_neon(&exact_buf, &hashes, &mut scratch, mask) };
        assert!((0..hashes.len()).all(|i| scratch[i / 64] & (1u64 << (i % 64)) != 0));

        let mut different_case_buf = vec![b'x'; 24];
        different_case_buf[5..11].copy_from_slice(b"NEEDLE");
        let mut scratch2 = vec![0u64; (hashes.len() + 63) / 64];
        unsafe { check_fragment_presence_neon(&different_case_buf, &hashes, &mut scratch2, mask) };
        assert!((0..hashes.len()).all(|i| scratch2[i / 64] & (1u64 << (i % 64)) == 0));
    }

    // === >64 fragments: exercises the `remaining`/per-word scratch
    // branch (as opposed to the single-u64 `found_mask` branch used
    // when num_frags <= 64), for the case-insensitive path specifically,
    // since that branch is untouched by the existing baseline tests. ===

    /// 80 pairwise-distinct printable-range ASCII bytes
    /// (0x30..0x80: a contiguous run covering digits, punctuation, and both letter cases).
    /// Because every byte in the string is unique, every 4-byte
    /// sliding window is also unique as a substring (shifting the
    /// window by one always swaps in a byte that appears nowhere else
    /// in the string), so this pattern is guaranteed to produce
    /// 80 - 4 + 1 = 77 distinct fragments (modulo an astronomically
    /// unlikely hash collision in a 32-bit space), safely over the
    /// 64-fragment threshold.
    fn long_distinct_pattern() -> Vec<u8> {
        // Bytes chosen so lowercasing preserves distinctness too: includes
        // 'A'..='Z' (which will fold to 'a'..='z'), but deliberately excludes
        // 'a'..='z' and anything else to_ascii_lowercase could produce, so
        // folding never collides two different source bytes onto one value.
        let mut bytes: Vec<u8> = Vec::new();
        bytes.extend(b'A'..=b'Z');     // 26 -- folds to 'a'..'z'
        bytes.extend(b'0'..=b'9');     // 10
        bytes.extend(0x21u8..=0x2Fu8); // 15
        bytes.extend(0x3Au8..=0x40u8); // 7
        bytes.extend(0x5Bu8..=0x60u8); // 6
        bytes.extend(0x7Bu8..=0x7Eu8); // 4
        bytes.extend(0x80u8..=0x8Bu8); // 12 -- non-ASCII, untouched by ASCII-only fold
        assert_eq!(bytes.len(), 80);

        let mut seen = std::collections::HashSet::new();
        assert!(bytes.iter().all(|b| seen.insert(*b)), "must be pairwise distinct");
        let mut seen_folded = std::collections::HashSet::new();
        assert!(
            bytes.iter().all(|b| seen_folded.insert(b.to_ascii_lowercase())),
            "must stay pairwise distinct after lowercasing"
        );
        bytes
    }

    #[test]
    fn case_insensitive_many_fragments_found() {
        let pattern = long_distinct_pattern();
        let fragment_len = select_fragment_len(std::iter::once(pattern.as_slice())).unwrap();
        assert_eq!(fragment_len, 4);
        let lowered = pattern.to_ascii_lowercase();
        let hashes = extract_pattern_fragments_with_len(&lowered, fragment_len);
        assert!(hashes.len() > 64, "expected >64 fragments, got {}", hashes.len());
        let index: IntSet<u32> = hashes.iter().copied().collect();

        // Embed an uppercased copy of the (letter-containing tail of
        // the) pattern into a big buffer.
        let mut buf = vec![b'.'; 200];
        let upper: Vec<u8> = pattern.iter().map(|b| b.to_ascii_uppercase()).collect();
        buf[50..50 + upper.len()].copy_from_slice(&upper);

        let mut scratch = vec![0u64; hashes.len().div_ceil(64)];
        check_fragment_presence(&buf, &hashes, &mut scratch, &index, fragment_len, true);
        let all_found = (0..hashes.len()).all(|i| scratch[i / 64] & (1u64 << (i % 64)) != 0);
        assert!(all_found, "every fragment of the embedded (case-swapped) pattern should be found");
    }

    #[test]
    fn finds_literal_at_tail_not_covered_by_a_full_simd_batch() {
        // For a 40-byte buffer, AVX2's 8-lane batching covers window-starts
        // 0..=31 and then stops (offset 32 fails the read_extent check),
        // window-starts 32..=36 used to be silently unscanned.
        // Places the match right there.

        let mut buf = vec![b'x'; 40];
        buf[34..40].copy_from_slice(b"NeEdLE");
        let (bits, _) = presence_bits(&buf, b"needle", true);
        assert!(bits.iter().all(|&b| b));
    }

    #[test]
    fn case_insensitive_many_fragments_absent() {
        let pattern = long_distinct_pattern();
        let fragment_len = select_fragment_len(std::iter::once(pattern.as_slice())).unwrap();
        let lowered = pattern.to_ascii_lowercase();
        let hashes = extract_pattern_fragments_with_len(&lowered, fragment_len);
        assert!(hashes.len() > 64);
        let index: IntSet<u32> = hashes.iter().copied().collect();

        let buf = vec![b'.'; 200];
        let mut scratch = vec![0u64; hashes.len().div_ceil(64)];
        check_fragment_presence(&buf, &hashes, &mut scratch, &index, fragment_len, true);
        let none_found = (0..hashes.len()).all(|i| scratch[i / 64] & (1u64 << (i % 64)) == 0);
        assert!(none_found, "an unrelated buffer should report no fragments found");
    }

    // === Differential fuzz: scalar CI vs. whatever `check_fragment_presence`
    // dispatches to (AVX2/NEON/scalar depending on host and buffer size)
    // must always agree. This is the main safety net for the SIMD CI
    // kernels, it doesn't assume which backend is running, just that
    // they all compute the same presence bits. ===

    #[test]
    fn case_insensitive_scalar_and_dispatched_agree_on_random_inputs() {
        let mut rng = 0xD1B54A32D192ED03u64;
        let mut next = move || {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            rng
        };

        let alphabet: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789 _-";

        for case in 0..2000u32 {
            let buf_len = 4 + (next() % 220) as usize;
            let pat_len = 3 + (next() % 6) as usize; // 3..=8

            let mut buf: Vec<u8> = (0..buf_len).map(|_| alphabet[(next() as usize) % alphabet.len()]).collect();
            let pattern: Vec<u8> = (0..pat_len).map(|_| alphabet[(next() as usize) % alphabet.len()]).collect();

            // Half the time, implant a case-mangled copy of the pattern
            // into the buffer so real matches get exercised, not just
            // random noise.
            if case % 2 == 0 && buf.len() >= pattern.len() {
                let max_start = buf.len() - pattern.len();
                let pos = (next() as usize) % (max_start + 1);
                for (i, &b) in pattern.iter().enumerate() {
                    buf[pos + i] = if next() % 2 == 0 { b.to_ascii_uppercase() } else { b.to_ascii_lowercase() };
                }
            }

            let fragment_len = match select_fragment_len(std::iter::once(pattern.as_slice())) {
                Some(l) => l,
                None => continue,
            };
            let lowered = pattern.to_ascii_lowercase();
            let hashes = extract_pattern_fragments_with_len(&lowered, fragment_len);
            if hashes.is_empty() {
                continue;
            }
            let index: IntSet<u32> = hashes.iter().copied().collect();
            let mask = fragment_mask_u32(fragment_len);

            let mut scratch_dispatched = vec![0u64; hashes.len().div_ceil(64)];
            let mut scratch_scalar = vec![0u64; hashes.len().div_ceil(64)];

            check_fragment_presence(&buf, &hashes, &mut scratch_dispatched, &index, fragment_len, true);
            check_fragment_presence_scalar(&buf, &hashes, &mut scratch_scalar, &index, mask, true);

            assert_eq!(
                scratch_dispatched,
                scratch_scalar,
                "mismatch for buf={:?} pattern={:?}",
                String::from_utf8_lossy(&buf),
                String::from_utf8_lossy(&pattern),
            );
        }
    }

    #[test]
    fn ascii_lowercase_u32_le_matches_per_byte_reference_exhaustively_sampled() {
        fn reference(w: u32) -> u32 {
            u32::from_le_bytes(w.to_le_bytes().map(|b| b.to_ascii_lowercase()))
        }

        let adversarial: [u8; 6] = [0x00, 0xFF, 0x40, 0x5B, 0x60, 0x7B];
        for lane in 0..4 {
            for v in 0..=255u8 {
                for &other in &adversarial {
                    let mut bytes = [other; 4];
                    bytes[lane] = v;
                    let w = u32::from_le_bytes(bytes);
                    assert_eq!(ascii_lowercase_u32_le(w), reference(w));
                }
            }
        }

        let mut rng = 0x2545F4914F6CDD1Du64;
        for _ in 0..10_000_000u32 {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            let w = rng as u32;
            assert_eq!(ascii_lowercase_u32_le(w), reference(w));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_pattern_gets_fragments_with_3_byte_fragment() {
        // "foo" is 3 bytes -- with the old fixed 4-byte fragment this produced zero fragments.
        let fragment_len = select_fragment_len(std::iter::once("foo".as_bytes())).unwrap();
        assert_eq!(fragment_len, 3);

        let frags = extract_pattern_fragments_with_len("foo".as_bytes(), fragment_len);
        assert_eq!(frags.len(), 1, "a 3-byte pattern with a 3-byte fragment is exactly one fragment");
    }

    #[test]
    fn em_dash_gets_fragments() {
        // U+2014 EM DASH is 3 bytes in UTF-8: [0xE2, 0x80, 0x94]
        let em_dash = "—".as_bytes();
        assert_eq!(em_dash.len(), 3);

        let fragment_len = select_fragment_len(std::iter::once(em_dash)).unwrap();
        let frags = extract_pattern_fragments_with_len(em_dash, fragment_len);
        assert_eq!(frags.len(), 1);
    }

    #[test]
    fn too_short_pattern_disables_fragment_cache() {
        // 2 bytes is below MIN_FRAGMENT_LEN -- the whole point is that the cache is not worth
        // using at all here, not that it should use an even smaller fragment.
        assert_eq!(select_fragment_len(std::iter::once("fo".as_bytes())), None);
        assert_eq!(select_fragment_len(std::iter::once("f".as_bytes())), None);
    }

    #[test]
    fn mixed_length_alternation_uses_shortest() {
        // "fo|hello" -- "fo" is only 2 bytes, so the fragment cache must bail out entirely for
        // the whole alternation, not just quietly drop "fo" from consideration.
        let patterns: [&[u8]; 2] = ["fo".as_bytes(), "hello".as_bytes()];
        assert_eq!(select_fragment_len(patterns), None);

        // "foo|hello" -- shortest is 3 bytes, so fragment_len should be 3 for both.
        let patterns: [&[u8]; 2] = ["foo".as_bytes(), "hello".as_bytes()];
        assert_eq!(select_fragment_len(patterns), Some(3));
    }

    #[test]
    fn masked_buffer_hash_matches_padded_pattern_hash() {
        // The whole trick relies on: hash(pattern zero-padded to 4 bytes) ==
        // hash(masked 4-byte buffer load), for any 4th byte in the buffer.
        let fragment_len = 3;
        let mask = fragment_mask_u32(fragment_len);

        let pattern_frag = {
            let mut frag = [0u8; 4];
            frag[..3].copy_from_slice(b"foo");
            hash_fragment(frag)
        };

        for tail_byte in 0..=255u8 {
            let buf_word = u32::from_le_bytes([b'f', b'o', b'o', tail_byte]);
            let buf_hash = hash_fragment_u32(buf_word & mask);
            assert_eq!(buf_hash, pattern_frag, "tail byte {tail_byte} should be masked out");
        }
    }
}
