//! PDF Standard Security Handler (encryption) — decryption side only.
//!
//! This module implements enough of the PDF 1.7 Standard Security Handler
//! to decrypt documents produced by:
//!
//! - revisions 2 and 3 (V=1 or V=2, RC4 with a 40-bit or 128-bit key),
//! - revision 4 with V=4 crypt filters naming `/V2` (RC4-128) or
//!   `/AESV2` (AES-128-CBC) as the stream and string method,
//!
//! under either the user password or the owner password. The empty user
//! password is accepted as a special case of the general user-password
//! path.
//!
//! V=5 / R=6 (AES-256) and public-key security handlers are not yet
//! implemented and still fail up front with `PdfError::Unsupported`.
//! They can be layered on top without changing this module's public
//! surface.

use aes::Aes128;
use aes::cipher::{BlockDecrypt, KeyInit, generic_array::GenericArray};
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
    R4,
}

/// Which crypt filter method applies to a given piece of ciphertext.
///
/// V=1/2 documents always use [`CryptMethod::V2`] (RC4) for everything.
/// V=4 documents name a crypt filter per kind (`/StmF`, `/StrF`, `/EFF`);
/// each may point at `/Identity` (no encryption), a V2 filter (RC4), or
/// an AESV2 filter (AES-128-CBC).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CryptMethod {
    Identity,
    V2,
    AesV2,
}

/// Which slot the ciphertext belongs to. Drives the crypt-method choice
/// (string vs stream) on V=4 documents and is a no-op on V=1/2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BytesKind {
    String,
    Stream,
}

