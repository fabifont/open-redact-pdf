use std::collections::{BTreeMap, BTreeSet};

use crate::crypto::{BytesKind, StandardSecurityHandler};
use crate::document::build_document;
use crate::error::{PdfError, PdfResult};
use crate::pubsec::{PubSecCredential, open_pubsec};
use crate::stream::decode_stream;
use crate::types::{
    ObjectRef, PdfDictionary, PdfFile, PdfObject, PdfStream, PdfString, PdfValue, XrefEntry,
    XrefForm,
};

/// Caller-supplied credential for opening an encrypted PDF. Standard
/// security handlers authenticate by password; the public-key handler
/// (`/Filter /Adobe.PubSec`) authenticates by an X.509 certificate plus
/// its RSA private key. For unencrypted PDFs the credential is ignored
/// (the empty `Password(b"")` is the natural default).
#[derive(Clone, Copy)]
pub enum PdfCredential<'a> {
    Password(&'a [u8]),
    Certificate {
        cert_der: &'a [u8],
        private_key_der: &'a [u8],
    },
}

/// Parses an unencrypted PDF, or an encrypted PDF whose user password is
/// empty. For encrypted PDFs that require a user- or owner-supplied
/// password, use [`parse_pdf_with_password`]; for `/Filter /Adobe.PubSec`
/// PDFs, use [`parse_pdf_with_certificate`].
pub fn parse_pdf(bytes: &[u8]) -> PdfResult<crate::document::ParsedDocument> {
    parse_pdf_with_credential(bytes, PdfCredential::Password(b""))
}

/// Parses an encrypted PDF with a caller-supplied password. The password
/// is tried first as the user password, then as the owner password; if
/// neither authenticates, the function returns
/// [`PdfError::InvalidPassword`]. For unencrypted documents the password
/// is ignored.
pub fn parse_pdf_with_password(
    bytes: &[u8],
    password: &[u8],
) -> PdfResult<crate::document::ParsedDocument> {
    parse_pdf_with_credential(bytes, PdfCredential::Password(password))
}

/// Parses an Adobe.PubSec-encrypted PDF using a recipient X.509
/// certificate (DER) and its matching PKCS#8 private key (DER). Returns
/// [`PdfError::InvalidPassword`] when no recipient blob in the PDF
/// unwraps with the supplied private key. For password-encrypted or
/// unencrypted documents this returns
/// [`PdfError::Unsupported`] — use [`parse_pdf_with_password`] /
/// [`parse_pdf`] respectively.
pub fn parse_pdf_with_certificate(
    bytes: &[u8],
    cert_der: &[u8],
    private_key_der: &[u8],
) -> PdfResult<crate::document::ParsedDocument> {
    parse_pdf_with_credential(
        bytes,
        PdfCredential::Certificate {
            cert_der,
            private_key_der,
        },
    )
}

/// Generic entry point that accepts either credential variant. The
/// password and certificate wrappers above thread their arguments
/// through this function.
pub fn parse_pdf_with_credential(
    bytes: &[u8],
    credential: PdfCredential,
) -> PdfResult<crate::document::ParsedDocument> {
    let version = parse_header(bytes)?;
    let startxref = find_startxref(bytes)?;
    let (xref, mut trailer, xref_form) = parse_xref_table(bytes, startxref)?;

    let mut objects = BTreeMap::new();
    let mut max_object_number = 0;
    let mut compressed: Vec<(ObjectRef, u32, u32)> = Vec::new();

    for (object_ref, entry) in &xref {
        match entry {
            XrefEntry::Free => {}
            XrefEntry::Uncompressed { offset, .. } => {
                if object_ref.object_number == 0 {
                    continue;
                }
                let object = parse_indirect_object(bytes, *offset, Some(&xref))?;
                max_object_number = max_object_number.max(object_ref.object_number);
                objects.insert(*object_ref, object);
            }
            XrefEntry::Compressed {
                stream_object_number,
                index,
            } => {
                compressed.push((*object_ref, *stream_object_number, *index));
            }
        }
    }

    // Decrypt in place before materializing object streams: the ObjStm stream
    // itself is encrypted, but once its bytes are decrypted the contained
    // members are plaintext and materialize_object_streams can proceed as
    // usual. Order matters — if we materialized first, each ObjStm's decoded
    // body would still be ciphertext and we'd parse garbage.
    decrypt_document_if_encrypted(&mut objects, &mut trailer, credential)?;

    materialize_object_streams(&mut objects, &mut max_object_number, &compressed)?;

    let file = PdfFile {
        version,
        objects,
        trailer,
        max_object_number,
        xref_form,
    };
    build_document(file)
}

fn decrypt_document_if_encrypted(
    objects: &mut BTreeMap<ObjectRef, PdfObject>,
    trailer: &mut PdfDictionary,
    credential: PdfCredential,
) -> PdfResult<()> {
    let encrypt_ref = match trailer.get("Encrypt") {
        Some(PdfValue::Reference(object_ref)) => *object_ref,
        Some(PdfValue::Dictionary(_)) => {
            return Err(PdfError::Unsupported(
                "direct (non-indirect) /Encrypt dictionaries are not supported".to_string(),
            ));
        }
        Some(_) => {
            return Err(PdfError::Corrupt(
                "trailer /Encrypt is not a reference".to_string(),
            ));
        }
        None => return Ok(()),
    };

    let encrypt_dict = match objects.get(&encrypt_ref) {
        Some(PdfObject::Value(PdfValue::Dictionary(dict))) => dict.clone(),
        _ => {
            return Err(PdfError::Corrupt(
                "trailer /Encrypt does not point at a dictionary".to_string(),
            ));
        }
    };

    let filter_name = encrypt_dict
        .get("Filter")
        .and_then(PdfValue::as_name)
        .unwrap_or("");

    let handler = match filter_name {
        "Standard" => match credential {
            PdfCredential::Password(password) => {
                let id_first = extract_id_first(trailer)?;
                StandardSecurityHandler::open(&encrypt_dict, &id_first, password)?
                    .ok_or(PdfError::InvalidPassword)?
            }
            PdfCredential::Certificate { .. } => {
                return Err(PdfError::Unsupported(
                    "/Filter /Standard requires a password, not a certificate".to_string(),
                ));
            }
        },
        "Adobe.PubSec" => match credential {
            PdfCredential::Certificate {
                cert_der,
                private_key_der,
            } => open_pubsec(
                &encrypt_dict,
                &PubSecCredential {
                    certificate_der: cert_der,
                    private_key_der,
                },
            )?,
            PdfCredential::Password(_) => {
                return Err(PdfError::Unsupported(
                    "/Filter /Adobe.PubSec requires a certificate, not a password".to_string(),
                ));
            }
        },
        other => {
            return Err(PdfError::Unsupported(format!(
                "encryption filter /{other} is not supported"
            )));
        }
    };

    let refs: Vec<ObjectRef> = objects.keys().copied().collect();
    for object_ref in refs {
        if object_ref == encrypt_ref {
            // Strings and streams in the Encrypt dictionary itself are
            // exempt from encryption (PDF 1.7 §7.6.1).
            continue;
        }
        let object = objects
            .get_mut(&object_ref)
            .expect("ref obtained from map keys must still be present");
        match object {
            PdfObject::Stream(stream) => {
                // Cross-reference streams are never encrypted; metadata
                // streams are exempt when the document sets
                // /EncryptMetadata false (Tr. ISO 32000-1 §7.6.1).
                let type_name = stream.dict.get("Type").and_then(PdfValue::as_name);
                let is_xref_stream = type_name == Some("XRef");
                let is_exempt_metadata =
                    !handler.encrypts_metadata() && type_name == Some("Metadata");
                decrypt_strings_in_dict(&mut stream.dict, &handler, object_ref)?;
                if !is_xref_stream && !is_exempt_metadata {
                    stream.data =
                        handler.decrypt_bytes(&stream.data, object_ref, BytesKind::Stream)?;
                }
            }
            PdfObject::Value(value) => {
                decrypt_strings_in_value(value, &handler, object_ref)?;
            }
        }
    }

    trailer.remove("Encrypt");
    // Remove the Encrypt dictionary object itself so the writer never
    // emits its now-decrypted /O, /U, /OE, /UE, /Perms fields as
    // dangling unreferenced bytes (would leak the password verifiers).
    objects.remove(&encrypt_ref);
    Ok(())
}

fn extract_id_first(trailer: &PdfDictionary) -> PdfResult<Vec<u8>> {
    match trailer.get("ID") {
        Some(PdfValue::Array(entries)) => match entries.first() {
            Some(PdfValue::String(value)) => Ok(value.0.clone()),
            _ => Err(PdfError::Corrupt(
                "trailer /ID[0] is not a string — cannot derive encryption key".to_string(),
            )),
        },
        _ => Err(PdfError::Corrupt(
            "encrypted PDF is missing the trailer /ID array required for key derivation"
                .to_string(),
        )),
    }
}

fn decrypt_strings_in_value(
    value: &mut PdfValue,
    handler: &StandardSecurityHandler,
    object_ref: ObjectRef,
) -> PdfResult<()> {
    match value {
        PdfValue::String(string) => {
            string.0 = handler.decrypt_bytes(&string.0, object_ref, BytesKind::String)?;
        }
        PdfValue::Array(items) => {
            for item in items {
                decrypt_strings_in_value(item, handler, object_ref)?;
            }
        }
        PdfValue::Dictionary(dict) => {
            decrypt_strings_in_dict(dict, handler, object_ref)?;
        }
        _ => {}
    }
    Ok(())
}

fn decrypt_strings_in_dict(
    dict: &mut PdfDictionary,
    handler: &StandardSecurityHandler,
    object_ref: ObjectRef,
) -> PdfResult<()> {
    for value in dict.values_mut() {
        decrypt_strings_in_value(value, handler, object_ref)?;
    }
    Ok(())
}

fn parse_header(bytes: &[u8]) -> PdfResult<String> {
    if !bytes.starts_with(b"%PDF-") {
        return Err(PdfError::Parse("missing PDF header".to_string()));
    }
    let line_end = bytes
        .iter()
        .position(|byte| *byte == b'\n' || *byte == b'\r')
        .ok_or_else(|| PdfError::Parse("unterminated header".to_string()))?;
    Ok(String::from_utf8_lossy(&bytes[5..line_end])
        .trim()
        .to_string())
}

fn find_startxref(bytes: &[u8]) -> PdfResult<usize> {
    let marker = b"startxref";
    let position = bytes
        .windows(marker.len())
        .rposition(|window| window == marker)
        .ok_or_else(|| PdfError::Parse("missing startxref".to_string()))?;
    let mut parser = Cursor::new(bytes, position + marker.len());
    parser.skip_ws_and_comments();
    parser.parse_usize()
}

