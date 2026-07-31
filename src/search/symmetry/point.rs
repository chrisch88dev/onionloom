//! Edwards25519 coordinate operations used around the candidate hot loop.

use anyhow::{Result, ensure};

use super::field::FieldElement;

const EDWARDS_D_BYTES: [u8; 32] = [
    0xa3, 0x78, 0x59, 0x13, 0xca, 0x4d, 0xeb, 0x75, 0xab, 0xd8, 0x41, 0x41, 0x4d, 0x0a, 0x70, 0x00,
    0x98, 0xe8, 0x79, 0x77, 0x79, 0x40, 0xc7, 0x8c, 0x73, 0xfe, 0x6f, 0x2b, 0xee, 0x6c, 0x03, 0x52,
];
const EDWARDS_D2_BYTES: [u8; 32] = [
    0x59, 0xf1, 0xb2, 0x26, 0x94, 0x9b, 0xd6, 0xeb, 0x56, 0xb1, 0x83, 0x82, 0x9a, 0x14, 0xe0, 0x00,
    0x30, 0xd1, 0xf3, 0xee, 0xf2, 0x80, 0x8e, 0x19, 0xe7, 0xfc, 0xdf, 0x56, 0xdc, 0xd9, 0x06, 0x24,
];
const SQRT_MINUS_ONE_BYTES: [u8; 32] = [
    0xb0, 0xa0, 0x0e, 0x4a, 0x27, 0x1b, 0xee, 0xc4, 0x78, 0xe4, 0x2f, 0xad, 0x06, 0x18, 0x43, 0x2f,
    0xa7, 0xd7, 0xfb, 0x3d, 0x99, 0x00, 0x4d, 0x2b, 0x0b, 0xdf, 0xc1, 0x4f, 0x80, 0x24, 0x83, 0x2b,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct AffinePoint {
    pub(super) x: FieldElement,
    pub(super) y: FieldElement,
    pub(super) xy: FieldElement,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ExtendedPoint {
    pub(super) x: FieldElement,
    pub(super) y: FieldElement,
    pub(super) z: FieldElement,
    pub(super) t: FieldElement,
}

impl AffinePoint {
    pub(super) fn decompress(mut compressed: [u8; 32]) -> Result<Self> {
        let sign = compressed[31] >> 7;
        compressed[31] &= 0x7f;
        let y = FieldElement::from_bytes(compressed);
        let y_squared = y.square();
        let numerator = y_squared.subtract(FieldElement::ONE);
        let denominator = FieldElement::from_bytes(EDWARDS_D_BYTES)
            .multiply(y_squared)
            .add(FieldElement::ONE);
        let x_squared = numerator.multiply(denominator.invert());
        let mut x = x_squared.sqrt_power();
        if x.square() != x_squared {
            x = x.multiply(FieldElement::from_bytes(SQRT_MINUS_ONE_BYTES));
        }
        ensure!(x.square() == x_squared, "invalid compressed Edwards point");
        if u8::from(x.is_odd()) != sign {
            x = x.negate();
        }
        Ok(Self {
            x,
            y,
            xy: x.multiply(y),
        })
    }

    pub(super) fn to_extended(self) -> ExtendedPoint {
        ExtendedPoint {
            x: self.x,
            y: self.y,
            z: FieldElement::ONE,
            t: self.xy,
        }
    }

    #[cfg(test)]
    pub(super) fn compress(self) -> [u8; 32] {
        let mut output = self.y.to_bytes();
        output[31] |= u8::from(self.x.is_odd()) << 7;
        output
    }
}

impl ExtendedPoint {
    pub(super) fn add(self, right: Self) -> Self {
        let left_difference = self.y.subtract(self.x);
        let right_difference = right.y.subtract(right.x);
        let difference_product = left_difference.multiply(right_difference);
        let left_sum = self.y.add(self.x);
        let right_sum = right.y.add(right.x);
        let sum_product = left_sum.multiply(right_sum);
        let scaled_t_product =
            FieldElement::from_bytes(EDWARDS_D2_BYTES).multiply(self.t.multiply(right.t));
        let z_product = self.z.multiply(right.z);
        let doubled_z_product = z_product.add(z_product);
        let x_factor = sum_product.subtract(difference_product);
        let y_factor = sum_product.add(difference_product);
        let difference_factor = doubled_z_product.subtract(scaled_t_product);
        let sum_factor = doubled_z_product.add(scaled_t_product);
        Self {
            x: x_factor.multiply(difference_factor),
            y: sum_factor.multiply(y_factor),
            z: difference_factor.multiply(sum_factor),
            t: x_factor.multiply(y_factor),
        }
    }

    pub(super) fn to_affine(self, inverse_z: FieldElement) -> AffinePoint {
        AffinePoint {
            x: self.x.multiply(inverse_z),
            y: self.y.multiply(inverse_z),
            xy: self.t.multiply(inverse_z),
        }
    }

    #[cfg(test)]
    pub(super) fn invert_z(self) -> AffinePoint {
        self.to_affine(self.z.invert())
    }
}

#[cfg(test)]
mod tests {
    use curve25519_dalek::{EdwardsPoint, Scalar};

    use super::AffinePoint;

    #[test]
    fn decompression_and_addition_match_dalek() {
        let left = EdwardsPoint::mul_base(&Scalar::from(123_456_u64));
        let right = EdwardsPoint::mul_base(&Scalar::from(7_654_321_u64));
        let left_affine =
            AffinePoint::decompress(left.compress().to_bytes()).expect("valid left point");
        let right_affine =
            AffinePoint::decompress(right.compress().to_bytes()).expect("valid right point");
        assert_eq!(left_affine.compress(), left.compress().to_bytes());
        assert_eq!(right_affine.compress(), right.compress().to_bytes());
        assert_eq!(
            left_affine
                .to_extended()
                .add(right_affine.to_extended())
                .invert_z()
                .compress(),
            (left + right).compress().to_bytes()
        );
    }
}
