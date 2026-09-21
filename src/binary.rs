use crate::liner::Encoding;
use crate::worker::BINARY_CONTROL_COUNT;

#[inline(always)]
pub const fn is_dot_entry(name: &[u8]) -> bool {
    name.len() == 1 && name[0] == b'.' ||
    name.len() == 2 && name[0] == b'.' && name[1] == b'.'
}

#[inline(always)]
pub const fn is_hidden_entry(name: &[u8]) -> bool {
    name[0] == b'.'
}

pub use crate::binary_ext::{is_binary_ext, is_reserved_tool_dir};

#[inline(always)]
pub fn detect_byte_order_leading_mark_len(data: &[u8]) -> Option<Encoding> {
    if data.starts_with(&[0xFF, 0xFE, 0x00, 0x00]) { return Some(Encoding::Utf32LE) }
    if data.starts_with(&[0x00, 0x00, 0xFE, 0xFF]) { return Some(Encoding::Utf32BE) }
    if data.starts_with(&[0xEF, 0xBB, 0xBF])       { return Some(Encoding::Utf8)    }
    if data.starts_with(&[0xFF, 0xFE])             { return Some(Encoding::Utf16LE) }
    if data.starts_with(&[0xFE, 0xFF])             { return Some(Encoding::Utf16BE) }
    None
}

#[inline]
pub fn is_binary_chunk(data: &[u8]) -> bool {
    if data.is_empty() {
        return false;
    }

    //
    // A declared encoding overrides the heuristic entirely, since UTF-16/32 content
    // legitimately contains nulls that the check below would otherwise misread as binary.
    //
    if detect_byte_order_leading_mark_len(data).is_some() {
        return false;
    }

    let check_len = data.len().min(512);
    if memchr::memchr(0, &data[..check_len]).is_some() {
        return true;
    }

    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx2") {
            return unsafe { is_binary_chunk_avx2(data) };
        }
    }

    is_binary_chunk_(data)
}

/// # Safety
/// Caller's machine supports AVX2
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
pub unsafe fn is_binary_chunk_avx2(data: &[u8]) -> bool {
    use std::arch::x86_64::*;

    let check_len = data.len().min(512);
    let full = check_len / 32;
    let rem  = check_len - full * 32;
    let ptr  = data.as_ptr();

    let zero = _mm256_setzero_si256();
    let c1f  = _mm256_set1_epi8(0x1F);
    let tab  = _mm256_set1_epi8(0x09);
    let lf   = _mm256_set1_epi8(0x0A);
    let cr   = _mm256_set1_epi8(0x0D);
    let del  = _mm256_set1_epi8(0x7F);

    //
    //   bad_acc: per byte lane counter of rejected bytes. At most 17 iterations
    //            (16 full + 1 tail), so a u8 lane can never overflow.
    //
    //   nul_acc: OR of every 'byte == 0' mask, tested once at the end.
    //
    let mut bad_acc = zero;
    let mut nul_acc = zero;

    macro_rules! step {
        ($v:expr) => {{
            let v = $v;

            // Unsigned 'v <= 0x1F' as min(v, 0x1F) == v
            let below_space = _mm256_cmpeq_epi8(_mm256_min_epu8(v, c1f), v);

            let allowed = _mm256_or_si256(
                _mm256_or_si256(_mm256_cmpeq_epi8(v, tab), _mm256_cmpeq_epi8(v, lf)),
                _mm256_cmpeq_epi8(v, cr),
            );

            let bad = _mm256_or_si256(
                _mm256_andnot_si256(allowed, below_space),
                _mm256_cmpeq_epi8(v, del),
            );

            // Bad lanes are 0xFF == -1, so subtracting adds 1
            bad_acc = _mm256_sub_epi8(bad_acc, bad);
            nul_acc = _mm256_or_si256(nul_acc, _mm256_cmpeq_epi8(v, zero));
        }};
    }

    for i in 0..full {
        step!(unsafe { _mm256_loadu_si256(ptr.add(i * 32) as *const __m256i) });
    }

    // Tail: pad with spaces
    if rem != 0 {
        let mut tail = [0x20u8; 32];
        tail[..rem].copy_from_slice(&data[full * 32..check_len]);
        step!(unsafe { _mm256_loadu_si256(tail.as_ptr() as *const __m256i) });
    }

    if _mm256_testz_si256(nul_acc, nul_acc) == 0 {
        return true;
    }

    // Horizontal sum of the 32 byte counters: sad against zero gives four u64 sums.
    let sums = _mm256_sad_epu8(bad_acc, zero);
    let mut lanes = [0u64; 4];
    unsafe { _mm256_storeu_si256(lanes.as_mut_ptr() as *mut __m256i, sums); }

    let control_count = (lanes[0] + lanes[1] + lanes[2] + lanes[3]) as usize;

    control_count > BINARY_CONTROL_COUNT
}

#[inline]
fn is_binary_chunk_(data: &[u8]) -> bool {
    //
    // Portable fallback, same shape as the AVX2.
    //

    //
    // Lane counters: at most 512/16 + 1 = 33 iterations, so a u8 lane can never overflow.
    //
    #[inline(always)]
    fn is_rejected_byte(b: u8) -> u8 {
        // '&' and '|' on purpose, not '&&' and '||': no short circuit, so it stays branch free.
        (((b < 0x20) & (b != 0x09) & (b != 0x0A) & (b != 0x0D)) | (b == 0x7F)) as u8
    }

    const W: usize = 16;

    let check_len = data.len().min(512);

    let mut bad = [0u8; W];
    let mut nul = [0u8; W];

    let mut chunks = data[..check_len].chunks_exact(W);
    for chunk in &mut chunks {
        for j in 0..W {
            bad[j] += is_rejected_byte(chunk[j]);
            nul[j] |= (chunk[j] == 0) as u8;
        }
    }

    //
    // Tail padded with spaces
    //
    let rem = chunks.remainder();
    if !rem.is_empty() {
        let mut tail = [0x20u8; W];
        tail[..rem.len()].copy_from_slice(rem);
        for j in 0..W {
            bad[j] += is_rejected_byte(tail[j]);
            nul[j] |= (tail[j] == 0) as u8;
        }
    }

    let mut any_nul = 0u8;
    let mut control_count = 0usize;
    for j in 0..W {
        any_nul |= nul[j];
        control_count += bad[j] as usize;
    }

    any_nul != 0 || control_count > BINARY_CONTROL_COUNT
}