#[derive(Debug, Clone)]
pub struct StandardSecurityHandler {
    file_key: Vec<u8>,
    string_method: CryptMethod,
    stream_method: CryptMethod,
    /// `false` only for V=4 documents that explicitly set
    /// `/EncryptMetadata false`; `true` everywhere else. When `false`,
    /// streams with `/Type /Metadata` and `/Subtype /XML` skip
    /// decryption.
    encrypt_metadata: bool,
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
            4 => SecurityRevision::R4,
            other => {
                return Err(PdfError::Unsupported(format!(
                    "Standard security handler revision {other} is not supported (only R=2, R=3, and R=4 handled)"
                )));
            }
        };

        let (string_method, stream_method, key_length_bytes) = match v {
            1 | 2 => {
                let bits = encrypt_dict
                    .get("Length")
                    .and_then(PdfValue::as_integer)
                    .unwrap_or(40);
                if bits % 8 != 0 || !(40..=128).contains(&bits) {
                    return Err(PdfError::Corrupt(format!(
                        "invalid /Length {bits} in Encrypt dictionary"
                    )));
                }
                (CryptMethod::V2, CryptMethod::V2, (bits / 8) as usize)
            }
            4 => {
                // V=4: crypt filters decide the method per slot. The file
                // key is always 128-bit (16 bytes).
                let (strf, stmf) = resolve_v4_crypt_filters(encrypt_dict)?;
                (strf, stmf, 16)
            }
            other => {
                return Err(PdfError::Unsupported(format!(
                    "Standard security handler V={other} is not supported (only V=1, V=2, and V=4 handled)"
                )));
            }
        };

        // V=4's Algorithm 2 step 5: when /EncryptMetadata is explicitly
        // false, 0xFFFFFFFF is appended before the 50-round rehash.
        let encrypt_metadata = if matches!(revision, SecurityRevision::R4) {
            encrypt_dict
                .get("EncryptMetadata")
                .and_then(PdfValue::as_bool)
                .unwrap_or(true)
        } else {
            true
        };

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

        // First try the supplied password as the user password.
        let user_file_key = compute_file_key(
            password,
            &o,
            p as i32,
            id_first,
            key_length_bytes,
            revision,
            encrypt_metadata,
        );
        if authenticate_user_password(&user_file_key, revision, &u, id_first) {
            return Ok(Some(Self {
                file_key: user_file_key,
                string_method,
                stream_method,
                encrypt_metadata,
            }));
        }

        // Then try it as the owner password: Algorithm 7 recovers the
        // padded user password from /O, after which we redo the user-
        // password authentication with that recovered value. The file key
        // used for object decryption is always derived from the user
        // password — the owner password is only a way of recovering it.
        let recovered_user_password =
            recover_user_password_from_owner(password, &o, revision, key_length_bytes);
        let owner_file_key = compute_file_key(
            &recovered_user_password,
            &o,
            p as i32,
            id_first,
            key_length_bytes,
            revision,
            encrypt_metadata,
        );
        if authenticate_user_password(&owner_file_key, revision, &u, id_first) {
            return Ok(Some(Self {
                file_key: owner_file_key,
                string_method,
                stream_method,
                encrypt_metadata,
            }));
        }

        Ok(None)
    }

    /// Returns true when this handler was configured with
    /// `/EncryptMetadata false`. Parser uses this to skip
    /// `/Type /Metadata` streams.
    pub fn encrypts_metadata(&self) -> bool {
        self.encrypt_metadata
    }

    /// Decrypts `bytes` produced for the indirect object `(num, gen)`.
    /// The crypt method is chosen per `kind` — strings use `/StrF`,
    /// streams use `/StmF`. Returns the ciphertext unchanged for
    /// `/Identity` filters; returns an error for malformed AES input
    /// (wrong length, bad PKCS#7 padding).
    pub fn decrypt_bytes(
        &self,
        bytes: &[u8],
        object_ref: ObjectRef,
        kind: BytesKind,
    ) -> PdfResult<Vec<u8>> {
        let method = match kind {
            BytesKind::String => self.string_method,
            BytesKind::Stream => self.stream_method,
        };
        match method {
            CryptMethod::Identity => Ok(bytes.to_vec()),
            CryptMethod::V2 => Ok(rc4(&self.object_key(object_ref, method), bytes)),
            CryptMethod::AesV2 => aes_128_cbc_decrypt(&self.object_key(object_ref, method), bytes),
        }
    }

    fn object_key(&self, object_ref: ObjectRef, method: CryptMethod) -> Vec<u8> {
        // Algorithm 1 / 1a. Append the 4-byte ASCII suffix "sAlT" when
        // the method is AES so keys derived for the same object under
        // different methods never collide.
        let suffix_len = if matches!(method, CryptMethod::AesV2) {
            9
        } else {
            5
        };
        let mut material = Vec::with_capacity(self.file_key.len() + suffix_len);
        material.extend_from_slice(&self.file_key);
        let num = object_ref.object_number.to_le_bytes();
        material.push(num[0]);
        material.push(num[1]);
        material.push(num[2]);
        let generation = object_ref.generation.to_le_bytes();
        material.push(generation[0]);
        material.push(generation[1]);
        if matches!(method, CryptMethod::AesV2) {
            material.extend_from_slice(b"sAlT");
        }
        let digest = md5_bytes(&material);
        let truncated_len = (self.file_key.len() + 5).min(16);
        digest[..truncated_len].to_vec()
    }
}

fn resolve_v4_crypt_filters(encrypt_dict: &PdfDictionary) -> PdfResult<(CryptMethod, CryptMethod)> {
    let strf = encrypt_dict
        .get("StrF")
        .and_then(PdfValue::as_name)
        .unwrap_or("Identity");
    let stmf = encrypt_dict
        .get("StmF")
        .and_then(PdfValue::as_name)
        .unwrap_or("Identity");
    let cf = encrypt_dict.get("CF").and_then(|value| match value {
        PdfValue::Dictionary(dict) => Some(dict),
        _ => None,
    });
    Ok((
        resolve_crypt_filter_method(cf, strf)?,
        resolve_crypt_filter_method(cf, stmf)?,
    ))
}

