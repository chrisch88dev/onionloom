//! Compiled prefix filters that avoid base32 encoding in the hot loop.

use std::collections::BTreeSet;

use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};

use crate::address::BASE32_ALPHABET;

/// Maximum prefix length handled without hashing the onion checksum.
pub const MAX_FAST_PREFIX_LEN: usize = 51;

/// One validated user prefix.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Pattern(String);

impl Pattern {
    /// Validates and normalizes a prefix.
    pub fn parse(value: &str) -> Result<Self> {
        let value = value.strip_suffix(".onion").unwrap_or(value);
        ensure!(!value.is_empty(), "pattern cannot be empty");
        ensure!(
            value.len() <= MAX_FAST_PREFIX_LEN,
            "pattern is too long: the CPU fast path supports at most {MAX_FAST_PREFIX_LEN} characters"
        );
        if value.bytes().any(|byte| !BASE32_ALPHABET.contains(&byte)) {
            bail!("invalid pattern {value:?}: use lowercase a-z and digits 2-7 only");
        }
        Ok(Self(value.to_owned()))
    }

    /// Returns the normalized prefix text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns the number of base32 characters.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Returns whether the prefix is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[derive(Clone, Debug)]
struct CompiledPattern {
    bytes: [u8; 32],
    masks: [u8; 32],
    last_byte: usize,
}

impl CompiledPattern {
    fn new(pattern: &Pattern) -> Self {
        let mut bytes = [0_u8; 32];
        let mut masks = [0_u8; 32];
        let mut bit_index = 0_usize;
        for character in pattern.as_str().bytes() {
            let value = u8::try_from(
                BASE32_ALPHABET
                    .iter()
                    .position(|candidate| *candidate == character)
                    .expect("validated alphabet"),
            )
            .expect("base32 index fits u8");
            for source_bit in (0..5).rev() {
                let target_byte = bit_index / 8;
                let target_bit = 7 - (bit_index % 8);
                masks[target_byte] |= 1 << target_bit;
                if value & (1 << source_bit) != 0 {
                    bytes[target_byte] |= 1 << target_bit;
                }
                bit_index += 1;
            }
        }
        Self {
            bytes,
            masks,
            last_byte: (bit_index - 1) / 8,
        }
    }

    #[inline]
    fn matches(&self, public_key: &[u8; 32]) -> bool {
        for (index, candidate_byte) in public_key.iter().enumerate().take(self.last_byte + 1) {
            if (candidate_byte & self.masks[index]) != self.bytes[index] {
                return false;
            }
        }
        true
    }
}

/// Filter implementation chosen from the normalized pattern set.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FilterStrategy {
    /// Direct masked comparison for a small pattern set.
    LinearMasks,
    /// First-character buckets before masked comparison.
    FirstCharacterBuckets,
}

impl std::fmt::Display for FilterStrategy {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LinearMasks => formatter.write_str("linear bit masks"),
            Self::FirstCharacterBuckets => formatter.write_str("first-character buckets"),
        }
    }
}

/// A normalized collection of prefix filters.
#[derive(Clone, Debug)]
pub struct FilterSet {
    patterns: Vec<Pattern>,
    compiled: Vec<CompiledPattern>,
    buckets: [Vec<usize>; 32],
    strategy: FilterStrategy,
}

impl FilterSet {
    /// Removes duplicates and patterns made redundant by a shorter prefix.
    pub fn new(patterns: Vec<Pattern>) -> Result<Self> {
        ensure!(!patterns.is_empty(), "at least one pattern is required");
        let sorted: BTreeSet<String> = patterns.into_iter().map(|pattern| pattern.0).collect();
        let mut minimal = Vec::new();
        for candidate in sorted {
            if minimal
                .iter()
                .any(|existing: &Pattern| candidate.starts_with(existing.as_str()))
            {
                continue;
            }
            minimal.push(Pattern(candidate));
        }

        let compiled: Vec<_> = minimal.iter().map(CompiledPattern::new).collect();
        let strategy = if compiled.len() <= 8 {
            FilterStrategy::LinearMasks
        } else {
            FilterStrategy::FirstCharacterBuckets
        };
        let mut buckets: [Vec<usize>; 32] = std::array::from_fn(|_| Vec::new());
        for (index, pattern) in minimal.iter().enumerate() {
            let bucket = BASE32_ALPHABET
                .iter()
                .position(|character| *character == pattern.as_str().as_bytes()[0])
                .expect("validated first character");
            buckets[bucket].push(index);
        }
        Ok(Self {
            patterns: minimal,
            compiled,
            buckets,
            strategy,
        })
    }

