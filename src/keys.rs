//! Tor-compatible expanded Ed25519 key material.

use anyhow::{Result, ensure};
use curve25519_dalek::{EdwardsPoint, Scalar};
use sha2::{Digest, Sha512};
use zeroize::{Zeroize, ZeroizeOnDrop};

/// Size of Tor's tagged-file header.
pub const HEADER_LEN: usize = 32;
/// Exact Tor public-key file header.
pub const PUBLIC_HEADER: [u8; HEADER_LEN] = *b"== ed25519v1-public: type0 ==\0\0\0";
/// Exact Tor secret-key file header.
pub const SECRET_HEADER: [u8; HEADER_LEN] = *b"== ed25519v1-secret: type0 ==\0\0\0";

/// A Tor expanded Ed25519 secret key.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct ExpandedSecret {
    bytes: [u8; 64],
}

impl ExpandedSecret {
    /// Generates an expanded key from operating-system randomness.
    pub fn generate() -> Result<Self> {
        let mut seed = [0_u8; 32];
        getrandom::fill(&mut seed)
            .map_err(|error| anyhow::anyhow!("operating-system randomness failed: {error}"))?;
        let digest = Sha512::digest(seed);
        seed.zeroize();

        let mut bytes = [0_u8; 64];
        bytes.copy_from_slice(&digest);
        bytes[0] &= 0xf8;
        bytes[31] &= 63;
        bytes[31] |= 64;
        Ok(Self { bytes })
    }

    /// Parses already-expanded key bytes.
    pub fn from_bytes(bytes: [u8; 64]) -> Result<Self> {
        let key = Self { bytes };
        key.validate_shape()?;
        Ok(key)
    }

    /// Returns the expanded bytes.
    #[must_use]
    pub fn expose_bytes(&self) -> &[u8; 64] {
        &self.bytes
    }

    /// Calculates the Ed25519 public point.
    #[must_use]
    pub fn public_point(&self) -> EdwardsPoint {
        EdwardsPoint::mul_base(&self.scalar())
    }

    /// Calculates the compressed Ed25519 public key.
    #[must_use]
    pub fn public_key(&self) -> [u8; 32] {
        self.public_point().compress().to_bytes()
    }

    /// Derives the candidate key at `8 * offset` from this key.
    pub fn with_offset(&self, offset: u64) -> Result<Self> {
        let mut candidate = self.clone();
        let mut addend = u128::from(offset) << 3;
        let mut carry = 0_u16;
        for byte in &mut candidate.bytes[..32] {
            let sum = u16::from(*byte) + u16::from((addend & 0xff) as u8) + carry;
            *byte = sum.to_le_bytes()[0];
            carry = sum >> 8;
            addend >>= 8;
        }
        ensure!(
            addend == 0 && carry == 0,
            "search offset exhausted the scalar range; start a fresh search"
        );
        candidate.validate_shape()?;
        Ok(candidate)
    }

    /// Returns the curve point at an offset without exposing a candidate key.
    #[must_use]
    pub fn point_at_offset(&self, offset: u64) -> EdwardsPoint {
        let delta = Scalar::from(offset) * Scalar::from(8_u64);
        self.public_point() + EdwardsPoint::mul_base(&delta)
    }

    /// Serializes the exact `hs_ed25519_secret_key` contents.
    #[must_use]
    pub fn tagged_bytes(&self) -> [u8; 96] {
        let mut output = [0_u8; 96];
        output[..HEADER_LEN].copy_from_slice(&SECRET_HEADER);
        output[HEADER_LEN..].copy_from_slice(&self.bytes);
        output
    }

    /// Parses exact `hs_ed25519_secret_key` contents.
    pub fn from_tagged_bytes(input: &[u8]) -> Result<Self> {
        ensure!(
            input.len() == 96,
            "secret key file must be exactly 96 bytes"
        );
        ensure!(
            input[..HEADER_LEN] == SECRET_HEADER,
            "secret key file has an invalid Tor header"
        );
        let mut bytes = [0_u8; 64];
        bytes.copy_from_slice(&input[HEADER_LEN..]);
        Self::from_bytes(bytes)
    }

    fn scalar(&self) -> Scalar {
        let mut scalar = [0_u8; 32];
        scalar.copy_from_slice(&self.bytes[..32]);
        Scalar::from_bytes_mod_order(scalar)
    }

    fn validate_shape(&self) -> Result<()> {
        ensure!(self.bytes[0] & 7 == 0, "secret scalar is not clamped");
        ensure!(
            self.bytes[31] & 0xc0 == 0x40,
            "secret scalar has invalid high clamp bits"
        );
        Ok(())
    }
}

/// Serializes exact `hs_ed25519_public_key` contents.
#[must_use]
pub fn tagged_public_key(public_key: &[u8; 32]) -> [u8; 64] {
    let mut output = [0_u8; 64];
    output[..HEADER_LEN].copy_from_slice(&PUBLIC_HEADER);
    output[HEADER_LEN..].copy_from_slice(public_key);
    output
}

/// Parses exact `hs_ed25519_public_key` contents.
pub fn parse_tagged_public_key(input: &[u8]) -> Result<[u8; 32]> {
    ensure!(
        input.len() == 64,
        "public key file must be exactly 64 bytes"
    );
    ensure!(
        input[..HEADER_LEN] == PUBLIC_HEADER,
        "public key file has an invalid Tor header"
    );
    let mut public = [0_u8; 32];
    public.copy_from_slice(&input[HEADER_LEN..]);
    Ok(public)
}

#[cfg(test)]
mod tests {
    use super::{ExpandedSecret, parse_tagged_public_key, tagged_public_key};

    #[test]
    fn tagged_files_round_trip() {
        let key = ExpandedSecret::generate().expect("operating-system randomness");
        let parsed =
            ExpandedSecret::from_tagged_bytes(&key.tagged_bytes()).expect("valid secret file");
        assert_eq!(key.public_key(), parsed.public_key());

        let public = key.public_key();
        assert_eq!(
            parse_tagged_public_key(&tagged_public_key(&public)).expect("valid public file"),
            public
        );
    }

    #[test]
    fn offset_matches_point_offset() {
        let key = ExpandedSecret::generate().expect("operating-system randomness");
        for offset in [0, 1, 17, 65_537] {
            let candidate = key.with_offset(offset).expect("valid offset");
            assert_eq!(
                candidate.public_point().compress(),
                key.point_at_offset(offset).compress()
            );
        }
    }
}
