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

const BYTE_CLASS: [bool; 256] = {
    let mut table = [false; 256];
    let mut i = 0;
    while i < 256 {
        table[i] = matches!(
            i as u8,
            0x09 | 0x0A | 0x0D | 0x20..=0x7E | 0x80..=0xFF
        );
        i += 1;
    }
    table
};

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

    // A declared encoding overrides the heuristic entirely -- check this
    // before anything else, since UTF-16/32 content legitimately contains
    // nulls that the check below would otherwise misread as binary.
    if detect_byte_order_leading_mark_len(data).is_some() {
        return false;
    }

    let check_len = data.len().min(512);
    if memchr::memchr(0, &data[..check_len]).is_some() {
        return true;
    }

    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("sse2") {
            return unsafe { is_binary_chunk_simd_sse2(data) };
        }
    }

    is_binary_chunk_(data)
}

/// # Safety
/// Caller's machine supports SSE2
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse2")]
pub unsafe fn is_binary_chunk_simd_sse2(data: &[u8]) -> bool {
    use std::arch::x86_64::*;

    let check_len = data.len().min(512);
    let mut control_count = 0;

    let chunks = check_len / 16;
    let ptr = data.as_ptr();

    // Create masks for allowed control chars: \t (0x09), \n (0x0A), \r (0x0D)
    let tab   = _mm_set1_epi8(0x09);
    let lf    = _mm_set1_epi8(0x0A);
    let cr    = _mm_set1_epi8(0x0D);
    let space = _mm_set1_epi8(0x20);

    // _mm_cmplt_epi8 is a SIGNED compare. Bytes 0x80..=0xFF are negative as
    // i8, so without biasing they'd always compare "< 0x20" -- miscounting
    // every high-bit byte (multi-byte UTF-8 continuation bytes, etc.) as a
    // bad control char, even though BYTE_CLASS explicitly allows 0x80..=0xFF.
    // XOR-ing both operands with 0x80 turns this into an unsigned compare.
    let sign_flip = _mm_set1_epi8(-128i8); // 0x80
    let space_b   = _mm_xor_si128(space, sign_flip);

    for i in 0..chunks {
        use crate::worker::BINARY_CONTROL_COUNT;

        let chunk = unsafe { _mm_loadu_si128(ptr.add(i * 16) as *const __m128i) };
        let chunk_b = _mm_xor_si128(chunk, sign_flip);

        // Find bytes < 0x20 (potential control characters)
        let below_space = _mm_cmplt_epi8(chunk_b, space_b);

        // Exclude allowed control chars: tab, LF, CR
        let is_tab = _mm_cmpeq_epi8(chunk, tab);
        let is_lf  = _mm_cmpeq_epi8(chunk, lf);
        let is_cr  = _mm_cmpeq_epi8(chunk, cr);

        // Combine: allowed = tab | lf | cr
        let allowed = _mm_or_si128(_mm_or_si128(is_tab, is_lf), is_cr);

        // Bad control chars = below_space AND NOT allowed
        let bad_controls = _mm_andnot_si128(allowed, below_space);

        let mask = _mm_movemask_epi8(bad_controls) as u32;
        control_count += mask.count_ones() as usize;

        if control_count > BINARY_CONTROL_COUNT {
            return true;
        }
    }

    // Handle remaining bytes
    for &byte in &data[chunks * 16..check_len] {
        if !BYTE_CLASS[byte as usize] {
            control_count += 1;
            if control_count > BINARY_CONTROL_COUNT {
                return true;
            }
        }
    }

    false
}

#[inline]
fn is_binary_chunk_(data: &[u8]) -> bool {
    let check_len = data.len().min(512);
    let mut control_count = 0;

    let mut i = 0;
    while i + 4 <= check_len {
        control_count += !BYTE_CLASS[data[i] as usize] as usize;
        control_count += !BYTE_CLASS[data[i+1] as usize] as usize;
        control_count += !BYTE_CLASS[data[i+2] as usize] as usize;
        control_count += !BYTE_CLASS[data[i+3] as usize] as usize;

        if control_count > BINARY_CONTROL_COUNT {
            return true;
        }
        i += 4;
    }

    // Handle remaining bytes
    while i < check_len {
        control_count += !BYTE_CLASS[data[i] as usize] as usize;
        if control_count > BINARY_CONTROL_COUNT {
            return true;
        }
        i += 1;
    }

    false
}
