//! Required-literal extraction over a parsed regex Hir.
//!
//! The core guarantee this module provides: every byte string returned by
//! `required_literals` is a substring of *every* string the pattern can
//! match. This is the soundness property the fragment cache depends on --
//! get it wrong and rawgrep silently skips blocks that contain real matches.
//!
//! Deliberately conservative in a few places (documented at each site)
//! rather than clever, because a missed optimization costs throughput and
//! a wrong optimization costs correctness. Those are not the same severity
//! of bug.

use regex_syntax::hir::{Class, Hir, HirKind};

/// Extract byte strings that must appear verbatim, at least once, in any
/// text the given Hir can match.
///
/// @Incomplete: a fully general multi-string longest-common-substring
/// search across every possible way branches could align (the fold in
/// `alternation_literals` below is a sound but greedy approximation of
/// that), and unrolling a `{min,max}` repetition of a *non-literal* unit
/// into multiple independent copies (each copy still contributes its own
/// required parts once, since copies of a variable unit are not
/// guaranteed adjacent -- see the doc comment on the Repetition arm).
pub fn required_literals(hir: &Hir) -> Vec<Vec<u8>> {
    match hir.kind() {
        HirKind::Empty => Vec::new(),
        HirKind::Literal(lit) => vec![lit.0.to_vec()],

        // A multi-element class matches one of several bytes/chars, so no
        // specific byte is guaranteed. (Single-element classes are already
        // folded into HirKind::Literal by regex-syntax before we see them,
        // so there is no singleton case to special-case here.)
        HirKind::Class(_) => Vec::new(),

        // Zero-width assertions (^, $, \b, ...) contribute no matched bytes.
        HirKind::Look(_) => Vec::new(),

        HirKind::Repetition(rep) => {
            if rep.min == 0 {
                Vec::new()
            } else if let Some(unit) = literal_bytes(&rep.sub) {
                //
                // The repeated unit is a pure literal with zero
                // variability, so unlike the general case below, the
                // first `min` copies are guaranteed to sit back-to-back
                // regardless of how many times (up to max) the repeat
                // actually fires: '(?:ab){3,5}' always contains 'ababab'
                // as a leading substring, whether it repeated 3, 4, or 5
                // times.
                //
                // This does NOT generalize when the unit has any
                // internal variability (a class, an alternation) --
                // see `repetition_of_nonliteral_does_not_merge_across_copies`
                // in the tests for why 'aa' would be an unsound claim for '(?:a[xy]){2,3}'.
                //
                vec![repeat_bytes(&unit, rep.min)]
            } else {
                required_literals(&rep.sub)
            }
        }

        HirKind::Capture(cap) => required_literals(&cap.sub),
        HirKind::Concat(subs) => concat_literals(subs),
        HirKind::Alternation(subs) => alternation_literals(subs),
    }
}

/// A literal segment required in every match, together with whether it
/// needs case-folding to actually find it in raw file bytes.
///
/// When `case_insensitive` is `false`, `bytes` are the exact bytes any
/// match must contain verbatim -- identical in meaning to a plain
/// `Vec<u8>` from `required_literals`.
///
/// When `case_insensitive` is `true`, `bytes` is the ASCII-lowercased
/// canonical form; the real matched text has *some* casing of these
/// bytes, not necessarily this exact one.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct LiteralPart {
    pub bytes: Vec<u8>,
    pub case_insensitive: bool,
}

