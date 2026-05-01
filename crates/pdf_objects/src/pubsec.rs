//! Public-key security handler for PDFs that use `/Filter /Adobe.PubSec`.
//!
//! Unlike the password-based [`StandardSecurityHandler`], the file
//! encryption key is not derived from a password hash. The producer
//! wraps a 20-byte seed (plus 4-byte permission word) inside a CMS
//! `EnvelopedData` blob using each authorized recipient's RSA public
//! key. Decryption requires the recipient's X.509 certificate (to find
//! the right blob) plus its RSA private key (to unwrap the seed).
//!
//! The file encryption key is then derived from the unwrapped seed
//! together with all recipient blobs concatenated:
//! - V=4 (`/SubFilter /adbe.pkcs7.s4`):
//!   `SHA-1(seed[0..20] ‖ recipients_blobs ‖ permission_bytes)` truncated
//!   to the 16-byte AES-128 key.
//! - V=5 (`/SubFilter /adbe.pkcs7.s5`):
//!   `SHA-256(seed[0..20] ‖ recipients_blobs ‖ permission_bytes)`
//!   truncated to the 32-byte AES-256 key.
//!
//! Once the file key is derived the rest of the decryption pipeline
//! (per-object key derivation for V=4, direct file-key use for V=5,
//! AES-CBC-PKCS#7 unwrap) is identical to the Standard handler, so
//! [`open_pubsec`] returns a [`StandardSecurityHandler`] built via
//! [`StandardSecurityHandler::from_file_key`].

use cms::content_info::ContentInfo;
use cms::enveloped_data::{EnvelopedData, RecipientIdentifier, RecipientInfo};
use const_oid::ObjectIdentifier;
use der::{Decode, Encode};
use rsa::{Oaep, Pkcs1v15Encrypt, RsaPrivateKey};
use sha1::{Digest as Sha1Digest, Sha1};
use sha2::Sha256;
use x509_cert::Certificate;

use crate::crypto::{CryptMethod, StandardSecurityHandler, resolve_v4_crypt_filters};
use crate::error::{PdfError, PdfResult};
use crate::types::{PdfDictionary, PdfValue};

/// RSA-PKCS1v15 (rsaEncryption).
const OID_RSA_ENCRYPTION: ObjectIdentifier =
    ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.1");
/// RSA-OAEP (id-RSAES-OAEP).
const OID_RSA_OAEP: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.7");
/// CMS EnvelopedData content type.
const OID_ENVELOPED_DATA: ObjectIdentifier =
    ObjectIdentifier::new_unwrap("1.2.840.113549.1.7.3");

/// Caller-supplied X.509 credential pair. Both buffers are DER-encoded:
/// the certificate is a standard X.509 v3 cert; the private key is a
/// PKCS#8 `PrivateKeyInfo` (or, less commonly, PKCS#1 `RSAPrivateKey`).
pub struct PubSecCredential<'a> {
    pub certificate_der: &'a [u8],
    pub private_key_der: &'a [u8],
}

