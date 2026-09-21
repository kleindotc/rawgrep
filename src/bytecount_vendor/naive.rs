/// Count up to `(2^32)-1` occurrences of a byte in a slice
/// of bytes, simple
pub fn naive_count_32(haystack: &[u8], needle: u8) -> usize {
    haystack.iter().fold(0, |n, c| n + (*c == needle) as u32) as usize
}

/// Count occurrences of a byte in a slice of bytes, simple
pub fn naive_count(utf8_chars: &[u8], needle: u8) -> usize {
    utf8_chars
        .iter()
        .fold(0, |n, c| n + (*c == needle) as usize)
}

/// Count the number of UTF-8 encoded Unicode codepoints in a slice of bytes, simple
///
/// This function is safe to use on any byte array, valid UTF-8 or not,
/// but the output is only meaningful for well-formed UTF-8.
pub fn naive_num_chars(utf8_chars: &[u8]) -> usize {
    utf8_chars
        .iter()
        .filter(|&&byte| (byte >> 6) != 0b10)
        .count()
}
