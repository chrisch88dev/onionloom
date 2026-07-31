//! Safe prime-field arithmetic for paired-coordinate CPU search.

#![allow(
    clippy::cast_possible_truncation,
    reason = "low limbs intentionally take the low half of bounded u128 accumulators"
)]

use std::cmp::Ordering;

const MODULUS: [u64; 4] = [
    0xffff_ffff_ffff_ffed,
    0xffff_ffff_ffff_ffff,
    0xffff_ffff_ffff_ffff,
    0x7fff_ffff_ffff_ffff,
];

/// An element of the field modulo `2^255 - 19`.
///
/// Limbs retain a weakly reduced representative below `2^255`. Deferring the
/// final conditional subtraction avoids canonicalizing every intermediate
/// value in the candidate hot loop.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct FieldElement([u64; 4]);

impl PartialEq for FieldElement {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        Self::canonicalize(self.0).0 == Self::canonicalize(other.0).0
    }
}

impl Eq for FieldElement {}

impl FieldElement {
    pub(super) const ZERO: Self = Self([0, 0, 0, 0]);
    pub(super) const ONE: Self = Self([1, 0, 0, 0]);

    #[inline]
    pub(super) fn from_bytes(mut bytes: [u8; 32]) -> Self {
        bytes[31] &= 0x7f;
        let mut limbs = [0_u64; 4];
        for (limb, chunk) in limbs.iter_mut().zip(bytes.chunks_exact(8)) {
            *limb = u64::from_le_bytes(chunk.try_into().expect("eight-byte field limb"));
        }
        Self::canonicalize(limbs)
    }

    #[inline]
    pub(super) fn to_bytes(self) -> [u8; 32] {
        let canonical = Self::canonicalize(self.0);
        let mut output = [0_u8; 32];
        for (index, limb) in canonical.0.iter().enumerate() {
            output[index * 8..index * 8 + 8].copy_from_slice(&limb.to_le_bytes());
        }
        output
    }

    #[inline]
    pub(super) fn add(self, right: Self) -> Self {
        let mut output = [0_u64; 4];
        let mut carry = 0_u64;
        for (index, limb) in output.iter_mut().enumerate() {
            let sum = u128::from(self.0[index]) + u128::from(right.0[index]) + u128::from(carry);
            *limb = sum as u64;
            carry = (sum >> 64) as u64;
        }
        debug_assert_eq!(carry, 0);
        Self::reduce_255(output)
    }

    #[inline]
    pub(super) fn subtract(self, right: Self) -> Self {
        let (mut output, borrow) = subtract_limbs(self.0, right.0);
        if borrow != 0 {
            let (restored, _) = add_limbs(output, MODULUS);
            output = restored;
        }
        Self(output)
    }

    #[inline]
    pub(super) fn negate(self) -> Self {
        Self::ZERO.subtract(self)
    }

    #[inline]
    pub(super) fn multiply(self, right: Self) -> Self {
        let mut product = [0_u64; 8];
        for left_index in 0..4 {
            let mut carry = 0_u64;
            for right_index in 0..4 {
                let output_index = left_index + right_index;
                let accumulated = u128::from(self.0[left_index]) * u128::from(right.0[right_index])
                    + u128::from(product[output_index])
                    + u128::from(carry);
                product[output_index] = accumulated as u64;
                carry = (accumulated >> 64) as u64;
            }
            debug_assert_eq!(product[left_index + 4], 0);
            product[left_index + 4] = carry;
        }

        Self::reduce_product(product)
    }

    #[inline]
    pub(super) fn square(self) -> Self {
        let limbs = self.0;
        let mut product = [0_u64; 8];
        add_product(&mut product, 0, limbs[0], limbs[0], false);
        add_product(&mut product, 1, limbs[0], limbs[1], true);
        add_product(&mut product, 2, limbs[0], limbs[2], true);
        add_product(&mut product, 2, limbs[1], limbs[1], false);
        add_product(&mut product, 3, limbs[0], limbs[3], true);
        add_product(&mut product, 3, limbs[1], limbs[2], true);
        add_product(&mut product, 4, limbs[1], limbs[3], true);
        add_product(&mut product, 4, limbs[2], limbs[2], false);
        add_product(&mut product, 5, limbs[2], limbs[3], true);
        add_product(&mut product, 6, limbs[3], limbs[3], false);
        Self::reduce_product(product)
    }