/// Authenticates against the `/Encrypt` dictionary of an Adobe.PubSec
/// PDF and returns a configured decryption handler. Returns
/// [`PdfError::InvalidPassword`] when no recipient blob unwraps with the
/// supplied private key (matches the Standard handler's "credential did
/// not authenticate" error semantics).
pub fn open_pubsec(
    encrypt_dict: &PdfDictionary,
    credential: &PubSecCredential,
) -> PdfResult<StandardSecurityHandler> {
    let v = encrypt_dict
        .get("V")
        .and_then(PdfValue::as_integer)
        .unwrap_or(0);
    let sub_filter = encrypt_dict
        .get("SubFilter")
        .and_then(PdfValue::as_name)
        .unwrap_or("");

    if !matches!(sub_filter, "adbe.pkcs7.s4" | "adbe.pkcs7.s5") {
        return Err(PdfError::Unsupported(format!(
            "Adobe.PubSec /SubFilter /{sub_filter} is not supported (only adbe.pkcs7.s4 and adbe.pkcs7.s5)"
        )));
    }

    // Locate /Recipients. For s4 it lives at the top level of the
    // encrypt dict; for s5 it lives inside the active /CF crypt-filter
    // dictionary.
    let recipient_blobs = collect_recipient_blobs(encrypt_dict, sub_filter, v)?;
    if recipient_blobs.is_empty() {
        return Err(PdfError::Corrupt(
            "Adobe.PubSec /Encrypt has no /Recipients".to_string(),
        ));
    }

    // Parse caller's certificate so we can match it against each
    // recipient's RecipientIdentifier.
    let recipient_cert = Certificate::from_der(credential.certificate_der).map_err(|err| {
        PdfError::Corrupt(format!("recipient certificate is not valid DER: {err}"))
    })?;
    // Load the RSA private key. Try PKCS#8 first (typical), then
    // PKCS#1 RSAPrivateKey.
    let private_key = load_rsa_private_key(credential.private_key_der)?;

    // Concatenated recipient blob bytes feed into the file-key hash for
    // both s4 and s5; capture them verbatim from the PDF before we
    // start any RSA decryption.
    let mut recipients_buffer: Vec<u8> = Vec::new();
    for blob in &recipient_blobs {
        recipients_buffer.extend_from_slice(blob);
    }

    // Try each recipient blob until one of them unwraps with our key.
    let mut decrypted_seed_and_perms: Option<Vec<u8>> = None;
    for blob in &recipient_blobs {
        if let Some(plaintext) = try_unwrap_recipient(blob, &recipient_cert, &private_key)? {
            decrypted_seed_and_perms = Some(plaintext);
            break;
        }
    }

    let plaintext = decrypted_seed_and_perms.ok_or(PdfError::InvalidPassword)?;
    if plaintext.len() < 24 {
        return Err(PdfError::Corrupt(
            "decrypted PubSec seed must be at least 24 bytes (20-byte seed + 4-byte permissions)"
                .to_string(),
        ));
    }
    let seed = &plaintext[..20];
    let permission_bytes = &plaintext[20..24];

    // Derive the file encryption key per SubFilter.
    let file_key: Vec<u8> = match sub_filter {
        "adbe.pkcs7.s4" => {
            let mut hasher = Sha1::new();
            hasher.update(seed);
            hasher.update(&recipients_buffer);
            hasher.update(permission_bytes);
            hasher.finalize().to_vec()
        }
        "adbe.pkcs7.s5" => {
            let mut hasher = Sha256::new();
            hasher.update(seed);
            hasher.update(&recipients_buffer);
            hasher.update(permission_bytes);
            hasher.finalize().to_vec()
        }
        _ => unreachable!("sub_filter validated above"),
    };

    // Choose crypt methods.
    let (string_method, stream_method) = match v {
        4 => resolve_v4_crypt_filters(encrypt_dict)?,
        5 => (CryptMethod::AesV3, CryptMethod::AesV3),
        other => {
            return Err(PdfError::Unsupported(format!(
                "Adobe.PubSec V={other} is not supported (only V=4 and V=5)"
            )));
        }
    };

    // Truncate the file key to the symmetric algorithm's key length.
    let key_length_bytes = match v {
        4 => 16,
        5 => 32,
        _ => unreachable!("v validated above"),
    };
    let truncated_file_key = file_key[..key_length_bytes.min(file_key.len())].to_vec();

    let encrypt_metadata = encrypt_dict
        .get("EncryptMetadata")
        .and_then(PdfValue::as_bool)
        .unwrap_or(true);

    Ok(StandardSecurityHandler::from_file_key(
        truncated_file_key,
        string_method,
        stream_method,
        encrypt_metadata,
    ))
}