fn parse_xref_table(
    bytes: &[u8],
    start_offset: usize,
) -> PdfResult<(BTreeMap<ObjectRef, XrefEntry>, PdfDictionary, XrefForm)> {
    let mut merged_entries: BTreeMap<ObjectRef, XrefEntry> = BTreeMap::new();
    let mut newest_trailer: Option<PdfDictionary> = None;
    // The form of the very first section we visit (the one at startxref)
    // determines the output shape. Older sections reached via /Prev or
    // /XRefStm may use the opposite form, but the writer mirrors the
    // newest section's shape only.
    let mut top_form: Option<XrefForm> = None;
    let mut visited = BTreeSet::new();
    let mut pending: Vec<usize> = vec![start_offset];

    while let Some(offset) = pending.pop() {
        if !visited.insert(offset) {
            continue;
        }
        let section = parse_xref_section_at(bytes, offset)?;

        // Newest-first: only insert entries not already present
        for (object_ref, entry) in section.entries {
            merged_entries.entry(object_ref).or_insert(entry);
        }

        if newest_trailer.is_none() {
            newest_trailer = Some(section.trailer.clone());
            top_form = Some(section.form);
        }

        if let Some(stm_offset) = section
            .trailer
            .get("XRefStm")
            .and_then(PdfValue::as_integer)
        {
            pending.push(stm_offset as usize);
        }
        if let Some(prev_offset) = section.trailer.get("Prev").and_then(PdfValue::as_integer) {
            pending.push(prev_offset as usize);
        }
    }

    let trailer = newest_trailer
        .ok_or_else(|| PdfError::Parse("xref chain produced no trailer".to_string()))?;
    let form = top_form.unwrap_or(XrefForm::Classic);
    Ok((merged_entries, trailer, form))
}

struct XrefSection {
    entries: BTreeMap<ObjectRef, XrefEntry>,
    trailer: PdfDictionary,
    form: XrefForm,
}

fn parse_xref_section_at(bytes: &[u8], offset: usize) -> PdfResult<XrefSection> {
    let mut probe = Cursor::new(bytes, offset);
    probe.skip_ws_and_comments();
    if probe.peek_keyword("xref") {
        parse_classic_xref_section(bytes, offset)
    } else {
        parse_xref_stream_section(bytes, offset)
    }
}

fn parse_classic_xref_section(bytes: &[u8], offset: usize) -> PdfResult<XrefSection> {
    let mut cursor = Cursor::new(bytes, offset);
    cursor.expect_keyword("xref")?;
    let mut entries = BTreeMap::new();
    loop {
        cursor.skip_ws_and_comments();
        if cursor.peek_keyword("trailer") {
            break;
        }
        let start = cursor.parse_u32()?;
        cursor.skip_ws_and_comments();
        let count = cursor.parse_u32()?;
        cursor.skip_line_breaks();
        for index in 0..count {
            let line = cursor.read_line()?;
            if line.len() < 17 {
                return Err(PdfError::Parse("invalid xref entry".to_string()));
            }
            let parts = String::from_utf8_lossy(line).trim().to_string();
            let mut fields = parts.split_whitespace();
            let entry_offset = fields
                .next()
                .ok_or_else(|| PdfError::Parse("invalid xref entry offset".to_string()))?
                .parse::<usize>()
                .map_err(|_| PdfError::Parse("invalid xref entry offset".to_string()))?;
            let generation = fields
                .next()
                .ok_or_else(|| PdfError::Parse("invalid xref generation".to_string()))?
                .parse::<u16>()
                .map_err(|_| PdfError::Parse("invalid xref generation".to_string()))?;
            let flag = fields
                .next()
                .ok_or_else(|| PdfError::Parse("invalid xref flag".to_string()))?;
            let object_number = start
                .checked_add(index)
                .ok_or_else(|| PdfError::Parse("xref object number overflow".to_string()))?;
            let entry = if flag == "n" {
                XrefEntry::Uncompressed {
                    offset: entry_offset,
                    generation,
                }
            } else {
                XrefEntry::Free
            };
            entries.insert(ObjectRef::new(object_number, generation), entry);
        }
    }
    cursor.expect_keyword("trailer")?;
    let trailer = match cursor.parse_value()? {
        PdfValue::Dictionary(dictionary) => dictionary,
        _ => return Err(PdfError::Parse("trailer is not a dictionary".to_string())),
    };
    Ok(XrefSection {
        entries,
        trailer,
        form: XrefForm::Classic,
    })
}

fn parse_xref_stream_section(bytes: &[u8], offset: usize) -> PdfResult<XrefSection> {
    // The xref stream itself is read while the xref map is still being
    // built, so there is no xref available to resolve indirect /Length
    // references. Pass `None` and fall back to the endstream scan if the
    // xref stream ever uses an indirect /Length (vanishingly rare).
    let object = parse_indirect_object(bytes, offset, None)?;
    let stream = match object {
        PdfObject::Stream(stream) => stream,
        PdfObject::Value(_) => {
            return Err(PdfError::Parse(
                "expected xref stream object at startxref offset".to_string(),
            ));
        }
    };
    if stream.dict.get("Type").and_then(PdfValue::as_name) != Some("XRef") {
        return Err(PdfError::Parse(
            "xref stream object has wrong Type".to_string(),
        ));
    }

    let size = stream
        .dict
        .get("Size")
        .and_then(PdfValue::as_integer)
        .ok_or_else(|| PdfError::Corrupt("xref stream missing Size".to_string()))?
        as u32;

    let w = stream
        .dict
        .get("W")
        .and_then(PdfValue::as_array)
        .ok_or_else(|| PdfError::Corrupt("xref stream missing W".to_string()))?;
    if w.len() != 3 {
        return Err(PdfError::Corrupt(
            "xref stream W must have three entries".to_string(),
        ));
    }
    let w0 = w[0]
        .as_integer()
        .ok_or_else(|| PdfError::Corrupt("invalid W[0]".to_string()))? as usize;
    let w1 = w[1]
        .as_integer()
        .ok_or_else(|| PdfError::Corrupt("invalid W[1]".to_string()))? as usize;
    let w2 = w[2]
        .as_integer()
        .ok_or_else(|| PdfError::Corrupt("invalid W[2]".to_string()))? as usize;
    let row_len = w0 + w1 + w2;
    if row_len == 0 {
        return Err(PdfError::Corrupt(
            "xref stream row width is zero".to_string(),
        ));
    }

    let index: Vec<(u32, u32)> = match stream.dict.get("Index") {
        Some(PdfValue::Array(entries)) => {
            if entries.len() % 2 != 0 {
                return Err(PdfError::Corrupt(
                    "xref stream Index must have an even number of entries".to_string(),
                ));
            }
            let mut pairs = Vec::with_capacity(entries.len() / 2);
            for chunk in entries.chunks(2) {
                let first = chunk[0]
                    .as_integer()
                    .ok_or_else(|| PdfError::Corrupt("invalid Index entry".to_string()))?
                    as u32;
                let count = chunk[1]
                    .as_integer()
                    .ok_or_else(|| PdfError::Corrupt("invalid Index entry".to_string()))?
                    as u32;
                pairs.push((first, count));
            }
            pairs
        }
        Some(_) => {
            return Err(PdfError::Corrupt(
                "xref stream Index is not an array".to_string(),
            ));
        }
        None => vec![(0, size)],
    };

    let decoded = decode_stream(&stream)?;
    let expected_rows: u32 = index.iter().map(|(_, count)| *count).sum();
    if decoded.len() < expected_rows as usize * row_len {
        return Err(PdfError::Corrupt(
            "xref stream body is shorter than declared entries".to_string(),
        ));
    }

    let mut entries: BTreeMap<ObjectRef, XrefEntry> = BTreeMap::new();
    let mut cursor = 0usize;
    for (first, count) in index {
        for i in 0..count {
            let row = &decoded[cursor..cursor + row_len];
            cursor += row_len;
            let field_type = if w0 == 0 { 1u64 } else { read_be(&row[..w0])? };
            let f2 = read_be(&row[w0..w0 + w1])?;
            let f3 = read_be(&row[w0 + w1..])?;
            let object_number = first + i;
            let entry = match field_type {
                0 => XrefEntry::Free,
                1 => XrefEntry::Uncompressed {
                    offset: f2 as usize,
                    generation: f3 as u16,
                },
                2 => XrefEntry::Compressed {
                    stream_object_number: f2 as u32,
                    index: f3 as u32,
                },
                other => {
                    return Err(PdfError::Unsupported(format!(
                        "xref stream entry type {other} is not supported"
                    )));
                }
            };
            let generation = match entry {
                XrefEntry::Uncompressed { generation, .. } => generation,
                _ => 0,
            };
            entries.insert(ObjectRef::new(object_number, generation), entry);
        }
    }

    Ok(XrefSection {
        entries,
        trailer: stream.dict,
        form: XrefForm::Stream,
    })
}

fn read_be(bytes: &[u8]) -> PdfResult<u64> {
    if bytes.len() > 8 {
        return Err(PdfError::Corrupt(
            "xref stream field width exceeds 8 bytes".to_string(),
        ));
    }
    let mut value: u64 = 0;
    for byte in bytes {
        value = (value << 8) | *byte as u64;
    }
    Ok(value)
}

fn materialize_object_streams(
    objects: &mut BTreeMap<ObjectRef, PdfObject>,
    max_object_number: &mut u32,
    compressed: &[(ObjectRef, u32, u32)],
) -> PdfResult<()> {
    if compressed.is_empty() {
        return Ok(());
    }

    let mut by_stream: BTreeMap<u32, Vec<(ObjectRef, u32)>> = BTreeMap::new();
    for (object_ref, stream_obj_num, index) in compressed {
        by_stream
            .entry(*stream_obj_num)
            .or_default()
            .push((*object_ref, *index));
    }

    for (stream_obj_num, mut members) in by_stream {
        let stream_ref = ObjectRef::new(stream_obj_num, 0);
        let stream = match objects.get(&stream_ref) {
            Some(PdfObject::Stream(stream)) => stream.clone(),
            Some(PdfObject::Value(_)) => {
                return Err(PdfError::Corrupt(format!(
                    "object stream {stream_obj_num} is not a stream"
                )));
            }
            None => {
                return Err(PdfError::Corrupt(format!(
                    "compressed entry references missing object stream {stream_obj_num}"
                )));
            }
        };
        if stream.dict.get("Type").and_then(PdfValue::as_name) != Some("ObjStm") {
            return Err(PdfError::Corrupt(format!(
                "object {stream_obj_num} is not marked as ObjStm"
            )));
        }
        let n = stream
            .dict
            .get("N")
            .and_then(PdfValue::as_integer)
            .ok_or_else(|| PdfError::Corrupt("ObjStm missing N".to_string()))?
            as usize;
        let first = stream
            .dict
            .get("First")
            .and_then(PdfValue::as_integer)
            .ok_or_else(|| PdfError::Corrupt("ObjStm missing First".to_string()))?
            as usize;

        let decoded = decode_stream(&stream)?;
        if first > decoded.len() {
            return Err(PdfError::Corrupt(
                "ObjStm First offset is past end of decoded data".to_string(),
            ));
        }

        let header = &decoded[..first];
        let mut header_cursor = Cursor::new(header, 0);
        let mut entries: Vec<(u32, usize)> = Vec::with_capacity(n);
        for _ in 0..n {
            header_cursor.skip_ws_and_comments();
            let obj_num = header_cursor.parse_u32()?;
            header_cursor.skip_ws_and_comments();
            let rel_offset = header_cursor.parse_usize()?;
            entries.push((obj_num, rel_offset));
        }

        // Guard: a compressed entry's index must be in range.
        members.sort_by_key(|(_, index)| *index);
        for (member_ref, index) in members {
            let idx = index as usize;
            if idx >= entries.len() {
                return Err(PdfError::Corrupt(format!(
                    "ObjStm {stream_obj_num} has no index {idx}"
                )));
            }
            let (declared_number, rel_offset) = entries[idx];
            if declared_number != member_ref.object_number {
                return Err(PdfError::Corrupt(format!(
                    "ObjStm {stream_obj_num} index {idx} has number {declared_number} but xref expected {}",
                    member_ref.object_number
                )));
            }
            let absolute_offset = first
                .checked_add(rel_offset)
                .ok_or_else(|| PdfError::Corrupt("ObjStm offset overflow".to_string()))?;
            if absolute_offset > decoded.len() {
                return Err(PdfError::Corrupt(
                    "ObjStm member offset is past end of decoded data".to_string(),
                ));
            }
            let mut value_cursor = Cursor::new(&decoded, absolute_offset);
            let value = value_cursor.parse_value()?;
            if let PdfValue::Dictionary(dict) = &value {
                if dict.get("Type").and_then(PdfValue::as_name) == Some("ObjStm") {
                    return Err(PdfError::Unsupported(
                        "nested object streams are not supported".to_string(),
                    ));
                }
            }
            *max_object_number = (*max_object_number).max(member_ref.object_number);
            objects.insert(member_ref, PdfObject::Value(value));
        }
        // Drop the ObjStm container after its members are materialised.
        // The container's compressed bytes mirror the pre-redaction state
        // of every member dictionary that was packed into it; leaving it
        // in `objects` would make the writer emit the original bytes
        // even after the materialised members were modified by redaction.
        objects.remove(&stream_ref);
    }

    Ok(())
}

