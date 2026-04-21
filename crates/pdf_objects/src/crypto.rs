//! PDF Standard Security Handler (encryption) — decryption side only.
//!
//! This module implements just enough of the PDF 1.7 Standard Security
//! Handler to decrypt documents produced by revisions 2 and 3 (RC4 with a
//! 40-bit or 128-bit key) when the user password is empty. That covers the
//! large majority of "encrypted to prevent editing but openable by anyone"
//! PDFs that real-world documents ship with.
//!
//! AES (V=4 / V=5, R=4..6), non-empty user passwords, and public-key
//! security handlers are not implemented here and still fail up front with
//! `PdfError::Unsupported`. They can be layered on top without changing
//! this module's public surface.

use md5::{Digest, Md5};

use crate::error::{PdfError, PdfResult};
use crate::types::{ObjectRef, PdfDictionary, PdfValue};

/// Adobe's 32-byte password padding string (PDF 1.7, algorithm 2).
const PASSWORD_PADDING: [u8; 32] = [
    0x28, 0xBF, 0x4E, 0x5E, 0x4E, 0x75, 0x8A, 0x41, 0x64, 0x00, 0x4E, 0x56, 0xFF, 0xFA, 0x01, 0x08,
    0x2E, 0x2E, 0x00, 0xB6, 0xD0, 0x68, 0x3E, 0x80, 0x2F, 0x0C, 0xA9, 0xFE, 0x64, 0x53, 0x69, 0x7A,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecurityRevision {
    R2,
    R3,
}

#[derive(Debug, Clone)]
pub struct StandardSecurityHandler {
    file_key: Vec<u8>,
}

impl StandardSecurityHandler {
    /// Builds a decryption handler from the `/Encrypt` dictionary and the
    /// trailer's first `/ID` string, authenticating the supplied password.
    /// Returns `None` if the password does not authenticate.
    pub fn open(
        encrypt_dict: &PdfDictionary,
        id_first: &[u8],
        password: &[u8],
    ) -> PdfResult<Option<Self>> {
        let filter = encrypt_dict
            .get("Filter")
            .and_then(PdfValue::as_name)
            .unwrap_or("");
        if filter != "Standard" {
            return Err(PdfError::Unsupported(format!(
                "encryption filter /{filter} is not supported"
            )));
        }
        let v = encrypt_dict
            .get("V")
            .and_then(PdfValue::as_integer)
            .unwrap_or(0);
        let r = encrypt_dict
            .get("R")
            .and_then(PdfValue::as_integer)
            .unwrap_or(0);
        let revision = match r {
            2 => SecurityRevision::R2,
            3 => SecurityRevision::R3,
            other => {
                return Err(PdfError::Unsupported(format!(
                    "Standard security handler revision {other} is not supported (only R=2 and R=3 handled)"
                )));
            }
        };
        if !(1..=2).contains(&v) {
            return Err(PdfError::Unsupported(format!(
                "Standard security handler V={v} is not supported (only V=1 and V=2 handled)"
            )));
        }

        let key_length_bits = encrypt_dict
            .get("Length")
            .and_then(PdfValue::as_integer)
            .unwrap_or(40);
        if key_length_bits % 8 != 0 || !(40..=128).contains(&key_length_bits) {
            return Err(PdfError::Corrupt(format!(
                "invalid /Length {key_length_bits} in Encrypt dictionary"
            )));
        }
        let key_length_bytes = (key_length_bits / 8) as usize;

        let o = pdf_string_bytes(encrypt_dict, "O")?;
        let u = pdf_string_bytes(encrypt_dict, "U")?;
        let p = encrypt_dict
            .get("P")
            .and_then(PdfValue::as_integer)
            .ok_or_else(|| PdfError::Corrupt("Encrypt dictionary missing /P".to_string()))?;
        if o.len() != 32 || u.len() != 32 {
            return Err(PdfError::Corrupt(
                "Encrypt /O and /U must each be 32 bytes".to_string(),
            ));
        }

        let file_key = compute_file_key(password, &o, p as i32, id_first, key_length_bytes);
        if !authenticate_user_password(&file_key, revision, &u, id_first) {
            return Ok(None);
        }
        Ok(Some(Self { file_key }))
    }

    /// Decrypts `bytes` produced for the indirect object `(num, gen)` under
    /// RC4 with the per-object key described in PDF 1.7 algorithm 1.
    pub fn decrypt_bytes(&self, bytes: &[u8], object_ref: ObjectRef) -> Vec<u8> {
        let object_key = self.object_key(object_ref);
        rc4(&object_key, bytes)
    }

    fn object_key(&self, object_ref: ObjectRef) -> Vec<u8> {
        let mut material = Vec::with_capacity(self.file_key.len() + 5);
        material.extend_from_slice(&self.file_key);
        let num = object_ref.object_number.to_le_bytes();
        material.push(num[0]);
        material.push(num[1]);
        material.push(num[2]);
        let generation = object_ref.generation.to_le_bytes();
        material.push(generation[0]);
        material.push(generation[1]);
        let digest = md5_bytes(&material);
        let truncated_len = (self.file_key.len() + 5).min(16);
        digest[..truncated_len].to_vec()
    }
}

fn pdf_string_bytes(dict: &PdfDictionary, key: &str) -> PdfResult<Vec<u8>> {
    match dict.get(key) {
        Some(PdfValue::String(s)) => Ok(s.0.clone()),
        Some(_) => Err(PdfError::Corrupt(format!("Encrypt /{key} is not a string"))),
        None => Err(PdfError::Corrupt(format!(
            "Encrypt dictionary missing /{key}"
        ))),
    }
}

fn compute_file_key(
    password: &[u8],
    o_entry: &[u8],
    permissions: i32,
    id_first: &[u8],
    key_length_bytes: usize,
) -> Vec<u8> {
    // Algorithm 2 (PDF 1.7 section 7.6.3.3):
    //   1. Pad the password to 32 bytes.
    let padded = pad_password(password);
    let mut hasher = Md5::new();
    hasher.update(padded);
    //   2. Append /O.
    hasher.update(o_entry);
    //   3. Append /P (4 bytes little-endian).
    hasher.update(permissions.to_le_bytes());
    //   4. Append the first element of /ID.
    hasher.update(id_first);
    //   (Step 5 — append 0xFFFFFFFF when /EncryptMetadata is false — is an
    //   R=4+ rule; our MVP only handles R<=3 so skip it.)
    let mut digest = hasher.finalize_reset();

    // Algorithm 2, step 6: for R>=3, re-MD5 the first n bytes 50 times.
    for _ in 0..50 {
        hasher.update(&digest[..key_length_bytes]);
        digest = hasher.finalize_reset();
    }
    digest[..key_length_bytes].to_vec()
}

fn pad_password(password: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    let take = password.len().min(32);
    out[..take].copy_from_slice(&password[..take]);
    if take < 32 {
        out[take..].copy_from_slice(&PASSWORD_PADDING[..32 - take]);
    }
    out
}

fn authenticate_user_password(
    file_key: &[u8],
    revision: SecurityRevision,
    u_entry: &[u8],
    id_first: &[u8],
) -> bool {
    match revision {
        SecurityRevision::R2 => {
            // Algorithm 4: encrypt the password padding with the file key; the
            // full 32 bytes must equal /U.
            let encrypted = rc4(file_key, &PASSWORD_PADDING);
            encrypted == u_entry
        }
        SecurityRevision::R3 => {
            // Algorithm 5.
            let mut hasher = Md5::new();
            hasher.update(PASSWORD_PADDING);
            hasher.update(id_first);
            let seed = hasher.finalize();
            let mut buffer = rc4(file_key, &seed);
            for i in 1u8..=19 {
                let key: Vec<u8> = file_key.iter().map(|byte| byte ^ i).collect();
                buffer = rc4(&key, &buffer);
            }
            // The first 16 bytes of /U must match the buffer; the remaining
            // 16 bytes are arbitrary padding.
            buffer.as_slice() == &u_entry[..16]
        }
    }
}

fn md5_bytes(input: &[u8]) -> [u8; 16] {
    let mut hasher = Md5::new();
    hasher.update(input);
    hasher.finalize().into()
}

fn rc4(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut s: [u8; 256] = [0; 256];
    for (index, value) in s.iter_mut().enumerate() {
        *value = index as u8;
    }
    let mut j: u8 = 0;
    for i in 0..256 {
        j = j.wrapping_add(s[i]).wrapping_add(key[i % key.len()]);
        s.swap(i, j as usize);
    }
    let mut output = Vec::with_capacity(data.len());
    let mut i: u8 = 0;
    let mut j: u8 = 0;
    for &byte in data {
        i = i.wrapping_add(1);
        j = j.wrapping_add(s[i as usize]);
        s.swap(i as usize, j as usize);
        let k = s[(s[i as usize].wrapping_add(s[j as usize])) as usize];
        output.push(byte ^ k);
    }
    output
}

#[cfg(test)]
pub(crate) mod test_helpers {
    //! Expose the low-level primitives so parser tests can build a tiny
    //! encrypted PDF end-to-end — pick an arbitrary `/O`, derive a file key
    //! from the empty password, encrypt each object's data with per-object
    //! RC4, and then round-trip it through `parse_pdf`.

    use super::*;

    pub fn rc4(key: &[u8], data: &[u8]) -> Vec<u8> {
        super::rc4(key, data)
    }

    pub fn compute_file_key(
        password: &[u8],
        o_entry: &[u8],
        permissions: i32,
        id_first: &[u8],
        key_length_bytes: usize,
    ) -> Vec<u8> {
        super::compute_file_key(password, o_entry, permissions, id_first, key_length_bytes)
    }

    /// Produce the 32-byte `/U` value that corresponds to the empty user
    /// password under revision 3. The first 16 bytes are the RC4 output
    /// from algorithm 5; the remaining 16 bytes are arbitrary padding
    /// (here zeroed, which real writers often do).
    pub fn compute_u_r3(file_key: &[u8], id_first: &[u8]) -> Vec<u8> {
        let mut hasher = Md5::new();
        hasher.update(PASSWORD_PADDING);
        hasher.update(id_first);
        let seed = hasher.finalize();
        let mut buffer = super::rc4(file_key, &seed);
        for i in 1u8..=19 {
            let key: Vec<u8> = file_key.iter().map(|byte| byte ^ i).collect();
            buffer = super::rc4(&key, &buffer);
        }
        buffer.resize(32, 0);
        buffer
    }

    /// Build the per-object RC4 key in exactly the same way the handler
    /// does, so tests can encrypt a known plaintext and then check that
    /// the parser's decryption path inverts the transform.
    pub fn object_key(file_key: &[u8], object_number: u32, generation: u16) -> Vec<u8> {
        let mut material = Vec::with_capacity(file_key.len() + 5);
        material.extend_from_slice(file_key);
        let num = object_number.to_le_bytes();
        material.push(num[0]);
        material.push(num[1]);
        material.push(num[2]);
        let gen_bytes = generation.to_le_bytes();
        material.push(gen_bytes[0]);
        material.push(gen_bytes[1]);
        let digest = super::md5_bytes(&material);
        let truncated_len = (file_key.len() + 5).min(16);
        digest[..truncated_len].to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rc4_empty_input_returns_empty() {
        assert_eq!(rc4(b"Key", b""), Vec::<u8>::new());
    }

    #[test]
    fn rc4_matches_known_vector() {
        // RFC 6229 test vector: key "Key", data "Plaintext".
        let key = b"Key";
        let plaintext = b"Plaintext";
        let encrypted = rc4(key, plaintext);
        // Decrypting with the same keystream yields the original bytes.
        let decrypted = rc4(key, &encrypted);
        assert_eq!(decrypted, plaintext);
        // The ciphertext should match the well-known RFC 6229 output.
        assert_eq!(
            encrypted,
            [0xBB, 0xF3, 0x16, 0xE8, 0xD9, 0x40, 0xAF, 0x0A, 0xD3]
        );
    }

    #[test]
    fn pad_password_short_pads_with_padding_string() {
        let padded = pad_password(b"ab");
        assert_eq!(padded[0], b'a');
        assert_eq!(padded[1], b'b');
        assert_eq!(padded[2], PASSWORD_PADDING[0]);
        assert_eq!(padded[31], PASSWORD_PADDING[29]);
    }
}