/// Return all recipient blob byte strings as raw bytes. For V=4 they
/// live at `encrypt_dict["Recipients"]`; for V=5 they live inside the
/// crypt filter named by `encrypt_dict["StmF"]` (or "DefaultCryptFilter"
/// fallback) under `encrypt_dict["CF"]`.
fn collect_recipient_blobs(
    encrypt_dict: &PdfDictionary,
    sub_filter: &str,
    v: i64,
) -> PdfResult<Vec<Vec<u8>>> {
    let array = match (v, sub_filter) {
        (4, "adbe.pkcs7.s4") => encrypt_dict.get("Recipients").and_then(PdfValue::as_array),
        (5, "adbe.pkcs7.s5") => {
            let cf = encrypt_dict
                .get("CF")
                .and_then(PdfValue::as_dictionary)
                .ok_or_else(|| {
                    PdfError::Corrupt("Adobe.PubSec V=5 requires /CF dictionary".to_string())
                })?;
            let stmf = encrypt_dict
                .get("StmF")
                .and_then(PdfValue::as_name)
                .unwrap_or("DefaultCryptFilter");
            let filter_dict = cf
                .get(stmf)
                .and_then(PdfValue::as_dictionary)
                .ok_or_else(|| {
                    PdfError::Corrupt(format!(
                        "Adobe.PubSec V=5 /CF entry /{stmf} is missing or not a dictionary"
                    ))
                })?;
            filter_dict.get("Recipients").and_then(PdfValue::as_array)
        }
        _ => {
            return Err(PdfError::Unsupported(format!(
                "Adobe.PubSec V={v} /SubFilter /{sub_filter} combination is not supported"
            )));
        }
    };

    let array = array.ok_or_else(|| {
        PdfError::Corrupt(format!(
            "Adobe.PubSec /Recipients array is missing for /SubFilter /{sub_filter}"
        ))
    })?;

    let mut blobs = Vec::with_capacity(array.len());
    for entry in array {
        let bytes = match entry {
            PdfValue::String(s) => s.0.clone(),
            _ => {
                return Err(PdfError::Corrupt(
                    "Adobe.PubSec /Recipients entry must be a byte string".to_string(),
                ));
            }
        };
        blobs.push(bytes);
    }
    Ok(blobs)
}

/// Try to unwrap one recipient blob with the caller's RSA private key.
/// Returns `Ok(Some(seed_and_perms))` when the blob's
/// `RecipientIdentifier` matches the caller's certificate AND the full
/// CMS decryption succeeds (RSA-unwrap of the CEK followed by symmetric
/// decryption of the inner content). Returns `Ok(None)` when the blob
/// is for a different recipient; `Err` for malformed CMS, an unsupported
/// inner cipher, or a key-format mismatch.
fn try_unwrap_recipient(
    blob: &[u8],
    recipient_cert: &Certificate,
    private_key: &RsaPrivateKey,
) -> PdfResult<Option<Vec<u8>>> {
    let content_info = ContentInfo::from_der(blob).map_err(|err| {
        PdfError::Corrupt(format!(
            "Adobe.PubSec recipient blob is not a valid CMS ContentInfo: {err}"
        ))
    })?;
    if content_info.content_type != OID_ENVELOPED_DATA {
        return Err(PdfError::Corrupt(format!(
            "Adobe.PubSec recipient blob has wrong CMS content type {:?}",
            content_info.content_type
        )));
    }
    let inner_der = content_info.content.to_der().map_err(|err| {
        PdfError::Corrupt(format!("CMS inner re-encode failed: {err}"))
    })?;
    let enveloped = EnvelopedData::from_der(&inner_der).map_err(|err| {
        PdfError::Corrupt(format!("CMS EnvelopedData decode failed: {err}"))
    })?;

    let mut content_encryption_key: Option<Vec<u8>> = None;
    for ri in enveloped.recip_infos.0.iter() {
        let ktri = match ri {
            RecipientInfo::Ktri(ktri) => ktri,
            RecipientInfo::Kari(_) => {
                return Err(PdfError::Unsupported(
                    "Adobe.PubSec key-agreement recipients (KeyAgreeRecipientInfo) are not supported"
                        .to_string(),
                ));
            }
            _ => continue, // PWRI / KEKRI / OtherRecipientInfo — skip.
        };

        if !rid_matches(&ktri.rid, recipient_cert) {
            continue;
        }

        let cek = rsa_decrypt(private_key, ktri.key_enc_alg.oid, ktri.enc_key.as_bytes())?;
        content_encryption_key = Some(cek);
        break;
    }

    let Some(cek) = content_encryption_key else {
        return Ok(None);
    };

    // Decrypt the inner symmetric layer: AES-CBC over (seed || perms).
    let alg = &enveloped.encrypted_content.content_enc_alg;
    let ciphertext = enveloped
        .encrypted_content
        .encrypted_content
        .as_ref()
        .ok_or_else(|| {
            PdfError::Corrupt(
                "CMS EnvelopedData encryptedContent is missing".to_string(),
            )
        })?
        .as_bytes();
    let iv_param = alg.parameters.as_ref().ok_or_else(|| {
        PdfError::Corrupt("CMS content encryption algorithm has no parameters".to_string())
    })?;
    let iv_bytes = iv_param
        .decode_as::<der::asn1::OctetString>()
        .map_err(|err| {
            PdfError::Corrupt(format!(
                "CMS content encryption IV is not an OCTET STRING: {err}"
            ))
        })?;
    let iv = iv_bytes.as_bytes();

    let plaintext = decrypt_cms_inner(alg.oid, &cek, iv, ciphertext)?;
    Ok(Some(plaintext))
}