fn resolve_crypt_filter_method(cf: Option<&PdfDictionary>, name: &str) -> PdfResult<CryptMethod> {
    // The spec reserves the `Identity` filter name for "no encryption"
    // and specifies that it never appears in /CF; treat it as a pass-
    // through without consulting the dictionary.
    if name == "Identity" {
        return Ok(CryptMethod::Identity);
    }
    let subfilter = cf
        .and_then(|dict| dict.get(name))
        .and_then(|value| match value {
            PdfValue::Dictionary(dict) => Some(dict),
            _ => None,
        })
        .ok_or_else(|| {
            PdfError::Corrupt(format!(
                "Encrypt /CF is missing the crypt filter entry /{name}"
            ))
        })?;
    let cfm = subfilter
        .get("CFM")
        .and_then(PdfValue::as_name)
        .ok_or_else(|| {
            PdfError::Corrupt(format!("crypt filter /{name} is missing the /CFM entry"))
        })?;
    match cfm {
        "V2" => Ok(CryptMethod::V2),
        "AESV2" => Ok(CryptMethod::AesV2),
        "None" => Ok(CryptMethod::Identity),
        other => Err(PdfError::Unsupported(format!(
            "crypt filter method /{other} is not supported (only /V2 and /AESV2 handled)"
        ))),
    }
}

/// Decrypts AES-128-CBC ciphertext whose first 16 bytes are the IV and
/// whose payload is PKCS#7-padded. Used for V=4 /AESV2 streams and
/// strings.
fn aes_128_cbc_decrypt(key: &[u8], data: &[u8]) -> PdfResult<Vec<u8>> {
    if key.len() != 16 {
        return Err(PdfError::Corrupt(format!(
            "AES-128 object key must be 16 bytes, got {}",
            key.len()
        )));
    }
    if data.len() < 32 || data.len() % 16 != 0 {
        return Err(PdfError::Corrupt(format!(
            "AES-128-CBC ciphertext must be at least 32 bytes and a multiple of 16; got {}",
            data.len()
        )));
    }
    let cipher = Aes128::new_from_slice(key)
        .map_err(|error| PdfError::Corrupt(format!("AES-128 key rejected by cipher: {error}")))?;
    let mut prev_block: [u8; 16] = data[..16].try_into().expect("slice is 16 bytes");
    let mut output = Vec::with_capacity(data.len() - 16);
    for chunk in data[16..].chunks(16) {
        let mut block = GenericArray::clone_from_slice(chunk);
        cipher.decrypt_block(&mut block);
        for (plain_byte, iv_byte) in block.iter_mut().zip(prev_block.iter()) {
            *plain_byte ^= iv_byte;
        }
        output.extend_from_slice(block.as_slice());
        prev_block.copy_from_slice(chunk);
    }
    strip_pkcs7(output)
}