    #[inline]
    fn reduce_product(product: [u64; 8]) -> Self {
        let mut reduced = [product[0], product[1], product[2], product[3], 0_u64];
        let mut carry = 0_u64;
        for index in 0..4 {
            let accumulated = u128::from(product[index + 4]) * 38
                + u128::from(reduced[index])
                + u128::from(carry);
            reduced[index] = accumulated as u64;
            carry = (accumulated >> 64) as u64;
        }
        reduced[4] = carry;

        while reduced[4] != 0 {
            let high = reduced[4];
            reduced[4] = 0;
            let accumulated = u128::from(reduced[0]) + u128::from(high) * 38;
            reduced[0] = accumulated as u64;
            let mut carry = (accumulated >> 64) as u64;
            for limb in &mut reduced[1..4] {
                if carry == 0 {
                    break;
                }
                let (sum, overflow) = limb.overflowing_add(carry);
                *limb = sum;
                carry = u64::from(overflow);
            }
            reduced[4] = carry;
        }

        Self::reduce_255([reduced[0], reduced[1], reduced[2], reduced[3]])
    }

    pub(super) fn invert(self) -> Self {
        let value_2 = self.square();
        let mut value_9 = value_2.pow2k(2);
        value_9 = self.multiply(value_9);
        let value_11 = value_2.multiply(value_9);
        let value_31 = value_9.multiply(value_11.square());
        let value_10_bits = value_31.pow2k(5).multiply(value_31);
        let value_20_bits = value_10_bits.pow2k(10).multiply(value_10_bits);
        let value_40_bits = value_20_bits.pow2k(20).multiply(value_20_bits);
        let value_50_bits = value_40_bits.pow2k(10).multiply(value_10_bits);
        let value_100_bits = value_50_bits.pow2k(50).multiply(value_50_bits);
        let value_200_bits = value_100_bits.pow2k(100).multiply(value_100_bits);
        let value_250_bits = value_200_bits.pow2k(50).multiply(value_50_bits);
        value_250_bits.pow2k(5).multiply(value_11)
    }

    pub(super) fn sqrt_power(self) -> Self {
        self.pow22523().multiply(self)
    }

    pub(super) fn is_zero(self) -> bool {
        self.0 == Self::ZERO.0 || self.0 == MODULUS
    }

    pub(super) fn is_odd(self) -> bool {
        Self::canonicalize(self.0).0[0] & 1 != 0
    }

    #[inline]
    fn pow2k(self, power: usize) -> Self {
        let mut output = self.square();
        for _ in 1..power {
            output = output.square();
        }
        output
    }

    fn pow22523(self) -> Self {
        let value_2 = self.square();
        let mut value_9 = value_2.pow2k(2);
        value_9 = self.multiply(value_9);
        let value_11 = value_2.multiply(value_9);
        let value_31 = value_9.multiply(value_11.square());
        let value_10_bits = value_31.pow2k(5).multiply(value_31);
        let value_20_bits = value_10_bits.pow2k(10).multiply(value_10_bits);
        let value_40_bits = value_20_bits.pow2k(20).multiply(value_20_bits);
        let value_50_bits = value_40_bits.pow2k(10).multiply(value_10_bits);
        let value_100_bits = value_50_bits.pow2k(50).multiply(value_50_bits);
        let value_200_bits = value_100_bits.pow2k(100).multiply(value_100_bits);
        let value_250_bits = value_200_bits.pow2k(50).multiply(value_50_bits);
        value_250_bits.pow2k(2).multiply(self)
    }

    #[inline]
    fn reduce_255(mut limbs: [u64; 4]) -> Self {
        loop {
            let high = limbs[3] >> 63;
            limbs[3] &= 0x7fff_ffff_ffff_ffff;
            if high == 0 {
                break;
            }
            add_small(&mut limbs, high * 19);
        }
        Self(limbs)
    }

    #[inline]
    fn canonicalize(limbs: [u64; 4]) -> Self {
        if compare_limbs(limbs, MODULUS) == Ordering::Less {
            Self(limbs)
        } else {
            let (reduced, borrow) = subtract_limbs(limbs, MODULUS);
            debug_assert_eq!(borrow, 0);
            Self(reduced)
        }
    }
}

/// Adds one diagonal or doubled cross product to a 512-bit square.
#[allow(
    clippy::inline_always,
    reason = "constant-index square terms must specialize without call overhead"
)]
#[inline(always)]
fn add_product(product: &mut [u64; 8], index: usize, left: u64, right: u64, double: bool) {
    let term = u128::from(left) * u128::from(right);
    let term_low = term as u64;
    let term_high = (term >> 64) as u64;
    let (low, high, top) = if double {
        (
            term_low << 1,
            (term_high << 1) | (term_low >> 63),
            term_high >> 63,
        )
    } else {
        (term_low, term_high, 0)
    };

    let low_sum = u128::from(product[index]) + u128::from(low);
    product[index] = low_sum as u64;
    let high_sum = u128::from(product[index + 1]) + u128::from(high) + (low_sum >> 64);
    product[index + 1] = high_sum as u64;
    let mut carry = (high_sum >> 64) as u64 + top;
    for limb in &mut product[index + 2..] {
        if carry == 0 {
            return;
        }
        let (sum, overflow) = limb.overflowing_add(carry);
        *limb = sum;
        carry = u64::from(overflow);
    }
    debug_assert_eq!(carry, 0);
}