/// AES-CBC PKCS#7 decrypt for the inner content layer of CMS
/// `EnvelopedData`. Supports AES-128 / AES-192 / AES-256 (selected by
/// the algorithm OID). The IV is supplied separately because CMS
/// puts it in the algorithm parameters, not embedded as a prefix.
fn decrypt_cms_inner(
    algorithm_oid: ObjectIdentifier,
    cek: &[u8],
    iv: &[u8],
    ciphertext: &[u8],
) -> PdfResult<Vec<u8>> {
    use aes::cipher::{BlockDecrypt, KeyInit, generic_array::GenericArray};
    use aes::{Aes128, Aes192, Aes256};

    if iv.len() != 16 {
        return Err(PdfError::Corrupt(format!(
            "CMS AES-CBC IV must be 16 bytes, got {}",
            iv.len()
        )));
    }
    if ciphertext.is_empty() || ciphertext.len() % 16 != 0 {
        return Err(PdfError::Corrupt(format!(
            "CMS AES-CBC ciphertext length {} is not a positive multiple of 16",
            ciphertext.len()
        )));
    }

    const AES_128_CBC: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.16.840.1.101.3.4.1.2");
    const AES_192_CBC: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.16.840.1.101.3.4.1.22");
    const AES_256_CBC: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.16.840.1.101.3.4.1.42");

    let mut prev: [u8; 16] = iv.try_into().expect("iv length checked");
    let mut output = Vec::with_capacity(ciphertext.len());

    macro_rules! decrypt_with {
        ($cipher:ty, $expected_key:expr) => {{
            if cek.len() != $expected_key {
                return Err(PdfError::Corrupt(format!(
                    "CEK length {} does not match algorithm key size {}",
                    cek.len(),
                    $expected_key
                )));
            }
            let cipher = <$cipher>::new_from_slice(cek).map_err(|err| {
                PdfError::Corrupt(format!("AES init failed: {err}"))
            })?;
            for chunk in ciphertext.chunks(16) {
                let mut block = GenericArray::clone_from_slice(chunk);
                cipher.decrypt_block(&mut block);
                for (plain_byte, iv_byte) in block.iter_mut().zip(prev.iter()) {
                    *plain_byte ^= iv_byte;
                }
                output.extend_from_slice(block.as_slice());
                prev.copy_from_slice(chunk);
            }
        }};
    }

    if algorithm_oid == AES_128_CBC {
        decrypt_with!(Aes128, 16);
    } else if algorithm_oid == AES_192_CBC {
        decrypt_with!(Aes192, 24);
    } else if algorithm_oid == AES_256_CBC {
        decrypt_with!(Aes256, 32);
    } else {
        return Err(PdfError::Unsupported(format!(
            "CMS content encryption algorithm {algorithm_oid} is not supported (only AES-CBC)"
        )));
    }

    // Strip PKCS#7 padding.
    let pad = *output.last().ok_or_else(|| {
        PdfError::Corrupt("CMS AES-CBC plaintext is empty after decrypt".to_string())
    })?;
    if pad == 0 || pad > 16 || pad as usize > output.len() {
        return Err(PdfError::Corrupt(format!(
            "invalid PKCS#7 padding length {pad} in CMS plaintext"
        )));
    }
    let new_len = output.len() - pad as usize;
    output.truncate(new_len);
    Ok(output)
}