fn strip_pkcs7(mut data: Vec<u8>) -> PdfResult<Vec<u8>> {
    let Some(&pad) = data.last() else {
        return Err(PdfError::Corrupt(
            "AES-128-CBC plaintext is empty — missing PKCS#7 padding".to_string(),
        ));
    };
    if pad == 0 || pad > 16 || (pad as usize) > data.len() {
        return Err(PdfError::Corrupt(format!(
            "AES-128-CBC PKCS#7 padding byte {pad} is out of range"
        )));
    }
    let new_len = data.len() - pad as usize;
    if !data[new_len..].iter().all(|byte| *byte == pad) {
        return Err(PdfError::Corrupt(
            "AES-128-CBC PKCS#7 padding bytes do not match".to_string(),
        ));
    }
    data.truncate(new_len);
    Ok(data)
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
    revision: SecurityRevision,
    encrypt_metadata: bool,
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
    //   5. (R>=4 only) When /EncryptMetadata is explicitly false, append
    //      0xFFFFFFFF. R<=3 skips this step.
    if matches!(revision, SecurityRevision::R4) && !encrypt_metadata {
        hasher.update([0xFFu8; 4]);
    }
    let mut digest = hasher.finalize_reset();

    // Algorithm 2, step 6: for R>=3, re-MD5 the first n bytes 50 times.
    if matches!(revision, SecurityRevision::R3 | SecurityRevision::R4) {
        for _ in 0..50 {
            hasher.update(&digest[..key_length_bytes]);
            digest = hasher.finalize_reset();
        }
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

fn recover_user_password_from_owner(
    owner_password: &[u8],
    o_entry: &[u8],
    revision: SecurityRevision,
    key_length_bytes: usize,
) -> Vec<u8> {
    // Algorithm 7 (PDF 1.7 §7.6.3.4). Symmetric inverse of Algorithm 3:
    //   1. Pad the owner password and MD5 it.
    //   2. For R>=3 re-hash 50 times.
    //   3. Truncate to `key_length_bytes` — this is the RC4 key used on /O.
    //   4. For R=2, RC4-decrypt /O once with that key.
    //      For R>=3, RC4-decrypt /O 20 times with keys (base XOR i) for i
    //      decreasing from 19 down to 0.
    //   5. The result is the padded user password.
    let padded = pad_password(owner_password);
    let mut hasher = Md5::new();
    hasher.update(padded);
    let mut digest = hasher.finalize_reset();
    if matches!(revision, SecurityRevision::R3 | SecurityRevision::R4) {
        for _ in 0..50 {
            hasher.update(&digest[..key_length_bytes]);
            digest = hasher.finalize_reset();
        }
    }
    let base_key = digest[..key_length_bytes].to_vec();

    match revision {
        SecurityRevision::R2 => rc4(&base_key, o_entry),
        SecurityRevision::R3 | SecurityRevision::R4 => {
            let mut buffer = o_entry.to_vec();
            for i in (0u8..=19).rev() {
                let key: Vec<u8> = base_key.iter().map(|byte| byte ^ i).collect();
                buffer = rc4(&key, &buffer);
            }
            buffer
        }
    }
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
        SecurityRevision::R3 | SecurityRevision::R4 => {
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
        // Callers that do not care about the revision use the R=3 variant,
        // which matches the write side of the existing RC4 fixtures.
        super::compute_file_key(
            password,
            o_entry,
            permissions,
            id_first,
            key_length_bytes,
            SecurityRevision::R3,
            true,
        )
    }

    pub fn compute_file_key_with_revision(
        password: &[u8],
        o_entry: &[u8],
        permissions: i32,
        id_first: &[u8],
        key_length_bytes: usize,
        revision: SecurityRevision,
    ) -> Vec<u8> {
        super::compute_file_key(
            password,
            o_entry,
            permissions,
            id_first,
            key_length_bytes,
            revision,
            true,
        )
    }

    /// R=4 variant of the file-key derivation, exposed so AES-128 test
    /// fixtures can build a matching file key and `/U` entry. Mirrors
    /// [`compute_file_key`] but honours `encrypt_metadata` so the
    /// Algorithm 2 step-5 branch (append 0xFFFFFFFF) can be exercised.
    pub fn compute_file_key_r4(
        password: &[u8],
        o_entry: &[u8],
        permissions: i32,
        id_first: &[u8],
        encrypt_metadata: bool,
    ) -> Vec<u8> {
        super::compute_file_key(
            password,
            o_entry,
            permissions,
            id_first,
            16,
            SecurityRevision::R4,
            encrypt_metadata,
        )
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

    /// Build the `/O` value for the Encrypt dictionary, given the owner
    /// and user passwords and the security revision. Algorithm 3 — the
    /// write-side inverse of Algorithm 7, used by tests to construct
    /// synthetic encrypted PDFs with both owner and user passwords
    /// populated.
    pub fn compute_o(
        owner_password: &[u8],
        user_password: &[u8],
        revision: SecurityRevision,
        key_length_bytes: usize,
    ) -> Vec<u8> {
        let padded_owner = pad_password(owner_password);
        let mut hasher = Md5::new();
        hasher.update(padded_owner);
        let mut digest = hasher.finalize_reset();
        if matches!(revision, SecurityRevision::R3 | SecurityRevision::R4) {
            for _ in 0..50 {
                hasher.update(&digest[..key_length_bytes]);
                digest = hasher.finalize_reset();
            }
        }
        let base_key = digest[..key_length_bytes].to_vec();

        let padded_user = pad_password(user_password);
        match revision {
            SecurityRevision::R2 => super::rc4(&base_key, &padded_user),
            SecurityRevision::R3 | SecurityRevision::R4 => {
                let mut buffer = super::rc4(&base_key, &padded_user);
                for i in 1u8..=19 {
                    let key: Vec<u8> = base_key.iter().map(|byte| byte ^ i).collect();
                    buffer = super::rc4(&key, &buffer);
                }
                buffer
            }
        }
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

    /// AES variant of [`object_key`]: appends the literal `sAlT` suffix
    /// before the MD5 so the V=4 /AESV2 path derives a distinct key
    /// from the RC4 path for the same indirect object.
    pub fn object_key_aes(file_key: &[u8], object_number: u32, generation: u16) -> Vec<u8> {
        let mut material = Vec::with_capacity(file_key.len() + 9);
        material.extend_from_slice(file_key);
        let num = object_number.to_le_bytes();
        material.push(num[0]);
        material.push(num[1]);
        material.push(num[2]);
        let gen_bytes = generation.to_le_bytes();
        material.push(gen_bytes[0]);
        material.push(gen_bytes[1]);
        material.extend_from_slice(b"sAlT");
        let digest = super::md5_bytes(&material);
        let truncated_len = (file_key.len() + 5).min(16);
        digest[..truncated_len].to_vec()
    }

    /// Encrypt `plaintext` with AES-128-CBC, PKCS#7-padded, and prefix
    /// the 16-byte IV — matching exactly what the parser's decryption
    /// path expects. Used by tests to build synthetic V=4 fixtures.
    pub fn aes_128_cbc_encrypt(key: &[u8], iv: &[u8; 16], plaintext: &[u8]) -> Vec<u8> {
        use aes::cipher::BlockEncrypt;

        assert_eq!(key.len(), 16, "AES-128 key must be 16 bytes");
        let cipher = Aes128::new_from_slice(key).expect("key length validated");

        // Pad with PKCS#7, always appending at least one byte of padding.
        let pad_len = 16 - (plaintext.len() % 16);
        let mut padded = Vec::with_capacity(plaintext.len() + pad_len);
        padded.extend_from_slice(plaintext);
        padded.extend(std::iter::repeat_n(pad_len as u8, pad_len));

        let mut output = Vec::with_capacity(16 + padded.len());
        output.extend_from_slice(iv);
        let mut prev: [u8; 16] = *iv;
        for chunk in padded.chunks(16) {
            let mut block = [0u8; 16];
            for ((b, plain), iv_byte) in block.iter_mut().zip(chunk.iter()).zip(prev.iter()) {
                *b = plain ^ iv_byte;
            }
            let mut arr = GenericArray::clone_from_slice(&block);
            cipher.encrypt_block(&mut arr);
            output.extend_from_slice(arr.as_slice());
            prev.copy_from_slice(arr.as_slice());
        }
        output
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

    #[test]
    fn pad_password_truncates_to_32_bytes() {
        let long = vec![b'x'; 64];
        let padded = pad_password(&long);
        assert_eq!(padded, [b'x'; 32]);
    }

    fn build_encrypt_dict_r3(
        o_entry: Vec<u8>,
        u_entry: Vec<u8>,
        permissions: i32,
    ) -> PdfDictionary {
        let mut dict = PdfDictionary::default();
        dict.insert("Filter".to_string(), PdfValue::Name("Standard".to_string()));
        dict.insert("V".to_string(), PdfValue::Integer(2));
        dict.insert("R".to_string(), PdfValue::Integer(3));
        dict.insert("Length".to_string(), PdfValue::Integer(128));
        dict.insert(
            "O".to_string(),
            PdfValue::String(crate::types::PdfString(o_entry)),
        );
        dict.insert(
            "U".to_string(),
            PdfValue::String(crate::types::PdfString(u_entry)),
        );
        dict.insert("P".to_string(), PdfValue::Integer(permissions as i64));
        dict
    }

    fn build_r3_handler_inputs(
        user_password: &[u8],
        owner_password: &[u8],
        id_first: &[u8],
    ) -> (PdfDictionary, Vec<u8>) {
        let key_length_bytes = 16;
        let permissions: i32 = -4;
        let o = test_helpers::compute_o(
            owner_password,
            user_password,
            SecurityRevision::R3,
            key_length_bytes,
        );
        let file_key = test_helpers::compute_file_key(
            user_password,
            &o,
            permissions,
            id_first,
            key_length_bytes,
        );
        let u = test_helpers::compute_u_r3(&file_key, id_first);
        (build_encrypt_dict_r3(o, u, permissions), file_key)
    }

    #[test]
    fn open_authenticates_user_password() {
        let id_first = b"synthetic-id-0123456789abcdef";
        let (dict, expected_file_key) = build_r3_handler_inputs(b"userpw", b"ownerpw", id_first);
        let handler = StandardSecurityHandler::open(&dict, id_first, b"userpw")
            .expect("open succeeds")
            .expect("user password authenticates");
        assert_eq!(handler.file_key, expected_file_key);
    }

    #[test]
    fn open_authenticates_owner_password() {
        let id_first = b"synthetic-id-0123456789abcdef";
        let (dict, expected_file_key) = build_r3_handler_inputs(b"userpw", b"ownerpw", id_first);
        let handler = StandardSecurityHandler::open(&dict, id_first, b"ownerpw")
            .expect("open succeeds")
            .expect("owner password authenticates");
        // File key must match the one derived from the user password — the
        // owner password is only a way of recovering it.
        assert_eq!(handler.file_key, expected_file_key);
    }

    #[test]
    fn open_rejects_wrong_password() {
        let id_first = b"synthetic-id-0123456789abcdef";
        let (dict, _) = build_r3_handler_inputs(b"userpw", b"ownerpw", id_first);
        let result = StandardSecurityHandler::open(&dict, id_first, b"wrongpw")
            .expect("open does not fail, only reports authentication");
        assert!(result.is_none());
    }

    #[test]
    fn open_accepts_utf8_password() {
        let id_first = b"synthetic-id-0123456789abcdef";
        let password = "pässwörd".as_bytes();
        let (dict, _) = build_r3_handler_inputs(password, b"ownerpw", id_first);
        let handler = StandardSecurityHandler::open(&dict, id_first, password)
            .expect("open succeeds")
            .expect("UTF-8 password authenticates");
        assert_eq!(handler.file_key.len(), 16);
    }

    fn build_encrypt_dict_v4_aesv2(
        o_entry: Vec<u8>,
        u_entry: Vec<u8>,
        permissions: i32,
        encrypt_metadata: Option<bool>,
    ) -> PdfDictionary {
        let mut std_cf = PdfDictionary::default();
        std_cf.insert("CFM".to_string(), PdfValue::Name("AESV2".to_string()));
        std_cf.insert("Length".to_string(), PdfValue::Integer(16));
        std_cf.insert(
            "AuthEvent".to_string(),
            PdfValue::Name("DocOpen".to_string()),
        );

        let mut cf = PdfDictionary::default();
        cf.insert("StdCF".to_string(), PdfValue::Dictionary(std_cf));

        let mut dict = PdfDictionary::default();
        dict.insert("Filter".to_string(), PdfValue::Name("Standard".to_string()));
        dict.insert("V".to_string(), PdfValue::Integer(4));
        dict.insert("R".to_string(), PdfValue::Integer(4));
        dict.insert("Length".to_string(), PdfValue::Integer(128));
        dict.insert("CF".to_string(), PdfValue::Dictionary(cf));
        dict.insert("StmF".to_string(), PdfValue::Name("StdCF".to_string()));
        dict.insert("StrF".to_string(), PdfValue::Name("StdCF".to_string()));
        dict.insert(
            "O".to_string(),
            PdfValue::String(crate::types::PdfString(o_entry)),
        );
        dict.insert(
            "U".to_string(),
            PdfValue::String(crate::types::PdfString(u_entry)),
        );
        dict.insert("P".to_string(), PdfValue::Integer(permissions as i64));
        if let Some(value) = encrypt_metadata {
            dict.insert("EncryptMetadata".to_string(), PdfValue::Bool(value));
        }
        dict
    }

    fn build_v4_handler_inputs(
        user_password: &[u8],
        owner_password: &[u8],
        id_first: &[u8],
        encrypt_metadata: Option<bool>,
    ) -> (PdfDictionary, Vec<u8>) {
        let permissions: i32 = -4;
        let o = test_helpers::compute_o(owner_password, user_password, SecurityRevision::R4, 16);
        let file_key = test_helpers::compute_file_key_r4(
            user_password,
            &o,
            permissions,
            id_first,
            encrypt_metadata.unwrap_or(true),
        );
        let u = test_helpers::compute_u_r3(&file_key, id_first);
        (
            build_encrypt_dict_v4_aesv2(o, u, permissions, encrypt_metadata),
            file_key,
        )
    }

    #[test]
    fn open_v4_aesv2_handler_authenticates_user_password() {
        let id_first = b"v4-synthetic-id-0123456789";
        let (dict, expected_file_key) =
            build_v4_handler_inputs(b"userpw", b"ownerpw", id_first, None);
        let handler = StandardSecurityHandler::open(&dict, id_first, b"userpw")
            .expect("open succeeds")
            .expect("user password authenticates on V=4");
        assert_eq!(handler.file_key, expected_file_key);
        assert_eq!(handler.string_method, CryptMethod::AesV2);
        assert_eq!(handler.stream_method, CryptMethod::AesV2);
        assert!(handler.encrypt_metadata);
    }

    #[test]
    fn open_v4_aesv2_handler_authenticates_owner_password() {
        let id_first = b"v4-synthetic-id-0123456789";
        let (dict, expected_file_key) =
            build_v4_handler_inputs(b"userpw", b"ownerpw", id_first, None);
        let handler = StandardSecurityHandler::open(&dict, id_first, b"ownerpw")
            .expect("open succeeds")
            .expect("owner password authenticates on V=4");
        assert_eq!(handler.file_key, expected_file_key);
    }

    #[test]
    fn open_v4_honours_encrypt_metadata_false() {
        let id_first = b"v4-metadata-id";
        let (dict, _) = build_v4_handler_inputs(b"", b"ownerpw", id_first, Some(false));
        let handler = StandardSecurityHandler::open(&dict, id_first, b"")
            .expect("open succeeds")
            .expect("empty password authenticates");
        assert!(!handler.encrypts_metadata());
    }

    #[test]
    fn open_v4_identity_crypt_filter_is_passthrough() {
        let id_first = b"v4-identity-id";
        let (dict_v4, _) = build_v4_handler_inputs(b"", b"ownerpw", id_first, None);
        let mut dict = dict_v4;
        dict.insert("StrF".to_string(), PdfValue::Name("Identity".to_string()));
        dict.insert("StmF".to_string(), PdfValue::Name("Identity".to_string()));

        let handler = StandardSecurityHandler::open(&dict, id_first, b"")
            .expect("open succeeds")
            .expect("empty password authenticates");
        assert_eq!(handler.string_method, CryptMethod::Identity);
        assert_eq!(handler.stream_method, CryptMethod::Identity);

        let ciphertext = b"hello";
        let plaintext = handler
            .decrypt_bytes(ciphertext, ObjectRef::new(4, 0), BytesKind::Stream)
            .expect("identity passes bytes through");
        assert_eq!(plaintext, ciphertext);
    }

    #[test]
    fn open_v4_rejects_unsupported_cfm() {
        let id_first = b"v4-unsupported-id";

        let (dict_v4, _) = build_v4_handler_inputs(b"", b"ownerpw", id_first, None);
        let mut dict = dict_v4;
        let mut std_cf = PdfDictionary::default();
        std_cf.insert("CFM".to_string(), PdfValue::Name("AESV3".to_string()));
        std_cf.insert("Length".to_string(), PdfValue::Integer(32));
        let mut cf = PdfDictionary::default();
        cf.insert("StdCF".to_string(), PdfValue::Dictionary(std_cf));
        dict.insert("CF".to_string(), PdfValue::Dictionary(cf));

        let error = StandardSecurityHandler::open(&dict, id_first, b"")
            .expect_err("AESV3 must be rejected as unsupported");
        assert!(matches!(error, PdfError::Unsupported(_)), "got {error:?}");
    }

    #[test]
    fn aes_128_cbc_round_trip() {
        let key = [0x11u8; 16];
        let iv = [0x22u8; 16];
        let plaintext = b"redact me, please";
        let ciphertext = test_helpers::aes_128_cbc_encrypt(&key, &iv, plaintext);
        let decrypted = aes_128_cbc_decrypt(&key, &ciphertext).expect("round trip succeeds");
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn aes_128_cbc_rejects_bad_pkcs7_padding() {
        let key = [0x11u8; 16];
        let iv = [0x22u8; 16];
        let plaintext = b"abcdef";
        let mut ciphertext = test_helpers::aes_128_cbc_encrypt(&key, &iv, plaintext);
        // Flip the last ciphertext byte so the plaintext padding becomes
        // invalid (with high probability) after decryption.
        let last = ciphertext.len() - 1;
        ciphertext[last] ^= 0xFF;
        let error =
            aes_128_cbc_decrypt(&key, &ciphertext).expect_err("corrupted padding must be rejected");
        assert!(matches!(error, PdfError::Corrupt(_)), "got {error:?}");
    }

    #[test]
    fn aes_128_cbc_rejects_short_ciphertext() {
        let key = [0x11u8; 16];
        let error = aes_128_cbc_decrypt(&key, &[0u8; 16])
            .expect_err("ciphertext shorter than IV+1 block must be rejected");
        assert!(matches!(error, PdfError::Corrupt(_)), "got {error:?}");
    }

    #[test]
    fn open_r2_authenticates_owner_password() {
        // Algorithm 4 / 7 divergence from R=3: single RC4 round for /O,
        // full 32-byte /U match.
        let id_first = b"r2-synthetic-id";
        let user_password = b"u2";
        let owner_password = b"o2";
        let key_length_bytes = 5; // 40-bit key, matching R=2 default.
        let permissions: i32 = -4;
        let o = test_helpers::compute_o(
            owner_password,
            user_password,
            SecurityRevision::R2,
            key_length_bytes,
        );
        let file_key = test_helpers::compute_file_key_with_revision(
            user_password,
            &o,
            permissions,
            id_first,
            key_length_bytes,
            SecurityRevision::R2,
        );
        // Algorithm 4: /U is RC4(file_key, PASSWORD_PADDING).
        let u = test_helpers::rc4(&file_key, &PASSWORD_PADDING);

        let mut dict = PdfDictionary::default();
        dict.insert("Filter".to_string(), PdfValue::Name("Standard".to_string()));
        dict.insert("V".to_string(), PdfValue::Integer(1));
        dict.insert("R".to_string(), PdfValue::Integer(2));
        dict.insert("Length".to_string(), PdfValue::Integer(40));
        dict.insert(
            "O".to_string(),
            PdfValue::String(crate::types::PdfString(o)),
        );
        dict.insert(
            "U".to_string(),
            PdfValue::String(crate::types::PdfString(u)),
        );
        dict.insert("P".to_string(), PdfValue::Integer(permissions as i64));

        let handler = StandardSecurityHandler::open(&dict, id_first, owner_password)
            .expect("open succeeds")
            .expect("owner password authenticates on R=2");
        assert_eq!(handler.file_key, file_key);
    }
}