/// Case-aware counterpart to `required_literals`. Same soundness
/// guarantee (every returned part is a substring of every match, up to
/// the case-folding caveat on `LiteralPart` above), same conservative
/// philosophy: a class that isn't recognizably a plain ASCII case-fold
/// pair contributes nothing rather than guessing.
///
/// @Incomplete: comparing a case-sensitive candidate against a
/// case-insensitive one during alternation intersection (folding one
/// side just for that comparison is possible but adds real complexity
/// for what should be a rare pattern shape, so for now the two kinds
/// simply don't intersect with each other -- sound, just misses that
/// case); and any Unicode case-fold orbit wider than the simple ASCII
/// upper/lower pair (Kelvin sign folding to 'k' being the canonical
/// example -- deliberately narrow, since real-world case-insensitive
/// grep is overwhelmingly ASCII).
pub fn required_literal_parts(hir: &Hir) -> Vec<LiteralPart> {
    match hir.kind() {
        HirKind::Empty => Vec::new(),

        HirKind::Literal(lit) => vec![LiteralPart {
            bytes: lit.0.to_vec(),
            case_insensitive: false,
        }],

        HirKind::Class(class) => match ascii_case_fold_byte(class) {
            Some(b) => vec![LiteralPart { bytes: vec![b], case_insensitive: true }],
            None => Vec::new(),
        },

        HirKind::Look(_) => Vec::new(),

        HirKind::Repetition(rep) => {
            if rep.min == 0 {
                Vec::new()
            } else if let Some(unit) = literal_part_bytes(&rep.sub) {
                vec![LiteralPart {
                    bytes: repeat_bytes(&unit.bytes, rep.min),
                    case_insensitive: unit.case_insensitive,
                }]
            } else {
                required_literal_parts(&rep.sub)
            }
        }

        HirKind::Capture(cap) => required_literal_parts(&cap.sub),
        HirKind::Concat(subs) => concat_literal_parts(subs),
        HirKind::Alternation(subs) => alternation_literal_parts(subs),
    }
}

/// If `class` is exactly the ASCII case-fold pair of one letter -- two
/// single-codepoint ranges, one uppercase ASCII letter and its lowercase
/// counterpart, nothing else -- returns that letter's lowercase ASCII
/// byte. Rejects anything wider (like the 3-way Kelvin-sign fold for
/// 'k'), which falls back to contributing nothing: always sound.
fn ascii_case_fold_byte(class: &Class) -> Option<u8> {
    fn fold_pair(a_lo: u32, a_hi: u32, b_lo: u32, b_hi: u32) -> Option<u8> {
        if a_lo != a_hi || b_lo != b_hi || a_lo > 0x7f || b_lo > 0x7f {
            return None;
        }

        let a = a_lo as u8;
        let b = b_lo as u8;

        if !a.is_ascii_alphabetic() || !b.is_ascii_alphabetic() || a == b {
            return None;
        }

        if !a.eq_ignore_ascii_case(&b) {
            return None;
        }

        Some(a.to_ascii_lowercase())
    }

    match class {
        Class::Unicode(u) => {
            let ranges = u.ranges();
            if ranges.len() != 2 { return None; }

            fold_pair(
                ranges[0].start() as u32, ranges[0].end() as u32,
                ranges[1].start() as u32, ranges[1].end() as u32,
            )
        }

        Class::Bytes(b) => {
            let ranges = b.ranges();
            if ranges.len() != 2 { return None; }

            fold_pair(
                ranges[0].start() as u32, ranges[0].end() as u32,
                ranges[1].start() as u32, ranges[1].end() as u32,
            )
        }
    }
}

/// Case-aware counterpart to `literal_bytes`: unwraps a Hir that reduces
/// to exactly one literal run, now also allowing ASCII case-fold classes to
/// participate in the run. If any component of the run was
/// case-insensitive, the whole merged run is marked case-insensitive --
/// once merged we no longer track case-sensitivity per byte, so this is
/// the conservative direction: the caller folds a little more than
/// strictly necessary rather than a little less (which would be
/// unsound).
fn literal_part_bytes(hir: &Hir) -> Option<LiteralPart> {
    match hir.kind() {
        HirKind::Literal(lit) => Some(LiteralPart { bytes: lit.0.to_vec(), case_insensitive: false }),

        HirKind::Class(class) => ascii_case_fold_byte(class)
            .map(|b| LiteralPart { bytes: vec![b], case_insensitive: true }),

        HirKind::Capture(cap) => literal_part_bytes(&cap.sub),

        HirKind::Concat(subs) => {
            let mut bytes = Vec::new();
            let mut any_ci = false;
            for sub in subs {
                let part = literal_part_bytes(sub)?;
                bytes.extend_from_slice(&part.bytes);
                any_ci |= part.case_insensitive;
            }
            Some(LiteralPart { bytes, case_insensitive: any_ci })
        }

        _ => None,
    }
}