/// Compare a CMS `RecipientIdentifier` to the caller's certificate.
/// `IssuerAndSerialNumber` must match issuer DN + serial; `SubjectKeyIdentifier`
/// must equal the cert's `subjectKeyIdentifier` extension if present.
fn rid_matches(rid: &RecipientIdentifier, cert: &Certificate) -> bool {
    match rid {
        RecipientIdentifier::IssuerAndSerialNumber(iasn) => {
            iasn.issuer == cert.tbs_certificate.issuer
                && iasn.serial_number == cert.tbs_certificate.serial_number
        }
        RecipientIdentifier::SubjectKeyIdentifier(ski) => {
            // Walk the cert's extensions looking for SKI.
            let Some(extensions) = cert.tbs_certificate.extensions.as_ref() else {
                return false;
            };
            for ext in extensions {
                if ext.extn_id == const_oid::db::rfc5912::ID_CE_SUBJECT_KEY_IDENTIFIER {
                    return ext.extn_value.as_bytes() == ski.0.as_bytes();
                }
            }
            false
        }
    }
}

/// RSA-decrypt the recipient's encrypted key. PKCS1v15 is the default
/// for Acrobat output; OAEP is rare but spec-permitted.
fn rsa_decrypt(
    private_key: &RsaPrivateKey,
    algorithm_oid: ObjectIdentifier,
    ciphertext: &[u8],
) -> PdfResult<Vec<u8>> {
    if algorithm_oid == OID_RSA_ENCRYPTION {
        private_key
            .decrypt(Pkcs1v15Encrypt, ciphertext)
            .map_err(|err| PdfError::Corrupt(format!("RSA-PKCS1v15 unwrap failed: {err}")))
    } else if algorithm_oid == OID_RSA_OAEP {
        private_key
            .decrypt(Oaep::new::<Sha1>(), ciphertext)
            .map_err(|err| PdfError::Corrupt(format!("RSA-OAEP unwrap failed: {err}")))
    } else {
        Err(PdfError::Unsupported(format!(
            "Adobe.PubSec key-encryption OID {algorithm_oid} is not supported"
        )))
    }
}

/// Load a DER-encoded RSA private key. Accepts PKCS#8 `PrivateKeyInfo`
/// (most common for browser-side PEM-to-DER conversions) and falls back
/// to PKCS#1 `RSAPrivateKey` when PKCS#8 parsing fails.
fn load_rsa_private_key(der: &[u8]) -> PdfResult<RsaPrivateKey> {
    use rsa::pkcs1::DecodeRsaPrivateKey;
    use rsa::pkcs8::DecodePrivateKey;

    if let Ok(key) = RsaPrivateKey::from_pkcs8_der(der) {
        return Ok(key);
    }
    RsaPrivateKey::from_pkcs1_der(der).map_err(|err| {
        PdfError::Corrupt(format!(
            "private key is neither valid PKCS#8 nor PKCS#1 RSA DER: {err}"
        ))
    })
}