    /// Returns true when a public key has any requested base32 prefix.
    #[inline]
    #[must_use]
    pub fn matches(&self, public_key: &[u8; 32]) -> bool {
        match self.strategy {
            FilterStrategy::LinearMasks => self
                .compiled
                .iter()
                .any(|pattern| pattern.matches(public_key)),
            FilterStrategy::FirstCharacterBuckets => {
                let bucket = usize::from(public_key[0] >> 3);
                self.buckets[bucket]
                    .iter()
                    .any(|index| self.compiled[*index].matches(public_key))
            }
        }
    }

    /// Returns the selected strategy.
    #[must_use]
    pub fn strategy(&self) -> FilterStrategy {
        self.strategy
    }

    /// Returns normalized, non-overlapping patterns.
    #[must_use]
    pub fn patterns(&self) -> &[Pattern] {
        &self.patterns
    }

    /// Returns the exact success probability per independent candidate.
    #[must_use]
    pub fn probability(&self) -> f64 {
        self.patterns
            .iter()
            .map(|pattern| 2_f64.powi(-5 * i32::try_from(pattern.len()).unwrap_or(i32::MAX)))
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use crate::address::{BASE32_ALPHABET, onion_address, validate_address};
    use crate::keys::ExpandedSecret;

    use super::{FilterSet, MAX_FAST_PREFIX_LEN, Pattern};

    #[test]
    fn matches_encoded_public_key_without_encoding() {
        let address = "pg6mmjiyjmcrsslvykfwnntlaru7p5svn6y2ymmju6nubxndf4pscryd.onion";
        let public = validate_address(address).expect("valid spec vector");
        for prefix in [
            "p",
            "pg6mm",
            "pg6mmjiyjmcrsslvykfwnntlaru7p5svn6y2ymmju6nubxndf4p",
        ] {
            let filters = FilterSet::new(vec![Pattern::parse(prefix).expect("valid pattern")])
                .expect("filter set");
            assert!(filters.matches(&public), "{prefix}");
        }
        let filters = FilterSet::new(vec![Pattern::parse("other").expect("valid pattern")])
            .expect("filter set");
        assert!(!filters.matches(&public));
    }

    #[test]
    fn removes_overlapping_patterns() {
        let filters = FilterSet::new(vec![
            Pattern::parse("abc").expect("valid"),
            Pattern::parse("ab").expect("valid"),
            Pattern::parse("abc").expect("valid"),
        ])
        .expect("filter set");
        assert_eq!(filters.patterns(), &[Pattern::parse("ab").expect("valid")]);
    }

    #[test]
    fn packed_masks_match_every_bit_boundary() {
        let mut expanded = [0_u8; 64];
        expanded[0] = 0x18;
        expanded[31] = 0x40;
        let base = ExpandedSecret::from_bytes(expanded).expect("valid fixed test scalar");

        for offset in [0, 1, 31, 255, 65_535] {
            let public = base.with_offset(offset).expect("valid offset").public_key();
            let address = onion_address(&public);
            for length in 1..=MAX_FAST_PREFIX_LEN {
                let matching = Pattern::parse(&address[..length]).expect("valid prefix");
                assert!(
                    FilterSet::new(vec![matching])
                        .expect("filter")
                        .matches(&public),
                    "matching prefix length {length}"
                );

                let mut different = address.as_bytes()[..length].to_vec();
                let last = different[length - 1];
                let alphabet_index = BASE32_ALPHABET
                    .iter()
                    .position(|candidate| *candidate == last)
                    .expect("address character is base32");
                different[length - 1] = BASE32_ALPHABET[(alphabet_index + 1) % 32];
                let different = std::str::from_utf8(&different).expect("base32 is UTF-8");
                let rejecting = Pattern::parse(different).expect("valid prefix");
                assert!(
                    !FilterSet::new(vec![rejecting])
                        .expect("filter")
                        .matches(&public),
                    "different prefix length {length}"
                );
            }
        }
    }
}