fn parse_indirect_object(
    bytes: &[u8],
    offset: usize,
    xref: Option<&BTreeMap<ObjectRef, XrefEntry>>,
) -> PdfResult<PdfObject> {
    let mut cursor = Cursor::new(bytes, offset);
    let _object_number = cursor.parse_u32()?;
    cursor.skip_ws_and_comments();
    let _generation = cursor.parse_u16()?;
    cursor.skip_ws_and_comments();
    cursor.expect_keyword("obj")?;
    cursor.skip_ws_and_comments();

    let value = cursor.parse_value()?;
    cursor.skip_ws_and_comments();
    if matches!(value, PdfValue::Dictionary(_)) && cursor.peek_keyword("stream") {
        let dict = match value {
            PdfValue::Dictionary(dict) => dict,
            _ => unreachable!(),
        };
        cursor.expect_keyword("stream")?;
        cursor.consume_stream_line_break();
        let stream_start = cursor.position;
        // Prefer the Length entry from the stream dictionary to determine the
        // data boundary. This prevents binary stream data that happens to
        // contain the literal bytes "endstream" from being truncated. When
        // /Length is an indirect reference we resolve it by following the
        // xref entry for the referenced integer object; see
        // `resolve_stream_length_ref`. A missing or unresolvable /Length
        // falls back to scanning forward for `endstream`.
        let length_hint = match dict.get("Length") {
            Some(PdfValue::Integer(len)) if *len >= 0 => Some(*len as usize),
            Some(PdfValue::Reference(target)) => {
                xref.and_then(|map| resolve_stream_length_ref(bytes, map, *target))
            }
            _ => None,
        };
        let (data, endstream_pos) = match length_hint {
            Some(len) if stream_start + len <= bytes.len() => {
                // Verify the endstream keyword follows at the expected offset.
                // Tolerate trailing EOL between data and keyword per PDF spec.
                let mut check = stream_start + len;
                while check < bytes.len() && matches!(bytes[check], b'\r' | b'\n') {
                    check += 1;
                }
                if bytes.get(check..check + 9) == Some(b"endstream") {
                    (bytes[stream_start..stream_start + len].to_vec(), check)
                } else {
                    // Length is wrong; fall back to scanning
                    let pos = find_keyword(bytes, stream_start, b"endstream")
                        .ok_or_else(|| PdfError::Parse("stream missing endstream".to_string()))?;
                    (bytes[stream_start..pos].to_vec(), pos)
                }
            }
            _ => {
                let pos = find_keyword(bytes, stream_start, b"endstream")
                    .ok_or_else(|| PdfError::Parse("stream missing endstream".to_string()))?;
                (bytes[stream_start..pos].to_vec(), pos)
            }
        };
        cursor.position = endstream_pos;
        cursor.expect_keyword("endstream")?;
        cursor.skip_ws_and_comments();
        cursor.expect_keyword("endobj")?;
        Ok(PdfObject::Stream(PdfStream { dict, data }))
    } else {
        cursor.expect_keyword("endobj")?;
        Ok(PdfObject::Value(value))
    }
}

/// Resolve an indirect `/Length` reference inside a stream dictionary to
/// the plain non-negative integer it points at. Follows `target` through
/// the xref table, parses the referenced object, and returns its integer
/// value if and only if the resolved object is a plain integer value
/// (not a stream, reference, or negative integer). Returns `None` when
/// the target entry is missing, compressed, or the resolved value is not
/// a usable length; the caller then falls back to scanning for
/// `endstream`.
fn resolve_stream_length_ref(
    bytes: &[u8],
    xref: &BTreeMap<ObjectRef, XrefEntry>,
    target: ObjectRef,
) -> Option<usize> {
    let entry = xref.get(&target)?;
    let offset = match entry {
        XrefEntry::Uncompressed { offset, .. } => *offset,
        // Compressed (ObjStm) length refs are exotic and have not shown up
        // in the wild for stream /Length specifically; skip for now.
        XrefEntry::Compressed { .. } | XrefEntry::Free => return None,
    };
    // Do not pass `xref` into the recursive parse — a /Length reference
    // should point at a plain integer, and forbidding further recursion
    // keeps a malformed cycle from spiralling.
    let object = parse_indirect_object(bytes, offset, None).ok()?;
    match object {
        PdfObject::Value(PdfValue::Integer(len)) if len >= 0 => Some(len as usize),
        _ => None,
    }
}

fn find_keyword(bytes: &[u8], start: usize, keyword: &[u8]) -> Option<usize> {
    bytes[start..]
        .windows(keyword.len())
        .position(|window| window == keyword)
        .map(|relative| start + relative)
}

struct Cursor<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8], position: usize) -> Self {
        Self { bytes, position }
    }

    fn eof(&self) -> bool {
        self.position >= self.bytes.len()
    }

    fn current(&self) -> Option<u8> {
        self.bytes.get(self.position).copied()
    }

    fn skip_ws_and_comments(&mut self) {
        while let Some(byte) = self.current() {
            match byte {
                b' ' | b'\t' | b'\n' | b'\r' | 0x0C | 0x00 => self.position += 1,
                b'%' => {
                    while let Some(next) = self.current() {
                        self.position += 1;
                        if next == b'\n' || next == b'\r' {
                            break;
                        }
                    }
                }
                _ => break,
            }
        }
    }

    fn skip_line_breaks(&mut self) {
        while matches!(self.current(), Some(b'\n' | b'\r')) {
            self.position += 1;
        }
    }

    fn read_line(&mut self) -> PdfResult<&'a [u8]> {
        if self.eof() {
            return Err(PdfError::Parse("unexpected end of file".to_string()));
        }
        let start = self.position;
        while let Some(byte) = self.current() {
            if byte == b'\n' || byte == b'\r' {
                let end = self.position;
                self.skip_line_breaks();
                return Ok(&self.bytes[start..end]);
            }
            self.position += 1;
        }
        Ok(&self.bytes[start..self.position])
    }

    fn peek_keyword(&self, keyword: &str) -> bool {
        self.bytes
            .get(self.position..self.position + keyword.len())
            .map(|slice| slice == keyword.as_bytes())
            .unwrap_or(false)
    }

    fn expect_keyword(&mut self, keyword: &str) -> PdfResult<()> {
        self.skip_ws_and_comments();
        if self.peek_keyword(keyword) {
            self.position += keyword.len();
            Ok(())
        } else {
            Err(PdfError::Parse(format!("expected keyword {keyword}")))
        }
    }

    fn consume_stream_line_break(&mut self) {
        if self.current() == Some(b'\r') {
            self.position += 1;
        }
        if self.current() == Some(b'\n') {
            self.position += 1;
        }
    }

    fn parse_u32(&mut self) -> PdfResult<u32> {
        let token = self.parse_token()?;
        token
            .parse::<u32>()
            .map_err(|_| PdfError::Parse(format!("invalid integer token: {token}")))
    }

    fn parse_u16(&mut self) -> PdfResult<u16> {
        let token = self.parse_token()?;
        token
            .parse::<u16>()
            .map_err(|_| PdfError::Parse(format!("invalid integer token: {token}")))
    }

    fn parse_usize(&mut self) -> PdfResult<usize> {
        let token = self.parse_token()?;
        token
            .parse::<usize>()
            .map_err(|_| PdfError::Parse(format!("invalid offset token: {token}")))
    }

    fn parse_token(&mut self) -> PdfResult<String> {
        self.skip_ws_and_comments();
        let start = self.position;
        while let Some(byte) = self.current() {
            if is_delimiter(byte) || is_whitespace(byte) {
                break;
            }
            self.position += 1;
        }
        if self.position == start {
            return Err(PdfError::Parse("expected token".to_string()));
        }
        Ok(String::from_utf8_lossy(&self.bytes[start..self.position]).to_string())
    }

    fn parse_value(&mut self) -> PdfResult<PdfValue> {
        self.skip_ws_and_comments();
        match self.current() {
            Some(b'/') => self.parse_name(),
            Some(b'(') => self.parse_literal_string(),
            Some(b'[') => self.parse_array(),
            Some(b'<') if self.bytes.get(self.position + 1) == Some(&b'<') => {
                self.parse_dictionary()
            }
            Some(b'<') => self.parse_hex_string(),
            Some(b't') if self.peek_keyword("true") => {
                self.position += 4;
                Ok(PdfValue::Bool(true))
            }
            Some(b'f') if self.peek_keyword("false") => {
                self.position += 5;
                Ok(PdfValue::Bool(false))
            }
            Some(b'n') if self.peek_keyword("null") => {
                self.position += 4;
                Ok(PdfValue::Null)
            }
            Some(_) => self.parse_number_or_reference(),
            None => Err(PdfError::Parse("unexpected end of file".to_string())),
        }
    }

    fn parse_name(&mut self) -> PdfResult<PdfValue> {
        self.position += 1;
        let mut raw = Vec::new();
        while let Some(byte) = self.current() {
            if is_delimiter(byte) || is_whitespace(byte) {
                break;
            }
            if byte == b'#' {
                let high =
                    self.bytes.get(self.position + 1).copied().ok_or_else(|| {
                        PdfError::Parse("truncated #XX escape in name".to_string())
                    })?;
                let low =
                    self.bytes.get(self.position + 2).copied().ok_or_else(|| {
                        PdfError::Parse("truncated #XX escape in name".to_string())
                    })?;
                let decoded = u8::from_str_radix(&format!("{}{}", high as char, low as char), 16)
                    .map_err(|_| {
                    PdfError::Parse("invalid #XX hex escape in name".to_string())
                })?;
                raw.push(decoded);
                self.position += 3;
            } else {
                raw.push(byte);
                self.position += 1;
            }
        }
        Ok(PdfValue::Name(String::from_utf8_lossy(&raw).to_string()))
    }

    fn parse_literal_string(&mut self) -> PdfResult<PdfValue> {
        self.position += 1;
        let mut output = Vec::new();
        let mut depth = 1usize;
        while let Some(byte) = self.current() {
            self.position += 1;
            match byte {
                b'\\' => {
                    let escaped = self
                        .current()
                        .ok_or_else(|| PdfError::Parse("unterminated string escape".to_string()))?;
                    self.position += 1;
                    match escaped {
                        b'n' => output.push(b'\n'),
                        b'r' => output.push(b'\r'),
                        b't' => output.push(b'\t'),
                        b'b' => output.push(0x08),
                        b'f' => output.push(0x0C),
                        b'(' | b')' | b'\\' => output.push(escaped),
                        b'\n' => {}
                        b'\r' => {
                            if self.current() == Some(b'\n') {
                                self.position += 1;
                            }
                        }
                        b'0'..=b'7' => {
                            let mut octal = vec![escaped];
                            for _ in 0..2 {
                                match self.current() {
                                    Some(next @ b'0'..=b'7') => {
                                        octal.push(next);
                                        self.position += 1;
                                    }
                                    _ => break,
                                }
                            }
                            // PDF spec: octal value is taken modulo 256
                            let value =
                                u16::from_str_radix(std::str::from_utf8(&octal).unwrap_or("0"), 8)
                                    .unwrap_or(0);
                            output.push((value % 256) as u8);
                        }
                        other => output.push(other),
                    }
                }
                b'(' => {
                    depth += 1;
                    output.push(byte);
                }
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(PdfValue::String(PdfString(output)));
                    }
                    output.push(byte);
                }
                _ => output.push(byte),
            }
        }
        Err(PdfError::Parse("unterminated literal string".to_string()))
    }

    fn parse_hex_string(&mut self) -> PdfResult<PdfValue> {
        self.position += 1;
        let start = self.position;
        while self.current() != Some(b'>') {
            if self.eof() {
                return Err(PdfError::Parse("unterminated hex string".to_string()));
            }
            self.position += 1;
        }
        let raw = String::from_utf8_lossy(&self.bytes[start..self.position])
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect::<String>();
        self.position += 1;
        let mut chars = raw.chars().collect::<Vec<_>>();
        if chars.len() % 2 != 0 {
            chars.push('0');
        }
        let mut bytes = Vec::with_capacity(chars.len() / 2);
        for pair in chars.chunks(2) {
            let value = u8::from_str_radix(&pair.iter().collect::<String>(), 16)
                .map_err(|_| PdfError::Parse("invalid hex string".to_string()))?;
            bytes.push(value);
        }
        Ok(PdfValue::String(PdfString(bytes)))
    }

    fn parse_array(&mut self) -> PdfResult<PdfValue> {
        self.position += 1;
        let mut values = Vec::new();
        loop {
            self.skip_ws_and_comments();
            match self.current() {
                Some(b']') => {
                    self.position += 1;
                    break;
                }
                Some(_) => values.push(self.parse_value()?),
                None => return Err(PdfError::Parse("unterminated array".to_string())),
            }
        }
        Ok(PdfValue::Array(values))
    }

    fn parse_dictionary(&mut self) -> PdfResult<PdfValue> {
        self.position += 2;
        let mut dictionary = PdfDictionary::new();
        loop {
            self.skip_ws_and_comments();
            if self.current() == Some(b'>') && self.bytes.get(self.position + 1) == Some(&b'>') {
                self.position += 2;
                break;
            }
            let key = match self.parse_name()? {
                PdfValue::Name(name) => name,
                _ => unreachable!(),
            };
            let value = self.parse_value()?;
            dictionary.insert(key, value);
        }
        Ok(PdfValue::Dictionary(dictionary))
    }

    fn parse_number_or_reference(&mut self) -> PdfResult<PdfValue> {
        let first_token = self.parse_token()?;
        if first_token.contains('.') || first_token.contains(['e', 'E']) {
            return first_token
                .parse::<f64>()
                .map(PdfValue::Number)
                .map_err(|_| PdfError::Parse(format!("invalid number token: {first_token}")));
        }

        let checkpoint = self.position;
        self.skip_ws_and_comments();
        if let Ok(second_token) = self.parse_token() {
            self.skip_ws_and_comments();
            if self.current() == Some(b'R')
                && second_token
                    .chars()
                    .all(|character| character.is_ascii_digit())
            {
                self.position += 1;
                return Ok(PdfValue::Reference(ObjectRef::new(
                    first_token
                        .parse::<u32>()
                        .map_err(|_| PdfError::Parse("invalid reference object".to_string()))?,
                    second_token
                        .parse::<u16>()
                        .map_err(|_| PdfError::Parse("invalid reference generation".to_string()))?,
                )));
            }
        }
        self.position = checkpoint;
        first_token
            .parse::<i64>()
            .map(PdfValue::Integer)
            .or_else(|_| first_token.parse::<f64>().map(PdfValue::Number))
            .map_err(|_| PdfError::Parse(format!("invalid number token: {first_token}")))
    }
}

