// Portions of this file are copied from the `bytecount` crate.
// Copyright (c) the `bytecount` crate developers.
// Licensed under the MIT License (https://opensource.org/licenses/MIT).

#![allow(unused, dead_code)]
#![allow(clippy::pedantic, clippy::all)]

//! count occurrences of a given byte, or the number of UTF-8 code points, in a
//! byte slice, fast.
//!
//! This crate has the [`count`](fn.count.html) method to count byte
//! occurrences (for example newlines) in a larger `&[u8]` slice.
//~
//! For completeness and easy comparison, the "naive" versions of both
//! count and num_chars are provided. Those are also faster if used on
//! predominantly small strings. The
//! [`naive_count_32`](fn.naive_count_32.html) method can be faster
//! still on small strings.

#![cfg_attr(feature = "generic-simd", feature(portable_simd))]
#![deny(missing_docs)]

#[cfg(not(feature = "runtime-dispatch-simd"))]
use core::mem;
#[cfg(feature = "runtime-dispatch-simd")]
use std::mem;

mod naive;
pub use naive::*;
mod integer_simd;

#[cfg(any(
    all(
        feature = "runtime-dispatch-simd",
        any(target_arch = "x86", target_arch = "x86_64")
    ),
    all(target_arch = "aarch64", target_endian = "little"),
    target_arch = "wasm32",
    feature = "generic-simd"
))]
mod simd;

/// Count occurrences of a byte in a slice of bytes, fast
pub fn count(haystack: &[u8], needle: u8) -> usize {
    if haystack.len() >= 32 {
        #[cfg(all(feature = "runtime-dispatch-simd", target_arch = "x86_64"))]
        {
            if is_x86_feature_detected!("avx2") {
                unsafe {
                    return simd::x86_avx2::chunk_count(haystack, needle);
                }
            }
        }

        #[cfg(feature = "generic-simd")]
        return simd::generic::chunk_count(haystack, needle);
    }

    if haystack.len() >= 16 {
        #[cfg(all(
            feature = "runtime-dispatch-simd",
            any(target_arch = "x86", target_arch = "x86_64"),
            not(feature = "generic-simd")
        ))]
        {
            if is_x86_feature_detected!("sse2") {
                unsafe {
                    return simd::x86_sse2::chunk_count(haystack, needle);
                }
            }
        }
        #[cfg(all(
            target_arch = "aarch64",
            target_endian = "little",
            not(feature = "generic-simd")
        ))]
        {
            unsafe {
                return simd::aarch64::chunk_count(haystack, needle);
            }
        }

        #[cfg(target_arch = "wasm32")]
        {
            unsafe {
                return simd::wasm::chunk_count(haystack, needle);
            }
        }
    }

    if haystack.len() >= mem::size_of::<usize>() {
        return integer_simd::chunk_count(haystack, needle);
    }

    naive_count(haystack, needle)
}

/// Count the number of UTF-8 encoded Unicode codepoints in a slice of bytes, fast
///
/// This function is safe to use on any byte array, valid UTF-8 or not,
/// but the output is only meaningful for well-formed UTF-8.
pub fn num_chars(utf8_chars: &[u8]) -> usize {
    if utf8_chars.len() >= 32 {
        #[cfg(all(feature = "runtime-dispatch-simd", target_arch = "x86_64"))]
        {
            if is_x86_feature_detected!("avx2") {
                unsafe {
                    return simd::x86_avx2::chunk_num_chars(utf8_chars);
                }
            }
        }

        #[cfg(feature = "generic-simd")]
        return simd::generic::chunk_num_chars(utf8_chars);
    }

    if utf8_chars.len() >= 16 {
        #[cfg(all(
            feature = "runtime-dispatch-simd",
            any(target_arch = "x86", target_arch = "x86_64"),
            not(feature = "generic-simd")
        ))]
        {
            if is_x86_feature_detected!("sse2") {
                unsafe {
                    return simd::x86_sse2::chunk_num_chars(utf8_chars);
                }
            }
        }
        #[cfg(all(
            target_arch = "aarch64",
            target_endian = "little",
            not(feature = "generic-simd")
        ))]
        {
            unsafe {
                return simd::aarch64::chunk_num_chars(utf8_chars);
            }
        }

        #[cfg(target_arch = "wasm32")]
        {
            unsafe {
                return simd::wasm::chunk_num_chars(utf8_chars);
            }
        }
    }

    if utf8_chars.len() >= mem::size_of::<usize>() {
        return integer_simd::chunk_num_chars(utf8_chars);
    }

    naive_num_chars(utf8_chars)
}
