//! Tor v3 onion address encoding and validation.

use anyhow::{Result, bail, ensure};
use sha3::{Digest, Sha3_256};

/// Length of a Tor v3 hostname without the `.onion` suffix.
pub const ADDRESS_LEN: usize = 56;
/// RFC 4648 base32 alphabet used by Tor.
pub const BASE32_ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
const CHECKSUM_DOMAIN: &[u8] = b".onion checksum";
const VERSION: u8 = 3;

/// Encodes an Ed25519 public key as a Tor v3 onion hostname.
#[must_use]
pub fn onion_address(public_key: &[u8; 32]) -> String {
    let mut hasher = Sha3_256::new();
    hasher.update(CHECKSUM_DOMAIN);
    hasher.update(public_key);
    hasher.update([VERSION]);
    let checksum = hasher.finalize();

    let mut payload = [0_u8; 35];
    payload[..32].copy_from_slice(public_key);
    payload[32..34].copy_from_slice(&checksum[..2]);
    payload[34] = VERSION;

    let mut encoded = String::with_capacity(ADDRESS_LEN + 6);
    let mut accumulator = 0_u32;
    let mut bits = 0_u8;
    for byte in payload {
        accumulator = (accumulator << 8) | u32::from(byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            let index = ((accumulator >> bits) & 0x1f) as usize;
            encoded.push(char::from(BASE32_ALPHABET[index]));
        }
    }
    debug_assert_eq!(bits, 0);
    debug_assert_eq!(encoded.len(), ADDRESS_LEN);
    encoded.push_str(".onion");
    encoded
}

/// Decodes and validates a Tor v3 onion hostname.
pub fn validate_address(value: &str) -> Result<[u8; 32]> {
    let name = value
        .strip_suffix(".onion")
        .ok_or_else(|| anyhow::anyhow!("hostname must end in .onion"))?;
    ensure!(
        name.len() == ADDRESS_LEN,
        "Tor v3 hostname must contain {ADDRESS_LEN} base32 characters"
    );

    let mut payload = [0_u8; 35];
    let mut accumulator = 0_u32;
    let mut bits = 0_u8;
    let mut output = 0_usize;
    for byte in name.bytes() {
        let value = match byte {
            b'a'..=b'z' => byte - b'a',
            b'2'..=b'7' => byte - b'2' + 26,
            _ => bail!("hostname contains a character outside a-z and 2-7"),
        };
        accumulator = (accumulator << 5) | u32::from(value);
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            payload[output] = ((accumulator >> bits) & 0xff) as u8;
            output += 1;
        }
    }
    ensure!(
        output == payload.len() && bits == 0,
        "invalid base32 length"
    );
    ensure!(payload[34] == VERSION, "unsupported onion address version");

    let mut public_key = [0_u8; 32];
    public_key.copy_from_slice(&payload[..32]);
    ensure!(
        onion_address(&public_key) == value,
        "onion address checksum does not match"
    );
    Ok(public_key)
}

#[cfg(test)]
mod tests {
    use super::{onion_address, validate_address};

    #[test]
    fn matches_tor_spec_vector() {
        let expected = "pg6mmjiyjmcrsslvykfwnntlaru7p5svn6y2ymmju6nubxndf4pscryd.onion";
        let public = validate_address(expected).expect("spec vector should validate");
        assert_eq!(onion_address(&public), expected);
    }

    #[test]
    fn rejects_bad_checksum() {
        let bad = "pg6mmjiyjmcrsslvykfwnntlaru7p5svn6y2ymmju6nubxndf4pscrya.onion";
        assert!(validate_address(bad).is_err());
    }
}