#[inline]
fn add_small(limbs: &mut [u64; 4], value: u64) {
    let (sum, mut overflow) = limbs[0].overflowing_add(value);
    limbs[0] = sum;
    for limb in &mut limbs[1..] {
        if !overflow {
            return;
        }
        let (sum, next) = limb.overflowing_add(1);
        *limb = sum;
        overflow = next;
    }
}

#[inline]
fn add_limbs(left: [u64; 4], right: [u64; 4]) -> ([u64; 4], u64) {
    let mut output = [0_u64; 4];
    let mut carry = 0_u64;
    for index in 0..4 {
        let sum = u128::from(left[index]) + u128::from(right[index]) + u128::from(carry);
        output[index] = sum as u64;
        carry = (sum >> 64) as u64;
    }
    (output, carry)
}

#[inline]
fn subtract_limbs(left: [u64; 4], right: [u64; 4]) -> ([u64; 4], u64) {
    let mut output = [0_u64; 4];
    let mut borrow = 0_u64;
    for index in 0..4 {
        let (difference, first) = left[index].overflowing_sub(right[index]);
        let (difference, second) = difference.overflowing_sub(borrow);
        output[index] = difference;
        borrow = u64::from(first | second);
    }
    (output, borrow)
}

#[inline]
fn compare_limbs(left: [u64; 4], right: [u64; 4]) -> Ordering {
    for index in (0..4).rev() {
        match left[index].cmp(&right[index]) {
            Ordering::Equal => {}
            ordering => return ordering,
        }
    }
    Ordering::Equal
}

#[cfg(test)]
mod tests {
    use super::{FieldElement, MODULUS};

    #[test]
    fn arithmetic_identities_hold_for_boundary_values() {
        let values = [
            [0_u8; 32],
            {
                let mut value = [0xff_u8; 32];
                value[31] = 0x7f;
                value
            },
            [
                0x42, 0x91, 0x7d, 0x23, 0xa8, 0x55, 0x0c, 0xee, 0x31, 0x19, 0x72, 0xb4, 0x8f, 0xd0,
                0x06, 0x39, 0x77, 0x54, 0x22, 0x11, 0xac, 0x09, 0xd3, 0x68, 0x5e, 0x40, 0x9a, 0x27,
                0xf1, 0x63, 0x35, 0x2a,
            ],
        ];
        for bytes in values {
            let value = FieldElement::from_bytes(bytes);
            assert_eq!(value.add(FieldElement::ZERO), value);
            assert_eq!(value.subtract(value), FieldElement::ZERO);
            assert_eq!(value.multiply(FieldElement::ONE), value);
            if !value.is_zero() {
                assert_eq!(value.multiply(value.invert()), FieldElement::ONE);
            }
        }
    }

    #[test]
    fn multiplication_distributes() {
        let a = FieldElement::from_bytes([17_u8; 32]);
        let b = FieldElement::from_bytes([93_u8; 32]);
        let c = FieldElement::from_bytes([201_u8; 32]);
        assert_eq!(a.multiply(b.add(c)), a.multiply(b).add(a.multiply(c)));
    }

    #[test]
    fn specialized_squaring_matches_generic_multiplication() {
        let mut state = 0x73d5_1c9a_f812_460b_u64;
        for _ in 0..1024 {
            let mut bytes = [0_u8; 32];
            for chunk in bytes.chunks_exact_mut(8) {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                chunk.copy_from_slice(&state.to_le_bytes());
            }
            let value = FieldElement::from_bytes(bytes);
            assert_eq!(value.square(), value.multiply(value));
        }
    }

    #[test]
    fn weak_modulus_representative_observes_canonical_semantics() {
        let weak_zero = FieldElement(MODULUS);
        assert_eq!(weak_zero, FieldElement::ZERO);
        assert!(weak_zero.is_zero());
        assert_eq!(weak_zero.to_bytes(), [0_u8; 32]);
        assert_eq!(weak_zero.add(FieldElement::ONE), FieldElement::ONE);
        assert_eq!(
            weak_zero.subtract(FieldElement::ONE).add(FieldElement::ONE),
            weak_zero
        );
        assert_eq!(weak_zero.multiply(weak_zero), FieldElement::ZERO);
        assert!(!weak_zero.is_odd());
    }
}
