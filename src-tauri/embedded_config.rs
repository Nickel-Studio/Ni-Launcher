//! Randomized build-field envelopes, not a client-side trust boundary.
//! The executable contains the decoding material; a determined local attacker can recover it.

use aes_gcm::aead::{AeadInOut, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use zeroize::{Zeroize, Zeroizing};

// Checked in build.rs and the runtime; disabling key-schedule clearing must fail compilation.
const _: () = {
    fn require_clearing<T: zeroize::ZeroizeOnDrop>() {}
    let _ = require_clearing::<Aes256Gcm>;
};

const MAGIC: &[u8; 8] = b"NICKCFG1";
const FIELD_COUNT: usize = 4;
const KEY_BYTES: usize = 32;
const NONCE_BYTES: usize = 12;
const TAG_BYTES: usize = 16;
const MAX_FIELD_BYTES: usize = 8192;
const HEADER_BYTES: usize = MAGIC.len() + 1;
const RECORD_BYTES: usize = 1 + KEY_BYTES * 2 + NONCE_BYTES + 4;
const MAX_ENVELOPE_BYTES: usize = HEADER_BYTES + FIELD_COUNT * (RECORD_BYTES + MAX_FIELD_BYTES + TAG_BYTES);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Field {
    MicrosoftClient = 1,
    MicrosoftAuthority = 2,
    MicrosoftRedirect = 3,
    CurseforgeKey = 4,
}

impl Field {
    fn from_id(id: u8) -> Option<Self> {
        match id {
            1 => Some(Self::MicrosoftClient),
            2 => Some(Self::MicrosoftAuthority),
            3 => Some(Self::MicrosoftRedirect),
            4 => Some(Self::CurseforgeKey),
            _ => None,
        }
    }

    fn associated_data(self) -> [u8; 9] {
        let mut result = [0; 9];
        result[..MAGIC.len()].copy_from_slice(MAGIC);
        result[MAGIC.len()] = self as u8;
        result
    }
}

struct Record<'a> {
    field: Field,
    left: &'a [u8],
    right: &'a [u8],
    nonce: &'a [u8],
    ciphertext: &'a [u8],
}

fn take<'a>(bytes: &mut &'a [u8], length: usize) -> Result<&'a [u8], &'static str> {
    if bytes.len() < length {
        return Err("Invalid embedded API configuration");
    }
    let (head, tail) = bytes.split_at(length);
    *bytes = tail;
    Ok(head)
}

fn records(bytes: &[u8]) -> Result<Vec<Record<'_>>, &'static str> {
    if bytes.len() > MAX_ENVELOPE_BYTES || !bytes.starts_with(MAGIC) {
        return Err("Invalid embedded API configuration");
    }
    let mut rest = &bytes[MAGIC.len()..];
    let count = take(&mut rest, 1)?[0] as usize;
    if count > FIELD_COUNT {
        return Err("Invalid embedded API configuration");
    }
    let mut seen = 0_u8;
    let mut result = Vec::with_capacity(count);
    for _ in 0..count {
        let field = Field::from_id(take(&mut rest, 1)?[0])
            .ok_or("Invalid embedded API configuration")?;
        let bit = 1_u8 << (field as u8 - 1);
        if seen & bit != 0 {
            return Err("Invalid embedded API configuration");
        }
        seen |= bit;
        let left = take(&mut rest, KEY_BYTES)?;
        let right = take(&mut rest, KEY_BYTES)?;
        let nonce = take(&mut rest, NONCE_BYTES)?;
        let length = u32::from_le_bytes(take(&mut rest, 4)?.try_into().unwrap()) as usize;
        if !(TAG_BYTES + 1..=MAX_FIELD_BYTES + TAG_BYTES).contains(&length) {
            return Err("Invalid embedded API configuration");
        }
        let ciphertext = take(&mut rest, length)?;
        result.push(Record { field, left, right, nonce, ciphertext });
    }
    if !rest.is_empty() {
        return Err("Invalid embedded API configuration");
    }
    Ok(result)
}