#[inline]
fn concat_literal_parts(subs: &[Hir]) -> Vec<LiteralPart> {
    let mut parts = Vec::new();
    let mut run_bytes: Vec<u8> = Vec::new();
    let mut run_ci = false;

    for sub in subs {
        if let Some(part) = literal_part_bytes(sub) {
            run_bytes.extend_from_slice(&part.bytes);
            run_ci |= part.case_insensitive;

        } else {
            if !run_bytes.is_empty() {
                parts.push(LiteralPart {
                    bytes: std::mem::take(&mut run_bytes),
                    case_insensitive: run_ci
                });
                run_ci = false;
            }

            parts.extend(required_literal_parts(sub));
        }
    }

    if !run_bytes.is_empty() {
        parts.push(LiteralPart { bytes: run_bytes, case_insensitive: run_ci });
    }

    parts
}

fn alternation_literal_parts(subs: &[Hir]) -> Vec<LiteralPart> {
    const MIN_ALTERNATION_SUBSTRING:  usize = 2;
    const MAX_ALTERNATION_CANDIDATES: usize = 32;

    let mut branches = subs.iter().map(required_literal_parts);
    let mut candidates = match branches.next() {
        Some(parts) => parts,
        None => return Vec::new(),
    };

    for parts in branches {
        if candidates.is_empty() {
            break;
        }

        let mut next = Vec::new();
        for c in &candidates {
            for p in &parts {
                //
                // Not attempting cross-comparison between a case-sensitive
                // candidate and a case-insensitive one; see the doc
                // comment on `required_literal_parts`.
                //
                if c.case_insensitive != p.case_insensitive {
                    continue;
                }

                for bytes in common_substrings(&c.bytes, &p.bytes, MIN_ALTERNATION_SUBSTRING) {
                    next.push(LiteralPart { bytes, case_insensitive: c.case_insensitive });
                }
            }
        }

        next.sort();
        next.dedup();

        if next.len() > MAX_ALTERNATION_CANDIDATES {
            next.sort_by_key(|c| std::cmp::Reverse(c.bytes.len()));
            next.truncate(MAX_ALTERNATION_CANDIDATES);
        }

        candidates = next;
    }

    candidates
}

/// Bounds how large a literal we'll materialize by repeating a unit
/// 'min' times, so a pathological pattern like "a{100000000}" can't force
/// an unbounded allocation here. Repeating fewer than 'min' times is
/// still a fully sound (if less specific) claim: the real match always
/// contains at least 'min' copies, and any prefix of that guaranteed run
/// is equally guaranteed. Always produces at least one copy when 'min >= 1'
/// and the unit is nonempty.
pub const MAX_REPEATED_LITERAL_BYTES: usize = 4096;

fn repeat_bytes(unit: &[u8], min: u32) -> Vec<u8> {
    if unit.is_empty() {
        return Vec::new();
    }

    let max_copies = (MAX_REPEATED_LITERAL_BYTES / unit.len()).max(1) as u32;
    let copies = min.min(max_copies);

    let mut out = Vec::with_capacity(unit.len() * copies as usize);
    for _ in 0..copies {
        out.extend_from_slice(unit);
    }

    out
}