fn is_whitespace(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0C | 0x00)
}

fn is_delimiter(byte: u8) -> bool {
    matches!(
        byte,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}

#[cfg(test)]
mod tests {
    use super::{parse_pdf, parse_pdf_with_certificate, parse_pdf_with_password};
    use crate::error::PdfError;
    use crate::types::{PdfObject, PdfValue};

    #[test]
    fn parses_simple_pdf_fixture() {
        let bytes = include_bytes!("../../../tests/fixtures/simple-text.pdf");
        let document = parse_pdf(bytes).expect("fixture should parse");
        assert_eq!(document.pages.len(), 1);
    }

    #[test]
    fn parses_incremental_update_fixture() {
        let bytes = include_bytes!("../../../tests/fixtures/incremental-update.pdf");
        let document = parse_pdf(bytes).expect("incremental fixture should parse");
        assert_eq!(document.pages.len(), 1);

        // The updated content stream (object 4) should contain "Updated Secret",
        // not "Original Secret"
        let content_refs = &document.pages[0].content_refs;
        assert!(!content_refs.is_empty());
        let content_obj = document.file.objects.get(&content_refs[0]).unwrap();
        let stream_data = match content_obj {
            PdfObject::Stream(stream) => String::from_utf8_lossy(&stream.data),
            _ => panic!("expected stream object for page content"),
        };
        assert!(
            stream_data.contains("Updated Secret"),
            "content stream should contain updated text"
        );
        assert!(
            !stream_data.contains("Original Secret"),
            "content stream should not contain original text"
        );
    }

    #[test]
    fn circular_prev_chain_does_not_loop() {
        // Build a minimal PDF where Prev points back to the same xref offset.
        // The parser should de-duplicate the offset via its visited-set and
        // parse the tree successfully instead of returning an error.
        let mut pdf = Vec::new();
        pdf.extend_from_slice(b"%PDF-1.4\n");

        // Object 1: catalog
        let obj1_offset = pdf.len();
        pdf.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");

        // Object 2: pages
        let obj2_offset = pdf.len();
        pdf.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Count 0 /Kids [] >>\nendobj\n");

        let xref_offset = pdf.len();
        pdf.extend_from_slice(b"xref\n0 3\n");
        pdf.extend_from_slice(b"0000000000 65535 f \n");
        pdf.extend_from_slice(format!("{:010} 00000 n \n", obj1_offset).as_bytes());
        pdf.extend_from_slice(format!("{:010} 00000 n \n", obj2_offset).as_bytes());
        pdf.extend_from_slice(b"trailer\n");
        // Prev points back to this same xref offset — circular
        pdf.extend_from_slice(
            format!("<< /Size 3 /Root 1 0 R /Prev {} >>\n", xref_offset).as_bytes(),
        );
        pdf.extend_from_slice(format!("startxref\n{}\n%%EOF\n", xref_offset).as_bytes());

        let document = parse_pdf(&pdf).expect("circular Prev should be tolerated");
        assert_eq!(document.pages.len(), 0);
    }

    #[test]
    fn stream_length_indirect_reference_is_resolved() {
        // Minimal PDF whose page content stream has `/Length 5 0 R`, where
        // object 5 is a plain integer. The stream's payload includes the
        // literal bytes "endstream" so the fallback endstream scan would
        // underflow; resolving the indirect /Length reads the exact bytes.
        let payload = b"--endstream--HIDDEN";
        let mut pdf = Vec::new();
        pdf.extend_from_slice(b"%PDF-1.4\n");

        let obj1_offset = pdf.len();
        pdf.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");

        let obj2_offset = pdf.len();
        pdf.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Count 1 /Kids [3 0 R] >>\nendobj\n");

        let obj3_offset = pdf.len();
        pdf.extend_from_slice(
            b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << >> /Contents 4 0 R >>\nendobj\n",
        );

        let obj4_offset = pdf.len();
        pdf.extend_from_slice(b"4 0 obj\n<< /Length 5 0 R >>\nstream\n");
        pdf.extend_from_slice(payload);
        pdf.extend_from_slice(b"\nendstream\nendobj\n");

        let obj5_offset = pdf.len();
        pdf.extend_from_slice(format!("5 0 obj\n{}\nendobj\n", payload.len()).as_bytes());

        let xref_offset = pdf.len();
        pdf.extend_from_slice(b"xref\n0 6\n");
        pdf.extend_from_slice(b"0000000000 65535 f \n");
        pdf.extend_from_slice(format!("{:010} 00000 n \n", obj1_offset).as_bytes());
        pdf.extend_from_slice(format!("{:010} 00000 n \n", obj2_offset).as_bytes());
        pdf.extend_from_slice(format!("{:010} 00000 n \n", obj3_offset).as_bytes());
        pdf.extend_from_slice(format!("{:010} 00000 n \n", obj4_offset).as_bytes());
        pdf.extend_from_slice(format!("{:010} 00000 n \n", obj5_offset).as_bytes());
        pdf.extend_from_slice(b"trailer\n<< /Size 6 /Root 1 0 R >>\n");
        pdf.extend_from_slice(format!("startxref\n{}\n%%EOF\n", xref_offset).as_bytes());

        let document = parse_pdf(&pdf).expect("indirect-length fixture should parse");
        let content_refs = &document.pages[0].content_refs;
        let content_obj = document.file.objects.get(&content_refs[0]).unwrap();
        let data = match content_obj {
            PdfObject::Stream(stream) => &stream.data,
            _ => panic!("expected stream object for page content"),
        };
        assert_eq!(
            data.as_slice(),
            payload,
            "resolved indirect /Length should yield the exact original payload bytes"
        );
    }

    #[test]
    fn parses_uncompressed_xref_stream() {
        // Minimal PDF using an xref stream with no filters and no predictor.
        // W = [1 2 1] means type(1) + offset(2) + generation(1).
        let mut pdf: Vec<u8> = Vec::new();
        pdf.extend_from_slice(b"%PDF-1.5\n");

        let obj1_offset = pdf.len();
        pdf.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
        let obj2_offset = pdf.len();
        pdf.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Count 0 /Kids [] >>\nendobj\n");

        // Build the xref stream body: four 4-byte rows for objects 0..3.
        // Row layout: type(1) | offset(2) | generation(1).
        let row_for = |t: u8, off: u16, generation: u8| {
            let mut row = [0u8; 4];
            row[0] = t;
            row[1] = (off >> 8) as u8;
            row[2] = off as u8;
            row[3] = generation;
            row
        };
        let mut body = Vec::new();
        body.extend_from_slice(&row_for(0, 0, 0xFF)); // object 0 free
        body.extend_from_slice(&row_for(1, obj1_offset as u16, 0));
        body.extend_from_slice(&row_for(1, obj2_offset as u16, 0));
        body.extend_from_slice(&row_for(1, 0, 0)); // self (object 3), placeholder; we will overwrite after knowing offset

        let xref_obj_offset = pdf.len();
        // Overwrite object 3 self-offset in body now that we know it.
        let self_offset = xref_obj_offset as u16;
        body[12] = 1;
        body[13] = (self_offset >> 8) as u8;
        body[14] = self_offset as u8;
        body[15] = 0;

        let stream_dict = format!(
            "<< /Type /XRef /Size 4 /W [1 2 1] /Root 1 0 R /Length {} >>",
            body.len()
        );
        pdf.extend_from_slice(format!("3 0 obj\n{stream_dict}\nstream\n").as_bytes());
        pdf.extend_from_slice(&body);
        pdf.extend_from_slice(b"\nendstream\nendobj\n");
        pdf.extend_from_slice(format!("startxref\n{}\n%%EOF\n", xref_obj_offset).as_bytes());

        let document = parse_pdf(&pdf).expect("xref stream fixture should parse");
        assert_eq!(document.pages.len(), 0);
        // Object 1 and 2 must be materialized.
        assert!(document.file.objects.len() >= 2);
    }

    #[test]
    fn parses_object_stream_via_xref_stream() {
        use flate2::{Compression, write::ZlibEncoder};
        use std::io::Write;

        // Pages tree is compressed inside an ObjStm.
        // Layout:
        //   1: Catalog (uncompressed)
        //   2: Pages (compressed in ObjStm 3, index 0)
        //   3: ObjStm (uncompressed, flate-compressed body)
        //   4: xref stream (uncompressed)
        let mut pdf: Vec<u8> = Vec::new();
        pdf.extend_from_slice(b"%PDF-1.5\n");

        let obj1_offset = pdf.len();
        pdf.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");

        // Object 3 is an ObjStm holding object 2.
        let member_payload = b"<< /Type /Pages /Count 0 /Kids [] >>";
        let header = b"2 0 ";
        let first = header.len();
        let mut decompressed = Vec::new();
        decompressed.extend_from_slice(header);
        decompressed.extend_from_slice(member_payload);

        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&decompressed).unwrap();
        let compressed = encoder.finish().unwrap();

        let obj3_offset = pdf.len();
        let objstm_dict = format!(
            "<< /Type /ObjStm /N 1 /First {} /Filter /FlateDecode /Length {} >>",
            first,
            compressed.len()
        );
        pdf.extend_from_slice(format!("3 0 obj\n{objstm_dict}\nstream\n").as_bytes());
        pdf.extend_from_slice(&compressed);
        pdf.extend_from_slice(b"\nendstream\nendobj\n");

        // Build xref stream entries for objects 0..5:
        // 0 free, 1 uncompressed, 2 compressed (stream=3, index=0),
        // 3 uncompressed (ObjStm), 4 uncompressed (xref stream itself).
        let row_for = |t: u8, a: u32, b: u16| {
            let mut row = [0u8; 5];
            row[0] = t;
            row[1] = (a >> 16) as u8;
            row[2] = (a >> 8) as u8;
            row[3] = a as u8;
            row[4] = b as u8;
            row
        };

        let obj4_offset = pdf.len();
        let mut body = Vec::new();
        body.extend_from_slice(&row_for(0, 0, 0xFF));
        body.extend_from_slice(&row_for(1, obj1_offset as u32, 0));
        body.extend_from_slice(&row_for(2, 3, 0));
        body.extend_from_slice(&row_for(1, obj3_offset as u32, 0));
        body.extend_from_slice(&row_for(1, obj4_offset as u32, 0));

        let stream_dict = format!(
            "<< /Type /XRef /Size 5 /W [1 3 1] /Root 1 0 R /Length {} >>",
            body.len()
        );
        pdf.extend_from_slice(format!("4 0 obj\n{stream_dict}\nstream\n").as_bytes());
        pdf.extend_from_slice(&body);
        pdf.extend_from_slice(b"\nendstream\nendobj\n");
        pdf.extend_from_slice(format!("startxref\n{}\n%%EOF\n", obj4_offset).as_bytes());

        let document = parse_pdf(&pdf).expect("ObjStm fixture should parse");
        assert_eq!(document.pages.len(), 0);
        // Pages dictionary should be materialized.
        let pages_ref = document.catalog.pages_ref;
        let pages_dict = document.file.get_dictionary(pages_ref).unwrap();
        assert_eq!(
            pages_dict.get("Type").and_then(|v| v.as_name()),
            Some("Pages")
        );
    }

    #[test]
    fn rejects_nested_object_stream() {
        use flate2::{Compression, write::ZlibEncoder};
        use std::io::Write;

        // A compressed member is itself an ObjStm dictionary → must fail.
        let mut pdf: Vec<u8> = Vec::new();
        pdf.extend_from_slice(b"%PDF-1.5\n");

        let obj1_offset = pdf.len();
        pdf.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");

        let member_payload = b"<< /Type /ObjStm /N 0 /First 0 /Length 0 >>";
        let header = b"2 0 ";
        let first = header.len();
        let mut decompressed = Vec::new();
        decompressed.extend_from_slice(header);
        decompressed.extend_from_slice(member_payload);

        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&decompressed).unwrap();
        let compressed = encoder.finish().unwrap();

        let obj3_offset = pdf.len();
        let objstm_dict = format!(
            "<< /Type /ObjStm /N 1 /First {} /Filter /FlateDecode /Length {} >>",
            first,
            compressed.len()
        );
        pdf.extend_from_slice(format!("3 0 obj\n{objstm_dict}\nstream\n").as_bytes());
        pdf.extend_from_slice(&compressed);
        pdf.extend_from_slice(b"\nendstream\nendobj\n");

        let row_for = |t: u8, a: u32, b: u16| {
            let mut row = [0u8; 5];
            row[0] = t;
            row[1] = (a >> 16) as u8;
            row[2] = (a >> 8) as u8;
            row[3] = a as u8;
            row[4] = b as u8;
            row
        };

        let obj4_offset = pdf.len();
        let mut body = Vec::new();
        body.extend_from_slice(&row_for(0, 0, 0xFF));
        body.extend_from_slice(&row_for(1, obj1_offset as u32, 0));
        body.extend_from_slice(&row_for(2, 3, 0));
        body.extend_from_slice(&row_for(1, obj3_offset as u32, 0));
        body.extend_from_slice(&row_for(1, obj4_offset as u32, 0));

        let stream_dict = format!(
            "<< /Type /XRef /Size 5 /W [1 3 1] /Root 1 0 R /Length {} >>",
            body.len()
        );
        pdf.extend_from_slice(format!("4 0 obj\n{stream_dict}\nstream\n").as_bytes());
        pdf.extend_from_slice(&body);
        pdf.extend_from_slice(b"\nendstream\nendobj\n");
        pdf.extend_from_slice(format!("startxref\n{}\n%%EOF\n", obj4_offset).as_bytes());

        match parse_pdf(&pdf) {
            Err(PdfError::Unsupported(message)) => {
                assert!(message.contains("nested object streams"), "got: {message}")
            }
            other => panic!("expected Unsupported, got: {other:?}"),
        }
    }

    /// Build a minimal V=2/R=3 RC4-encrypted PDF with the supplied user /
    /// owner passwords; encrypt a single content stream whose plaintext is
    /// returned alongside the bytes. Reused by all the RC4-encryption
    /// regression tests so the only per-test variable is which password
    /// the caller supplies to `parse_pdf_with_password`.
    fn build_rc4_encrypted_pdf(
        user_password: &[u8],
        owner_password: &[u8],
    ) -> (Vec<u8>, &'static [u8]) {
        use crate::crypto::SecurityRevision;
        use crate::crypto::test_helpers::{
            compute_file_key, compute_o, compute_u_r3, object_key, rc4,
        };

        let id_first: [u8; 16] = [
            0x6e, 0x05, 0xb1, 0x20, 0x63, 0x94, 0x69, 0x1f, 0x22, 0x2c, 0x32, 0xac, 0x61, 0x8b,
            0xe6, 0x8d,
        ];
        let permissions: i32 = -4;
        let key_length_bytes = 16;

        let owner_entry = compute_o(
            owner_password,
            user_password,
            SecurityRevision::R3,
            key_length_bytes,
        );
        let file_key = compute_file_key(
            user_password,
            &owner_entry,
            permissions,
            &id_first,
            key_length_bytes,
        );
        let u_entry = compute_u_r3(&file_key, &id_first);

        let escape_literal = |bytes: &[u8]| -> Vec<u8> {
            let mut out = Vec::with_capacity(bytes.len() + 2);
            out.push(b'(');
            for &byte in bytes {
                match byte {
                    b'(' | b')' | b'\\' => {
                        out.push(b'\\');
                        out.push(byte);
                    }
                    _ => out.push(byte),
                }
            }
            out.push(b')');
            out
        };

        let content_plain: &'static [u8] = b"BT\n/F1 24 Tf\n72 700 Td\n(CIPHERED SECRET) Tj\nET\n";
        let content_cipher = rc4(&object_key(&file_key, 4, 0), content_plain);

        let mut pdf: Vec<u8> = Vec::new();
        pdf.extend_from_slice(b"%PDF-1.4\n");

        let catalog_offset = pdf.len();
        pdf.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");

        let pages_offset = pdf.len();
        pdf.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Count 1 /Kids [3 0 R] >>\nendobj\n");

        let page_offset = pdf.len();
        pdf.extend_from_slice(
            b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
              /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>\nendobj\n",
        );

        let content_offset = pdf.len();
        pdf.extend_from_slice(
            format!("4 0 obj\n<< /Length {} >>\nstream\n", content_cipher.len()).as_bytes(),
        );
        pdf.extend_from_slice(&content_cipher);
        pdf.extend_from_slice(b"\nendstream\nendobj\n");

        let font_offset = pdf.len();
        pdf.extend_from_slice(
            b"5 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica \
              /Encoding /WinAnsiEncoding >>\nendobj\n",
        );

        let encrypt_offset = pdf.len();
        pdf.extend_from_slice(b"6 0 obj\n<< /Filter /Standard /V 2 /R 3 /Length 128 ");
        pdf.extend_from_slice(format!("/P {permissions} ").as_bytes());
        pdf.extend_from_slice(b"/O ");
        pdf.extend_from_slice(&escape_literal(&owner_entry));
        pdf.extend_from_slice(b" /U ");
        pdf.extend_from_slice(&escape_literal(&u_entry));
        pdf.extend_from_slice(b" >>\nendobj\n");

        let xref_offset = pdf.len();
        pdf.extend_from_slice(b"xref\n0 7\n");
        pdf.extend_from_slice(b"0000000000 65535 f \n");
        for offset in [
            catalog_offset,
            pages_offset,
            page_offset,
            content_offset,
            font_offset,
            encrypt_offset,
        ] {
            pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        pdf.extend_from_slice(b"trailer\n<< /Size 7 /Root 1 0 R /Encrypt 6 0 R /ID [");
        pdf.extend_from_slice(&escape_literal(&id_first));
        pdf.extend_from_slice(&escape_literal(&id_first));
        pdf.extend_from_slice(b"] >>\n");
        pdf.extend_from_slice(format!("startxref\n{xref_offset}\n%%EOF\n").as_bytes());

        (pdf, content_plain)
    }

    fn assert_decrypts_content_stream(document: &crate::document::ParsedDocument, expected: &[u8]) {
        assert_eq!(document.pages.len(), 1);
        assert!(
            !document.file.trailer.contains_key("Encrypt"),
            "trailer /Encrypt must be stripped once the document is decrypted in place"
        );
        let content_ref = document.pages[0].content_refs[0];
        let stream = match document.file.get_object(content_ref).unwrap() {
            PdfObject::Stream(stream) => stream,
            _ => panic!("page content must be a stream"),
        };
        assert_eq!(stream.data, expected);
    }

    #[test]
    fn parses_rc4_encrypted_pdf_with_empty_password() {
        // Real-world "encrypted to prevent editing but openable by anyone"
        // PDFs ship with an empty user password. The regression target
        // here is that parse_pdf (the no-argument entry point) still opens
        // them without a caller-supplied password.
        let (pdf, plain) = build_rc4_encrypted_pdf(b"", b"arbitrary-owner-password");
        let document = parse_pdf(&pdf).expect("empty-password PDF should decrypt");
        assert_decrypts_content_stream(&document, plain);
    }

    #[test]
    fn parses_rc4_encrypted_pdf_with_user_password() {
        let (pdf, plain) = build_rc4_encrypted_pdf(b"userpw", b"ownerpw");
        let document =
            parse_pdf_with_password(&pdf, b"userpw").expect("correct user password should decrypt");
        assert_decrypts_content_stream(&document, plain);
    }

    #[test]
    fn parses_rc4_encrypted_pdf_with_owner_password() {
        let (pdf, plain) = build_rc4_encrypted_pdf(b"userpw", b"ownerpw");
        let document = parse_pdf_with_password(&pdf, b"ownerpw")
            .expect("correct owner password should decrypt");
        assert_decrypts_content_stream(&document, plain);
    }

    #[test]
    fn rejects_wrong_password_with_invalid_password_error() {
        let (pdf, _) = build_rc4_encrypted_pdf(b"userpw", b"ownerpw");
        let err =
            parse_pdf_with_password(&pdf, b"wrongpw").expect_err("wrong password must not decrypt");
        assert_eq!(err, PdfError::InvalidPassword);
    }

    #[test]
    fn parses_rc4_encrypted_pdf_with_utf8_password() {
        let password = "pässwörd".as_bytes();
        let (pdf, plain) = build_rc4_encrypted_pdf(password, b"ownerpw");
        let document =
            parse_pdf_with_password(&pdf, password).expect("UTF-8 user password should decrypt");
        assert_decrypts_content_stream(&document, plain);
    }

    /// Build a minimal V=4/R=4 AES-128 encrypted PDF with the supplied
    /// user / owner passwords and `/EncryptMetadata` flag. Reused by all
    /// the AES encryption regression tests so the only per-test variable
    /// is which password the caller supplies.
    fn build_aes_128_encrypted_pdf(
        user_password: &[u8],
        owner_password: &[u8],
        encrypt_metadata: bool,
    ) -> (Vec<u8>, &'static [u8]) {
        use crate::crypto::SecurityRevision;
        use crate::crypto::test_helpers::{
            aes_128_cbc_encrypt, compute_file_key_r4, compute_o, compute_u_r3, object_key_aes,
        };

        let id_first: [u8; 16] = [
            0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88,
            0x99, 0x00,
        ];
        let permissions: i32 = -4;

        let owner_entry = compute_o(owner_password, user_password, SecurityRevision::R4, 16);
        let file_key = compute_file_key_r4(
            user_password,
            &owner_entry,
            permissions,
            &id_first,
            encrypt_metadata,
        );
        let u_entry = compute_u_r3(&file_key, &id_first);

        // The IV for each encrypted string / stream is arbitrary. Use
        // object-number-derived patterns so the two fixtures we produce
        // here do not collide on a block.
        let content_iv = [0x42u8; 16];
        let content_plain: &'static [u8] =
            b"BT\n/F1 24 Tf\n72 700 Td\n(AES SECRET REMOVED) Tj\nET\n";
        let content_key = object_key_aes(&file_key, 4, 0);
        let content_cipher = aes_128_cbc_encrypt(&content_key, &content_iv, content_plain);

        let escape_literal = |bytes: &[u8]| -> Vec<u8> {
            let mut out = Vec::with_capacity(bytes.len() + 2);
            out.push(b'(');
            for &byte in bytes {
                match byte {
                    b'(' | b')' | b'\\' => {
                        out.push(b'\\');
                        out.push(byte);
                    }
                    _ => out.push(byte),
                }
            }
            out.push(b')');
            out
        };

        let mut pdf: Vec<u8> = Vec::new();
        pdf.extend_from_slice(b"%PDF-1.5\n");

        let catalog_offset = pdf.len();
        pdf.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");

        let pages_offset = pdf.len();
        pdf.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Count 1 /Kids [3 0 R] >>\nendobj\n");

        let page_offset = pdf.len();
        pdf.extend_from_slice(
            b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
              /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>\nendobj\n",
        );

        let content_offset = pdf.len();
        pdf.extend_from_slice(
            format!("4 0 obj\n<< /Length {} >>\nstream\n", content_cipher.len()).as_bytes(),
        );
        pdf.extend_from_slice(&content_cipher);
        pdf.extend_from_slice(b"\nendstream\nendobj\n");

        let font_offset = pdf.len();
        pdf.extend_from_slice(
            b"5 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica \
              /Encoding /WinAnsiEncoding >>\nendobj\n",
        );

        let encrypt_offset = pdf.len();
        pdf.extend_from_slice(
            b"6 0 obj\n<< /Filter /Standard /V 4 /R 4 /Length 128 \
              /CF << /StdCF << /CFM /AESV2 /Length 16 /AuthEvent /DocOpen >> >> \
              /StmF /StdCF /StrF /StdCF ",
        );
        pdf.extend_from_slice(format!("/P {permissions} ").as_bytes());
        if !encrypt_metadata {
            pdf.extend_from_slice(b"/EncryptMetadata false ");
        }
        pdf.extend_from_slice(b"/O ");
        pdf.extend_from_slice(&escape_literal(&owner_entry));
        pdf.extend_from_slice(b" /U ");
        pdf.extend_from_slice(&escape_literal(&u_entry));
        pdf.extend_from_slice(b" >>\nendobj\n");

        let xref_offset = pdf.len();
        pdf.extend_from_slice(b"xref\n0 7\n");
        pdf.extend_from_slice(b"0000000000 65535 f \n");
        for offset in [
            catalog_offset,
            pages_offset,
            page_offset,
            content_offset,
            font_offset,
            encrypt_offset,
        ] {
            pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        pdf.extend_from_slice(b"trailer\n<< /Size 7 /Root 1 0 R /Encrypt 6 0 R /ID [");
        pdf.extend_from_slice(&escape_literal(&id_first));
        pdf.extend_from_slice(&escape_literal(&id_first));
        pdf.extend_from_slice(b"] >>\n");
        pdf.extend_from_slice(format!("startxref\n{xref_offset}\n%%EOF\n").as_bytes());

        (pdf, content_plain)
    }

    #[test]
    fn parses_aes_128_encrypted_pdf_with_empty_password() {
        let (pdf, plain) = build_aes_128_encrypted_pdf(b"", b"arbitrary-owner-password", true);
        let document = parse_pdf(&pdf).expect("empty-password AES-128 PDF should decrypt");
        assert_decrypts_content_stream(&document, plain);
    }

    #[test]
    fn parses_aes_128_encrypted_pdf_with_user_password() {
        let (pdf, plain) = build_aes_128_encrypted_pdf(b"userpw", b"ownerpw", true);
        let document = parse_pdf_with_password(&pdf, b"userpw")
            .expect("correct user password should decrypt AES-128 PDF");
        assert_decrypts_content_stream(&document, plain);
    }

    #[test]
    fn parses_aes_128_encrypted_pdf_with_owner_password() {
        let (pdf, plain) = build_aes_128_encrypted_pdf(b"userpw", b"ownerpw", true);
        let document = parse_pdf_with_password(&pdf, b"ownerpw")
            .expect("correct owner password should decrypt AES-128 PDF");
        assert_decrypts_content_stream(&document, plain);
    }

    #[test]
    fn aes_128_rejects_wrong_password() {
        let (pdf, _) = build_aes_128_encrypted_pdf(b"userpw", b"ownerpw", true);
        let err = parse_pdf_with_password(&pdf, b"wrongpw")
            .expect_err("wrong password must not decrypt AES-128 PDF");
        assert_eq!(err, PdfError::InvalidPassword);
    }

    /// Build a minimal V=5/R=6 AES-256 encrypted PDF. Reused by all the
    /// AES-256 regression tests so the only per-test variable is which
    /// password the caller supplies.
    fn build_aes_256_encrypted_pdf(
        user_password: &[u8],
        owner_password: &[u8],
        revision: crate::crypto::SecurityRevision,
    ) -> (Vec<u8>, &'static [u8]) {
        use crate::crypto::test_helpers::{
            aes_256_cbc_encrypt, compute_v5_o_and_oe, compute_v5_u_and_ue,
        };

        let permissions: i32 = -4;
        let file_key = [0x13u8; 32];
        let u_validation_salt = [0xAAu8; 8];
        let u_key_salt = [0xBBu8; 8];
        let o_validation_salt = [0xCCu8; 8];
        let o_key_salt = [0xDDu8; 8];

        let (u_entry, ue_entry) = compute_v5_u_and_ue(
            user_password,
            &u_validation_salt,
            &u_key_salt,
            &file_key,
            revision,
        );
        let u_vector: [u8; 48] = u_entry.as_slice().try_into().expect("U is 48 bytes");
        let (o_entry, oe_entry) = compute_v5_o_and_oe(
            owner_password,
            &o_validation_salt,
            &o_key_salt,
            &u_vector,
            &file_key,
            revision,
        );

        let content_iv = [0x42u8; 16];
        let content_plain: &'static [u8] = b"BT\n/F1 24 Tf\n72 700 Td\n(AES-256 SECRET) Tj\nET\n";
        let content_cipher = aes_256_cbc_encrypt(&file_key, &content_iv, content_plain);

        let escape_literal = |bytes: &[u8]| -> Vec<u8> {
            let mut out = Vec::with_capacity(bytes.len() + 2);
            out.push(b'(');
            for &byte in bytes {
                match byte {
                    b'(' | b')' | b'\\' => {
                        out.push(b'\\');
                        out.push(byte);
                    }
                    _ => out.push(byte),
                }
            }
            out.push(b')');
            out
        };

        let mut pdf: Vec<u8> = Vec::new();
        pdf.extend_from_slice(b"%PDF-2.0\n");

        let catalog_offset = pdf.len();
        pdf.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");

        let pages_offset = pdf.len();
        pdf.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Count 1 /Kids [3 0 R] >>\nendobj\n");

        let page_offset = pdf.len();
        pdf.extend_from_slice(
            b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
              /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>\nendobj\n",
        );

        let content_offset = pdf.len();
        pdf.extend_from_slice(
            format!("4 0 obj\n<< /Length {} >>\nstream\n", content_cipher.len()).as_bytes(),
        );
        pdf.extend_from_slice(&content_cipher);
        pdf.extend_from_slice(b"\nendstream\nendobj\n");

        let font_offset = pdf.len();
        pdf.extend_from_slice(
            b"5 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica \
              /Encoding /WinAnsiEncoding >>\nendobj\n",
        );

        let r_value = match revision {
            crate::crypto::SecurityRevision::R5 => 5,
            crate::crypto::SecurityRevision::R6 => 6,
            _ => panic!("V=5 fixture requires R=5 or R=6"),
        };

        let encrypt_offset = pdf.len();
        pdf.extend_from_slice(
            format!(
                "6 0 obj\n<< /Filter /Standard /V 5 /R {r_value} /Length 256 \
                  /CF << /StdCF << /CFM /AESV3 /Length 32 /AuthEvent /DocOpen >> >> \
                  /StmF /StdCF /StrF /StdCF /P {permissions} "
            )
            .as_bytes(),
        );
        pdf.extend_from_slice(b"/O ");
        pdf.extend_from_slice(&escape_literal(&o_entry));
        pdf.extend_from_slice(b" /U ");
        pdf.extend_from_slice(&escape_literal(&u_entry));
        pdf.extend_from_slice(b" /OE ");
        pdf.extend_from_slice(&escape_literal(&oe_entry));
        pdf.extend_from_slice(b" /UE ");
        pdf.extend_from_slice(&escape_literal(&ue_entry));
        pdf.extend_from_slice(b" >>\nendobj\n");

        let xref_offset = pdf.len();
        pdf.extend_from_slice(b"xref\n0 7\n");
        pdf.extend_from_slice(b"0000000000 65535 f \n");
        for offset in [
            catalog_offset,
            pages_offset,
            page_offset,
            content_offset,
            font_offset,
            encrypt_offset,
        ] {
            pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        // V=5 still requires /ID in the trailer even though it is not
        // consumed by the key-derivation algorithm.
        let id_literal: [u8; 16] = [
            0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xAA, 0xBB, 0xCC, 0xDD, 0xEE,
            0xFF, 0x00,
        ];
        pdf.extend_from_slice(b"trailer\n<< /Size 7 /Root 1 0 R /Encrypt 6 0 R /ID [");
        pdf.extend_from_slice(&escape_literal(&id_literal));
        pdf.extend_from_slice(&escape_literal(&id_literal));
        pdf.extend_from_slice(b"] >>\n");
        pdf.extend_from_slice(format!("startxref\n{xref_offset}\n%%EOF\n").as_bytes());

        (pdf, content_plain)
    }

    #[test]
    fn parses_aes_256_r6_encrypted_pdf_with_user_password() {
        let (pdf, plain) =
            build_aes_256_encrypted_pdf(b"userpw", b"ownerpw", crate::crypto::SecurityRevision::R6);
        let document = parse_pdf_with_password(&pdf, b"userpw")
            .expect("correct user password should decrypt AES-256 R=6 PDF");
        assert_decrypts_content_stream(&document, plain);
    }

    #[test]
    fn parses_aes_256_r6_encrypted_pdf_with_owner_password() {
        let (pdf, plain) =
            build_aes_256_encrypted_pdf(b"userpw", b"ownerpw", crate::crypto::SecurityRevision::R6);
        let document = parse_pdf_with_password(&pdf, b"ownerpw")
            .expect("correct owner password should decrypt AES-256 R=6 PDF");
        assert_decrypts_content_stream(&document, plain);
    }

    #[test]
    fn parses_aes_256_r5_encrypted_pdf_with_empty_password() {
        let (pdf, plain) =
            build_aes_256_encrypted_pdf(b"", b"ownerpw", crate::crypto::SecurityRevision::R5);
        let document = parse_pdf(&pdf).expect("empty-password AES-256 R=5 PDF should decrypt");
        assert_decrypts_content_stream(&document, plain);
    }

    #[test]
    fn aes_256_rejects_wrong_password() {
        let (pdf, _) =
            build_aes_256_encrypted_pdf(b"userpw", b"ownerpw", crate::crypto::SecurityRevision::R6);
        let err = parse_pdf_with_password(&pdf, b"wrongpw")
            .expect_err("wrong password must not decrypt AES-256 PDF");
        assert_eq!(err, PdfError::InvalidPassword);
    }

    #[test]
    fn parses_aes_128_with_encrypt_metadata_false() {
        // EncryptMetadata=false changes the file-key derivation (Algorithm 2
        // step 5 appends 0xFFFFFFFF), so the whole decryption path fails if
        // we do not honour the flag.
        let (pdf, plain) = build_aes_128_encrypted_pdf(b"", b"ownerpw", false);
        let document =
            parse_pdf(&pdf).expect("empty-password AES-128 PDF should decrypt with metadata off");
        assert_decrypts_content_stream(&document, plain);
    }

    #[test]
    fn decryption_drops_original_encrypt_dictionary_object() {
        // After successful decryption the parser strips the trailer's
        // /Encrypt reference. The Encrypt dictionary object itself must
        // also be removed from `objects` so the writer never re-emits its
        // /O, /U, /OE, /UE, /Perms fields as dangling unreferenced bytes.
        let (pdf, _) = build_aes_128_encrypted_pdf(b"", b"ownerpw", true);
        let document = parse_pdf(&pdf).expect("encrypted PDF should decrypt");
        for (object_ref, object) in &document.file.objects {
            if let PdfObject::Value(PdfValue::Dictionary(dict)) = object {
                let has_o = dict.contains_key("O");
                let has_u = dict.contains_key("U");
                let has_filter_standard =
                    dict.get("Filter").and_then(PdfValue::as_name) == Some("Standard");
                assert!(
                    !(has_o && has_u && has_filter_standard),
                    "Encrypt dictionary at {} {} survived parse",
                    object_ref.object_number,
                    object_ref.generation
                );
            }
        }
    }

    #[test]
    fn materialize_drops_objstm_containers() {
        // After ObjStm members are materialised into top-level objects the
        // container itself must be dropped from `objects`. Otherwise the
        // writer would re-emit the container's compressed bytes, leaking
        // the pre-redaction state of every member dictionary.
        let bytes = include_bytes!("../../../tests/fixtures/xref-object-stream.pdf");
        let document = parse_pdf(bytes).expect("xref+ObjStm fixture should parse");
        for (object_ref, object) in &document.file.objects {
            if let PdfObject::Stream(stream) = object {
                let type_name = stream.dict.get("Type").and_then(PdfValue::as_name);
                assert_ne!(
                    type_name,
                    Some("ObjStm"),
                    "ObjStm container at {} {} survived parse",
                    object_ref.object_number,
                    object_ref.generation
                );
            }
        }
    }

    /// Output of [`build_pubsec_encrypted_pdf`]: the encrypted PDF, the
    /// recipient's DER-encoded certificate, the recipient's DER-encoded
    /// PKCS#8 private key, and the plaintext content stream the test
    /// asserts the parser recovers.
    struct PubSecFixture {
        pdf: Vec<u8>,
        cert_der: Vec<u8>,
        private_key_der: Vec<u8>,
        plaintext: Vec<u8>,
    }

    /// Build a minimal Adobe.PubSec encrypted PDF for the requested
    /// SubFilter (`adbe.pkcs7.s4` → V=4 / AES-128, or
    /// `adbe.pkcs7.s5` → V=5 / AES-256). Generates a deterministic
    /// RSA-2048 keypair and self-signed cert from a fixed PRNG seed so
    /// the fixture bytes are reproducible across test runs without
    /// committing any private key material.
    fn build_pubsec_encrypted_pdf(sub_filter: &str) -> PubSecFixture {
        use cms::builder::{
            ContentEncryptionAlgorithm, EnvelopedDataBuilder, KeyEncryptionInfo,
            KeyTransRecipientInfoBuilder,
        };
        use cms::cert::IssuerAndSerialNumber;
        use cms::content_info::ContentInfo;
        use cms::enveloped_data::RecipientIdentifier;
        use const_oid::ObjectIdentifier;
        use der::asn1::{Any, PrintableString, SetOfVec};
        use der::{Decode, Encode};
        use rand_chacha::ChaCha8Rng;
        use rand_core::SeedableRng;
        use rsa::pkcs1v15::SigningKey;
        use rsa::pkcs8::{EncodePrivateKey, EncodePublicKey};
        use rsa::{RsaPrivateKey, RsaPublicKey};
        use sha2::Sha256;
        use spki::SubjectPublicKeyInfoOwned;
        use std::time::Duration;
        use x509_cert::Certificate;
        use x509_cert::attr::AttributeTypeAndValue;
        use x509_cert::builder::{Builder, CertificateBuilder, Profile};
        use x509_cert::name::{Name, RdnSequence, RelativeDistinguishedName};
        use x509_cert::serial_number::SerialNumber;
        use x509_cert::time::Validity;

        let mut rng = ChaCha8Rng::from_seed([0x42u8; 32]);
        let private_key = RsaPrivateKey::new(&mut rng, 2048).expect("RSA-2048 keygen must succeed");
        let public_key = RsaPublicKey::from(&private_key);
        let private_key_der = private_key
            .to_pkcs8_der()
            .expect("PKCS#8 encode")
            .as_bytes()
            .to_vec();

        // Build a minimal self-signed X.509 certificate.
        let serial_number = SerialNumber::from(0x01020304u32);
        let validity = Validity::from_now(Duration::from_secs(3600 * 24 * 30))
            .expect("validity computation must succeed");
        let cn = AttributeTypeAndValue {
            oid: const_oid::db::rfc4519::CN,
            value: Any::from(
                &PrintableString::new(b"open-redact-pdf-test-recipient").expect("printable string"),
            ),
        };
        let rdn_set = SetOfVec::try_from(vec![cn]).expect("rdn set");
        let mut subject = RdnSequence::default();
        subject.0.push(RelativeDistinguishedName::from(rdn_set));
        let subject_name =
            Name::from_der(&subject.to_der().expect("subject encode")).expect("subject re-decode");

        let signer: SigningKey<Sha256> = SigningKey::new(private_key.clone());
        let pub_key_der = public_key.to_public_key_der().expect("RSA public key DER");
        let pub_key_info =
            SubjectPublicKeyInfoOwned::try_from(pub_key_der.as_bytes()).expect("SPKI from DER");
        let cert_builder = CertificateBuilder::new(
            Profile::Root,
            serial_number.clone(),
            validity,
            subject_name.clone(),
            pub_key_info.clone(),
            &signer,
        )
        .expect("CertificateBuilder::new");
        let certificate: Certificate = cert_builder.build().expect("cert build");
        let cert_der = certificate.to_der().expect("cert DER");

        // Random 20-byte seed + 4-byte permissions (all 0xFF = full access).
        let mut seed_and_perms = [0u8; 24];
        rsa::rand_core::RngCore::fill_bytes(&mut rng, &mut seed_and_perms);
        seed_and_perms[20..24].copy_from_slice(&[0xFFu8, 0xFF, 0xFF, 0xFF]);

        // CMS EnvelopedData wrapping (seed || perms) for the recipient.
        let recipient_identifier =
            RecipientIdentifier::IssuerAndSerialNumber(IssuerAndSerialNumber {
                issuer: certificate.tbs_certificate.issuer.clone(),
                serial_number: certificate.tbs_certificate.serial_number.clone(),
            });
        let recipient_info_builder = KeyTransRecipientInfoBuilder::new(
            recipient_identifier,
            KeyEncryptionInfo::Rsa(public_key.clone()),
            &mut rng,
        )
        .expect("KeyTransRecipientInfoBuilder::new");

        let mut enveloped_builder = EnvelopedDataBuilder::new(
            None,
            &seed_and_perms,
            ContentEncryptionAlgorithm::Aes128Cbc,
            None,
        )
        .expect("EnvelopedDataBuilder::new");
        // Separate RNG instance for the EnvelopedData build step: the
        // KeyTransRecipientInfoBuilder still holds an exclusive borrow on
        // the primary rng until it is consumed inside the final
        // build_with_rng call below.
        let mut envelope_rng = ChaCha8Rng::from_seed([0xA5u8; 32]);
        let enveloped_data = enveloped_builder
            .add_recipient_info(recipient_info_builder)
            .expect("add_recipient_info")
            .build_with_rng(&mut envelope_rng)
            .expect("build_with_rng");

        // Wrap in ContentInfo (the outer ASN.1 structure).
        const ID_ENVELOPED: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.7.3");
        let enveloped_der = enveloped_data.to_der().expect("envelope DER");
        let content_info = ContentInfo {
            content_type: ID_ENVELOPED,
            content: Any::from_der(&enveloped_der).expect("Any from envelope DER"),
        };
        let recipient_blob = content_info.to_der().expect("content_info DER");

        // Derive file key per spec.
        let plaintext_content: Vec<u8> =
            b"BT\n/F1 24 Tf\n72 700 Td\n(PUBSEC SECRET) Tj\nET\n".to_vec();
        let (file_key, content_cipher, sub_filter_str, v_value, r_value, length_bits, cfm_name) =
            match sub_filter {
                "adbe.pkcs7.s5" => {
                    use crate::crypto::test_helpers::aes_256_cbc_encrypt;
                    use sha2::Digest as _;
                    let mut hasher = sha2::Sha256::new();
                    hasher.update(&seed_and_perms[..20]);
                    hasher.update(&recipient_blob);
                    hasher.update(&seed_and_perms[20..24]);
                    let file_key: [u8; 32] = hasher.finalize().into();
                    let iv = [0x55u8; 16];
                    let cipher = aes_256_cbc_encrypt(&file_key, &iv, &plaintext_content);
                    (
                        file_key.to_vec(),
                        cipher,
                        "adbe.pkcs7.s5",
                        5i32,
                        5i32,
                        256i32,
                        "AESV3",
                    )
                }
                "adbe.pkcs7.s4" => {
                    use crate::crypto::test_helpers::{aes_128_cbc_encrypt, object_key_aes};
                    use sha1::{Digest as _, Sha1};
                    let mut hasher = Sha1::new();
                    hasher.update(&seed_and_perms[..20]);
                    hasher.update(&recipient_blob);
                    hasher.update(&seed_and_perms[20..24]);
                    let hash = hasher.finalize();
                    let file_key: [u8; 16] = hash[..16].try_into().expect("16 bytes");
                    let object_key = object_key_aes(&file_key, 4, 0);
                    let iv = [0x77u8; 16];
                    let cipher = aes_128_cbc_encrypt(&object_key, &iv, &plaintext_content);
                    (
                        file_key.to_vec(),
                        cipher,
                        "adbe.pkcs7.s4",
                        4i32,
                        4i32,
                        128i32,
                        "AESV2",
                    )
                }
                other => panic!("unsupported sub_filter for fixture builder: {other}"),
            };
        let _ = (file_key, length_bits); // silence unused warning paths

        // Hex-encode the recipient blob for embedding as a PDF byte
        // string inside the /Recipients array.
        let blob_hex_string = {
            let mut s = String::from("<");
            for byte in &recipient_blob {
                s.push_str(&format!("{byte:02X}"));
            }
            s.push('>');
            s
        };

        let mut pdf: Vec<u8> = Vec::new();
        pdf.extend_from_slice(b"%PDF-1.7\n");

        let catalog_offset = pdf.len();
        pdf.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
        let pages_offset = pdf.len();
        pdf.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Count 1 /Kids [3 0 R] >>\nendobj\n");
        let page_offset = pdf.len();
        pdf.extend_from_slice(
            b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
              /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>\nendobj\n",
        );
        let content_offset = pdf.len();
        pdf.extend_from_slice(
            format!("4 0 obj\n<< /Length {} >>\nstream\n", content_cipher.len()).as_bytes(),
        );
        pdf.extend_from_slice(&content_cipher);
        pdf.extend_from_slice(b"\nendstream\nendobj\n");
        let font_offset = pdf.len();
        pdf.extend_from_slice(
            b"5 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica \
              /Encoding /WinAnsiEncoding >>\nendobj\n",
        );

        let encrypt_offset = pdf.len();
        if v_value == 5 {
            pdf.extend_from_slice(
                format!(
                    "6 0 obj\n<< /Filter /Adobe.PubSec /SubFilter /{sub_filter_str} \
                 /V {v_value} /R {r_value} /Length {length_bits} \
                 /CF << /DefaultCryptFilter << /CFM /{cfm_name} /Length 32 \
                 /AuthEvent /DocOpen /Recipients [{blob_hex_string}] >> >> \
                 /StmF /DefaultCryptFilter /StrF /DefaultCryptFilter \
                 /EncryptMetadata true >>\nendobj\n"
                )
                .as_bytes(),
            );
        } else {
            // V=4 stores /Recipients at the top level, not per-CF.
            pdf.extend_from_slice(
                format!(
                    "6 0 obj\n<< /Filter /Adobe.PubSec /SubFilter /{sub_filter_str} \
                 /V {v_value} /R {r_value} /Length {length_bits} \
                 /CF << /DefaultCryptFilter << /CFM /{cfm_name} /Length 16 \
                 /AuthEvent /DocOpen >> >> \
                 /StmF /DefaultCryptFilter /StrF /DefaultCryptFilter \
                 /Recipients [{blob_hex_string}] /EncryptMetadata true >>\nendobj\n"
                )
                .as_bytes(),
            );
        }

        let xref_offset = pdf.len();
        pdf.extend_from_slice(b"xref\n0 7\n");
        pdf.extend_from_slice(b"0000000000 65535 f \n");
        for offset in [
            catalog_offset,
            pages_offset,
            page_offset,
            content_offset,
            font_offset,
            encrypt_offset,
        ] {
            pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        pdf.extend_from_slice(
            b"trailer\n<< /Size 7 /Root 1 0 R /Encrypt 6 0 R /ID [<00112233445566778899AABBCCDDEEFF><00112233445566778899AABBCCDDEEFF>] >>\n",
        );
        pdf.extend_from_slice(format!("startxref\n{xref_offset}\n%%EOF\n").as_bytes());

        PubSecFixture {
            pdf,
            cert_der,
            private_key_der,
            plaintext: plaintext_content,
        }
    }

    #[test]
    fn parses_pubsec_s5_encrypted_pdf() {
        let fixture = build_pubsec_encrypted_pdf("adbe.pkcs7.s5");
        let document =
            parse_pdf_with_certificate(&fixture.pdf, &fixture.cert_der, &fixture.private_key_der)
                .expect("PubSec s5 PDF should decrypt with matching certificate");
        assert_decrypts_content_stream(&document, &fixture.plaintext);
    }

    #[test]
    fn parses_pubsec_s4_encrypted_pdf() {
        let fixture = build_pubsec_encrypted_pdf("adbe.pkcs7.s4");
        let document =
            parse_pdf_with_certificate(&fixture.pdf, &fixture.cert_der, &fixture.private_key_der)
                .expect("PubSec s4 PDF should decrypt with matching certificate");
        assert_decrypts_content_stream(&document, &fixture.plaintext);
    }

    #[test]
    fn pubsec_rejects_password_credential() {
        let fixture = build_pubsec_encrypted_pdf("adbe.pkcs7.s5");
        let err = parse_pdf_with_password(&fixture.pdf, b"any-password")
            .expect_err("PubSec PDF must reject a password credential");
        match err {
            PdfError::Unsupported(message) => {
                assert!(
                    message.contains("certificate"),
                    "error should mention certificate, got: {message}"
                );
            }
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }

    #[test]
    fn pubsec_s5_rejects_unknown_certificate() {
        // Build a fixture for one keypair, then attempt to open with a
        // different keypair's cert. The right blob is present in the
        // PDF but no recipient matches the supplied cert / key.
        use der::asn1::{Any, PrintableString, SetOfVec};
        use der::{Decode, Encode};
        use rand_chacha::ChaCha8Rng;
        use rand_core::SeedableRng;
        use rsa::RsaPrivateKey;
        use rsa::pkcs1v15::SigningKey;
        use rsa::pkcs8::{EncodePrivateKey, EncodePublicKey};
        use sha2::Sha256;
        use spki::SubjectPublicKeyInfoOwned;
        use std::time::Duration;
        use x509_cert::attr::AttributeTypeAndValue;
        use x509_cert::builder::{Builder, CertificateBuilder, Profile};
        use x509_cert::name::{Name, RdnSequence, RelativeDistinguishedName};
        use x509_cert::serial_number::SerialNumber;
        use x509_cert::time::Validity;

        let fixture = build_pubsec_encrypted_pdf("adbe.pkcs7.s5");

        // Different seed → different keypair.
        let mut rng = ChaCha8Rng::from_seed([0x99u8; 32]);
        let other_private = RsaPrivateKey::new(&mut rng, 2048).expect("other RSA-2048 keygen");
        let other_public = rsa::RsaPublicKey::from(&other_private);
        let other_pkcs8 = other_private
            .to_pkcs8_der()
            .expect("PKCS#8 encode")
            .as_bytes()
            .to_vec();

        let cn = AttributeTypeAndValue {
            oid: const_oid::db::rfc4519::CN,
            value: Any::from(&PrintableString::new(b"unrelated-cert").expect("printable string")),
        };
        let rdn_set = SetOfVec::try_from(vec![cn]).expect("rdn set");
        let mut subject = RdnSequence::default();
        subject.0.push(RelativeDistinguishedName::from(rdn_set));
        let subject_name =
            Name::from_der(&subject.to_der().expect("subject encode")).expect("subject re-decode");
        let signer: SigningKey<Sha256> = SigningKey::new(other_private.clone());
        let other_pub_der = other_public
            .to_public_key_der()
            .expect("RSA public key DER");
        let pub_key_info =
            SubjectPublicKeyInfoOwned::try_from(other_pub_der.as_bytes()).expect("SPKI from DER");
        let cert_builder = CertificateBuilder::new(
            Profile::Root,
            SerialNumber::from(0x55u32),
            Validity::from_now(Duration::from_secs(3600 * 24 * 30)).expect("validity"),
            subject_name,
            pub_key_info,
            &signer,
        )
        .expect("CertificateBuilder::new");
        let other_cert: x509_cert::Certificate = cert_builder.build().expect("cert build");
        let other_cert_der = other_cert.to_der().expect("cert DER");

        let err = parse_pdf_with_certificate(&fixture.pdf, &other_cert_der, &other_pkcs8)
            .expect_err("unrelated certificate must not unlock the PubSec PDF");
        assert_eq!(err, PdfError::InvalidPassword);
    }

    #[test]
    fn standard_pdf_rejects_certificate_credential() {
        let (pdf, _) = build_aes_128_encrypted_pdf(b"", b"ownerpw", true);
        // Any DER-shaped buffers will do: dispatcher rejects before the
        // PubSec code ever inspects them.
        let err = parse_pdf_with_certificate(&pdf, &[0x30, 0x00], &[0x30, 0x00])
            .expect_err("Standard-encrypted PDF must reject a certificate credential");
        match err {
            PdfError::Unsupported(message) => {
                assert!(
                    message.contains("password"),
                    "error should mention password, got: {message}"
                );
            }
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }
}