/// Decode only the requested field. Temporary key and plaintext buffers are cleared on drop.
#[inline(never)]
pub fn open(bytes: &[u8], field: Field) -> Result<Option<Zeroizing<String>>, &'static str> {
    let entries = records(bytes)?;
    let Some(record) = entries.iter().find(|record| record.field == field) else {
        return Ok(None);
    };
    let mut key = Zeroizing::new([0_u8; KEY_BYTES]);
    for (index, byte) in key.iter_mut().enumerate() {
        *byte = record.left[index] ^ record.right[index];
    }
    let cipher = Aes256Gcm::new_from_slice(key.as_ref())
        .map_err(|_| "Invalid embedded API configuration")?;
    let nonce = Nonce::try_from(record.nonce)
        .map_err(|_| "Invalid embedded API configuration")?;
    let mut plaintext = Zeroizing::new(record.ciphertext.to_vec());
    cipher.decrypt_in_place(&nonce, &field.associated_data(), &mut *plaintext)
        .map_err(|_| "Unable to decode embedded API configuration")?;
    let value = std::str::from_utf8(&plaintext)
        .map_err(|_| "Invalid embedded API configuration")?;
    if value.is_empty() || value.trim() != value || value.chars().any(char::is_control) {
        return Err("Invalid embedded API configuration");
    }
    Ok(Some(Zeroizing::new(value.to_owned())))
}

/// Seal whitelisted build fields or a backend runtime credential; never a complete source document.
pub fn seal(
    fields: &[(Field, &str)],
    mut random: impl FnMut(&mut [u8]) -> Result<(), &'static str>,
) -> Result<Vec<u8>, &'static str> {
    if fields.len() > FIELD_COUNT {
        return Err("Invalid embedded API configuration");
    }
    let mut seen = 0_u8;
    let mut output = Vec::new();
    output.extend_from_slice(MAGIC);
    output.push(fields.len() as u8);
    for &(field, value) in fields {
        let bit = 1_u8 << (field as u8 - 1);
        if seen & bit != 0 || value.is_empty() || value.len() > MAX_FIELD_BYTES
            || value.trim() != value || value.chars().any(char::is_control) {
            return Err("Invalid embedded API configuration");
        }
        seen |= bit;
        let mut key = Zeroizing::new([0_u8; KEY_BYTES]);
        let mut left = Zeroizing::new([0_u8; KEY_BYTES]);
        let mut right = Zeroizing::new([0_u8; KEY_BYTES]);
        let mut nonce = [0_u8; NONCE_BYTES];
        random(key.as_mut())?;
        random(left.as_mut())?;
        random(&mut nonce)?;
        for index in 0..KEY_BYTES {
            right[index] = key[index] ^ left[index];
        }
        let cipher = Aes256Gcm::new_from_slice(key.as_ref())
            .map_err(|_| "Invalid embedded API configuration")?;
        // Reserve the tag up front: reallocating a plaintext buffer would leave an uncleared copy.
        let mut ciphertext = Zeroizing::new(Vec::with_capacity(value.len() + TAG_BYTES));
        ciphertext.extend_from_slice(value.as_bytes());
        cipher.encrypt_in_place(&nonce.into(), &field.associated_data(), &mut *ciphertext)
            .map_err(|_| "Unable to seal embedded API configuration")?;
        output.push(field as u8);
        output.extend_from_slice(left.as_ref());
        output.extend_from_slice(right.as_ref());
        output.extend_from_slice(&nonce);
        output.extend_from_slice(&(ciphertext.len() as u32).to_le_bytes());
        output.extend_from_slice(&ciphertext);
        nonce.zeroize();
    }
    Ok(output)
}