/// Unwraps a Hir that reduces to exactly one known literal byte string,
/// with no other possibility -- through capturing-group wrappers
/// (a non-capturing group produces no Hir node at all, so only Capture needs
/// unwrapping) and through a Concat all of whose own children are, in
/// turn, fully literal (so `((a)(b))c` collapses all the way to 'abc').
///
/// Anything structurally uncertain anywhere in the tree returns None, so
/// callers never overclaim.
#[inline]
pub(crate) fn literal_bytes(hir: &Hir) -> Option<Vec<u8>> {
    match hir.kind() {
        HirKind::Literal(lit) => Some(lit.0.to_vec()),

        HirKind::Capture(cap) => literal_bytes(&cap.sub),

        HirKind::Concat(subs) => {
            let mut buf = Vec::new();
            for sub in subs {
                buf.extend_from_slice(&literal_bytes(sub)?);
            }
            Some(buf)
        }

        _ => None,
    }
}

/// True only when `hir` is (optionally wrapped in capturing groups) a
/// top-level alternation all of whose branches collapse to an exact
/// literal via `literal_bytes`. This is a stronger claim than anything
/// `required_literals` makes: not "these substrings are required
/// somewhere," but "the whole pattern *is* exactly one of these
/// strings." Any branch that isn't a pure literal invalidates the whole
/// pattern for this fast path -- we bail entirely rather than dropping
/// that branch, since dropping a branch would make the resulting
/// matcher under-match.
#[inline]
pub(crate) fn as_exact_alternation(hir: &Hir) -> Option<Vec<Vec<u8>>> {
    match hir.kind() {
        HirKind::Capture(cap) => as_exact_alternation(&cap.sub),
        HirKind::Alternation(subs) => subs.iter().map(literal_bytes).collect(),
        _ => None,
    }
}

/// Concatenation: merge adjacent literal children (through capture-group
/// wrappers) into contiguous byte runs, since they are guaranteed adjacent
/// in any match; anything else breaks the run and contributes its own
/// required parts independently.
fn concat_literals(subs: &[Hir]) -> Vec<Vec<u8>> {
    let mut parts = Vec::new();
    let mut run: Vec<u8> = Vec::new();
    for sub in subs {
        if let Some(bytes) = literal_bytes(sub) {
            run.extend_from_slice(&bytes);
        } else {
            if !run.is_empty() {
                parts.push(std::mem::take(&mut run));
            }
            parts.extend(required_literals(sub));
        }
    }

    if !run.is_empty() {
        parts.push(run);
    }

    parts
}

/// Alternation: in general no single required literal survives a branch
/// choice, but any byte run that shows up in *every* branch's own
/// required literals is still guaranteed no matter which branch actually
/// matched. This subsumes the shared-prefix and shared-suffix cases
/// (`prefix(foo|bar)suffix`) as special cases of 'common substring', and
/// also catches shared content in the *middle* of otherwise-unrelated
/// branches (`foo[a-z]123bar|baz[a-z]123qux` both requiring "123") that
/// prefix/suffix-only matching cannot see.
///
/// Implementation is a greedy pairwise fold, not an exhaustive multi-way
/// common-substring search: seed the candidate set from the first
/// branch's own required parts, then repeatedly intersect against each
/// remaining branch by finding maximal common substrings between every
/// current candidate and every part of that branch. Each surviving
/// candidate is, by construction, an exact substring of some required
/// part of every branch processed so far, so this is sound at every
/// step; it is not guaranteed to find every common substring an
/// exhaustive search would (a different fold order can occasionally
/// surface a different, non-overlapping set), which is the same
/// "correct but not maximal" tradeoff as everything else here. An
/// alternation with an empty branch (`foo|`) correctly collapses to
/// nothing, since intersecting against that branch's empty part list
/// empties the candidate set immediately.
fn alternation_literals(subs: &[Hir]) -> Vec<Vec<u8>> {
    const MIN_ALTERNATION_SUBSTRING:  usize = 2;
    const MAX_ALTERNATION_CANDIDATES: usize = 32;

    let mut branches = subs.iter().map(required_literals);
    let mut candidates = match branches.next() {
        Some(parts) => parts,
        None => return Vec::new(),
    };

    for parts in branches {
        if candidates.is_empty() {
            break;
        }

        let mut next = Vec::new();
        for c in &candidates {
            for p in &parts {
                next.extend(common_substrings(c, p, MIN_ALTERNATION_SUBSTRING));
            }
        }

        next.sort();
        next.dedup();

        //
        // A wide alternation with many required parts per branch
        // could otherwise make the candidate set grow every fold
        // step.
        //
        // Keeping the longest candidates is a reasonable bias --
        // they are the more useful fragments anyway.
        //
        if next.len() > MAX_ALTERNATION_CANDIDATES {
            next.sort_by_key(|c| std::cmp::Reverse(c.len()));
            next.truncate(MAX_ALTERNATION_CANDIDATES);
        }

        candidates = next;
    }

    candidates
}

// @Note:
//
// Capped at `MAX_LEN` per input since this is O(len(a) * len(b)) time and space.
//
// Regex literal parts are small in reality, but nothing
// here should let a pathological pattern turn this quadratic.
fn common_substrings(a: &[u8], b: &[u8], min_len: usize) -> Vec<Vec<u8>> {
    const MAX_LEN: usize = 256;
    if a.is_empty() || b.is_empty() || a.len() > MAX_LEN || b.len() > MAX_LEN {
        return Vec::new();
    }

    let n = a.len();
    let m = b.len();

    let mut dp = vec![vec![0usize; m + 1]; n + 1];
    let mut results = Vec::new();

    for i in 1..=n {
        for j in 1..=m {
            if a[i - 1] != b[j - 1] {
                continue;
            }

            dp[i][j] = dp[i - 1][j - 1] + 1;

            let at_end = i == n || j == m;
            let can_extend = !at_end && a[i] == b[j];
            if !can_extend && dp[i][j] >= min_len {
                results.push(a[i - dp[i][j]..i].to_owned());  // @Speed @Memory
            }
        }
    }

    results
}

pub fn extract_regex_literals(
    pattern: &str, case_insensitive: bool
) -> Option<(Vec<u32>, usize, bool, bool)> {
    use crate::logger::*;
    use crate::fragments::{MIN_FRAGMENT_LEN, extract_pattern_fragments_with_len, select_fragment_len};

    use nohash_hasher::IntSet;

    let hir = regex_syntax::ParserBuilder::new()
        .case_insensitive(case_insensitive)
        .build()
        .parse(pattern)
        .ok()?;

    // Always the case-fold-aware extractor -- for a pattern with no
    // case-insensitivity anywhere, this behaves identically to
    // required_literals, since ascii_case_fold_byte only ever fires on a
    // genuine case-fold class, which only exists if some form of
    // case-insensitivity was actually active during parsing.
    let literal_parts = required_literal_parts(&hir);

    // The REAL signal for whether these fragments need folded hashing --
    // derived from what actually got extracted, not from the incoming
    // `case_insensitive` argument, which only reflects the parser's
    // default and can disagree with the pattern's own inline syntax.
    let needs_folding = literal_parts.iter().any(|p| p.case_insensitive);

    let mut parts: Vec<Vec<u8>> = if needs_folding {
        literal_parts.into_iter().map(|p| p.bytes.to_ascii_lowercase()).collect()
    } else {
        literal_parts.into_iter().map(|p| p.bytes).collect()
    };

    //
    // All parts' lengths must be >= MIN_FRAGMENT_LEN
    //
    parts.retain(|p| p.len() >= MIN_FRAGMENT_LEN);
    if parts.is_empty() {
        return None;
    }

    if log_enabled() {
        crate::debug!(
            "Extracted literals from regex: [{}]",
            parts.iter().map(|v| String::from_utf8_lossy(v)).collect::<Vec<_>>().join(", ")
        );
    }

    let fragment_len = select_fragment_len(parts.iter().map(|p| p.as_slice()))?;

    let single_literal = parts.len() == 1;

    let mut all_fragments = IntSet::default();
    for part in &parts {
        let frags = extract_pattern_fragments_with_len(part, fragment_len);
        all_fragments.extend(frags);
    }

    Some((all_fragments.into_iter().collect(), fragment_len, needs_folding, single_literal))
}