#[cfg(test)]
pub fn snapshot(bytes: &[u8]) -> Result<serde_json::Value, &'static str> {
    use serde_json::{Map, Value};
    let mut output = Value::Object(Map::new());
    for record in records(bytes)? {
        let (section, name) = match record.field {
            Field::MicrosoftClient => ("microsoft", "clientId"),
            Field::MicrosoftAuthority => ("microsoft", "authority"),
            Field::MicrosoftRedirect => ("microsoft", "redirectUri"),
            Field::CurseforgeKey => ("curseforge", "apiKey"),
        };
        let value = open(bytes, record.field)?.ok_or("Invalid embedded API configuration")?;
        if output.get(section).is_none() {
            output[section] = Value::Object(Map::new());
        }
        output[section][name] = Value::String(value.to_string());
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn random(bytes: &mut [u8]) -> Result<(), &'static str> {
        getrandom::fill(bytes).map_err(|_| "Build randomness unavailable")
    }

    #[test]
    fn fields_round_trip_independently_without_literal_values() {
        let fields = [
            (Field::MicrosoftClient, "fixture-client-not-a-real-secret"),
            (Field::MicrosoftAuthority, "fixture-authority-not-a-real-secret"),
            (Field::MicrosoftRedirect, "fixture://redirect-not-a-real-secret"),
            (Field::CurseforgeKey, "fixture-cf-key-not-a-real-secret"),
        ];
        let first = seal(&fields, random).unwrap();
        let second = seal(&fields, random).unwrap();
        assert_ne!(first, second);
        for (field, value) in fields {
            assert_eq!(open(&first, field).unwrap().unwrap().as_str(), value);
            assert_eq!(open(&second, field).unwrap().unwrap().as_str(), value);
            assert!(!first.windows(value.len()).any(|window| window == value.as_bytes()));
        }
    }

    #[test]
    fn absent_fields_and_empty_envelopes_are_valid() {
        let bytes = seal(&[], random).unwrap();
        assert!(open(&bytes, Field::CurseforgeKey).unwrap().is_none());
        let bytes = seal(&[(Field::MicrosoftClient, "fixture-client")], random).unwrap();
        assert!(open(&bytes, Field::CurseforgeKey).unwrap().is_none());
    }

    #[test]
    fn corruption_is_rejected_and_other_fields_are_not_decrypted() {
        let mut bytes = seal(&[
            (Field::MicrosoftClient, "fixture-client"),
            (Field::CurseforgeKey, "fixture-key"),
        ], random).unwrap();
        *bytes.last_mut().unwrap() ^= 1;
        assert!(open(&bytes, Field::MicrosoftClient).unwrap().is_some());
        assert!(open(&bytes, Field::CurseforgeKey).is_err());
    }

    #[test]
    fn malformed_envelopes_never_panic_or_return_partial_values() {
        let bytes = seal(&[(Field::CurseforgeKey, "fixture-key")], random).unwrap();
        for length in 0..bytes.len() {
            assert!(open(&bytes[..length], Field::CurseforgeKey).is_err());
        }
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert!(open(&trailing, Field::CurseforgeKey).is_err());
        let mut unknown = bytes.clone();
        unknown[HEADER_BYTES] = 5;
        assert!(open(&unknown, Field::CurseforgeKey).is_err());
        let mut swapped = bytes.clone();
        swapped[HEADER_BYTES] = Field::MicrosoftClient as u8;
        assert!(open(&swapped, Field::MicrosoftClient).is_err());
        let mut oversized = bytes;
        let start = HEADER_BYTES + RECORD_BYTES - 4;
        oversized[start..start + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(open(&oversized, Field::CurseforgeKey).is_err());
    }

    #[test]
    fn bad_fields_duplicates_and_rng_failures_are_rejected_without_values() {
        for value in ["", " fixture", "bad\nvalue", "bad\0value"] {
            assert!(seal(&[(Field::CurseforgeKey, value)], random).is_err());
        }
        assert!(seal(&[(Field::CurseforgeKey, "x"), (Field::CurseforgeKey, "y")], random).is_err());
        let error = seal(&[(Field::CurseforgeKey, "private-error-fixture")], |_| Err("Build randomness unavailable"))
            .unwrap_err();
        assert!(!error.contains("private-error-fixture"));
        let maximum = "x".repeat(MAX_FIELD_BYTES);
        let bytes = seal(&[(Field::CurseforgeKey, &maximum)], random).unwrap();
        assert_eq!(open(&bytes, Field::CurseforgeKey).unwrap().unwrap().len(), MAX_FIELD_BYTES);
        assert!(seal(&[(Field::CurseforgeKey, &"x".repeat(MAX_FIELD_BYTES + 1))], random).is_err());
    }
}
