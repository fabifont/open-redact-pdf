use std::collections::BTreeMap;
use std::fmt::Write;

use crate::stream::flate_encode;
use crate::types::{ObjectRef, PdfDictionary, PdfFile, PdfObject, PdfString, PdfValue, XrefForm};

/// Maximum number of compressible objects packed into a single
/// `Type /ObjStm` container. Real-world writers typically split between
/// 50 and 100 members per stream; staying near the lower end keeps each
/// stream's decompressed size bounded for readers with conservative
/// per-object memory budgets.
const OBJSTM_CHUNK_SIZE: usize = 100;

pub fn serialize_pdf(file: &PdfFile) -> Vec<u8> {
    match file.xref_form {
        XrefForm::Classic => serialize_classic(file),
        XrefForm::Stream => serialize_with_xref_stream(file),
    }
}

fn serialize_classic(file: &PdfFile) -> Vec<u8> {
    let mut output = Vec::new();
    output.extend_from_slice(
        format!("%PDF-{}\n%\u{00FF}\u{00FF}\u{00FF}\u{00FF}\n", file.version).as_bytes(),
    );

    let mut offsets = BTreeMap::new();
    for (object_ref, object) in &file.objects {
        let offset = output.len();
        offsets.insert(object_ref.object_number, offset);
        output.extend_from_slice(
            format!(
                "{} {} obj\n",
                object_ref.object_number, object_ref.generation
            )
            .as_bytes(),
        );
        match object {
            PdfObject::Value(value) => {
                output.extend_from_slice(serialize_value(value).as_bytes());
                output.extend_from_slice(b"\nendobj\n");
            }
            PdfObject::Stream(stream) => {
                let mut dict = stream.dict.clone();
                dict.insert(
                    "Length".to_string(),
                    PdfValue::Integer(stream.data.len() as i64),
                );
                output.extend_from_slice(serialize_dictionary(&dict).as_bytes());
                output.extend_from_slice(b"\nstream\n");
                output.extend_from_slice(&stream.data);
                if !stream.data.ends_with(b"\n") {
                    output.push(b'\n');
                }
                output.extend_from_slice(b"endstream\nendobj\n");
            }
        }
    }

    let startxref = output.len();
    let size = file.max_object_number + 1;
    output.extend_from_slice(format!("xref\n0 {}\n", size).as_bytes());
    output.extend_from_slice(b"0000000000 65535 f \n");
    for object_number in 1..=file.max_object_number {
        if let Some(offset) = offsets.get(&object_number).copied() {
            output.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        } else {
            output.extend_from_slice(b"0000000000 65535 f \n");
        }
    }

    let mut trailer = file.trailer.clone();
    trailer.insert("Size".to_string(), PdfValue::Integer(size as i64));
    trailer.remove("Prev");
    trailer.remove("XRefStm");
    output.extend_from_slice(b"trailer\n");
    output.extend_from_slice(serialize_dictionary(&trailer).as_bytes());
    output.extend_from_slice(format!("\nstartxref\n{startxref}\n%%EOF\n").as_bytes());
    output
}

/// One row of the cross-reference stream. Mirrors the parsed
/// `XrefEntry` shape but with explicit byte offsets needed only at
/// emit time.
#[derive(Debug, Clone, Copy)]
enum XrefRow {
    Free,
    Direct {
        offset: usize,
        generation: u16,
    },
    InObjStm {
        stream_objnum: u32,
        index: u32,
    },
}

/// A freshly-built `Type /ObjStm` container. `members` records each
/// packed value's original `ObjectRef` and its index inside the stream.
struct PackedObjStm {
    container_objnum: u32,
    body: Vec<u8>,
    first: usize,
    members: Vec<(ObjectRef, u32)>,
}

fn serialize_with_xref_stream(file: &PdfFile) -> Vec<u8> {
    // 1. Partition: PdfObject::Stream stays as a direct indirect object;
    //    PdfObject::Value with generation 0 is eligible for ObjStm
    //    packing; PdfObject::Value with generation != 0 also stays direct
    //    (cannot be inside an ObjStm per ISO 32000-1 §7.5.7).
    let mut direct: Vec<(ObjectRef, &PdfObject)> = Vec::new();
    let mut compressible: Vec<(ObjectRef, &PdfValue)> = Vec::new();
    for (object_ref, object) in &file.objects {
        match object {
            PdfObject::Value(value) if object_ref.generation == 0 => {
                compressible.push((*object_ref, value));
            }
            _ => direct.push((*object_ref, object)),
        }
    }

    // 2. Pack compressible objects into one or more ObjStm containers.
    //    Allocate fresh object numbers from `max_object_number + 1`.
    let mut next_objnum = file.max_object_number + 1;
    let mut packed_streams = Vec::new();
    for chunk in compressible.chunks(OBJSTM_CHUNK_SIZE) {
        let pack = pack_objstm_chunk(next_objnum, chunk);
        next_objnum += 1;
        packed_streams.push(pack);
    }

    // 3. Allocate xref-stream object number after all ObjStm containers.
    let xref_stream_objnum = next_objnum;
    let xref_size = xref_stream_objnum + 1;

    // 4. Emit header + direct objects + ObjStm containers, capturing
    //    each object's byte offset.
    let mut output = Vec::new();
    output.extend_from_slice(
        format!("%PDF-{}\n%\u{00FF}\u{00FF}\u{00FF}\u{00FF}\n", file.version).as_bytes(),
    );

    let mut direct_offsets: BTreeMap<u32, usize> = BTreeMap::new();
    for (object_ref, object) in &direct {
        let offset = output.len();
        direct_offsets.insert(object_ref.object_number, offset);
        write_indirect_object(&mut output, *object_ref, object);
    }

    let mut objstm_offsets: BTreeMap<u32, usize> = BTreeMap::new();
    for pack in &packed_streams {
        let offset = output.len();
        objstm_offsets.insert(pack.container_objnum, offset);
        write_objstm_container(&mut output, pack);
    }

    // 5. Build xref rows for every object number 0..xref_size.
    let mut rows: Vec<XrefRow> = vec![XrefRow::Free; xref_size as usize];
    for (object_ref, _) in &direct {
        if let Some(offset) = direct_offsets.get(&object_ref.object_number).copied() {
            rows[object_ref.object_number as usize] = XrefRow::Direct {
                offset,
                generation: object_ref.generation,
            };
        }
    }
    for pack in &packed_streams {
        for (member_ref, index) in &pack.members {
            rows[member_ref.object_number as usize] = XrefRow::InObjStm {
                stream_objnum: pack.container_objnum,
                index: *index,
            };
        }
        if let Some(offset) = objstm_offsets.get(&pack.container_objnum).copied() {
            rows[pack.container_objnum as usize] = XrefRow::Direct {
                offset,
                generation: 0,
            };
        }
    }

    // 6. Pick widths and serialize entry table.
    let max_offset = direct_offsets
        .values()
        .chain(objstm_offsets.values())
        .copied()
        .max()
        .unwrap_or(0);
    let max_member_index = packed_streams
        .iter()
        .flat_map(|p| p.members.iter().map(|(_, i)| *i))
        .max()
        .unwrap_or(0)
        .max(file.max_object_number);
    let widths = xref_entry_widths(max_offset, max_member_index);
    let xref_data = build_xref_stream_data(&rows, widths);

    // 7. Build the xref-stream dict (carry trailer keys minus ones we
    //    rewrite ourselves).
    let mut xref_dict = file.trailer.clone();
    for key in [
        "Prev", "XRefStm", "Encrypt", "Length", "Filter", "DecodeParms", "W", "Index", "Type",
    ] {
        xref_dict.remove(key);
    }
    xref_dict.insert("Type".to_string(), PdfValue::Name("XRef".to_string()));
    xref_dict.insert("Size".to_string(), PdfValue::Integer(xref_size as i64));
    xref_dict.insert(
        "W".to_string(),
        PdfValue::Array(
            widths
                .iter()
                .map(|w| PdfValue::Integer(i64::from(*w)))
                .collect(),
        ),
    );
    xref_dict.insert(
        "Filter".to_string(),
        PdfValue::Name("FlateDecode".to_string()),
    );

    // Compress xref body with Flate to match what real producers emit.
    let compressed_xref = flate_encode(&xref_data).expect("flate_encode is infallible for in-memory buffers");
    xref_dict.insert(
        "Length".to_string(),
        PdfValue::Integer(compressed_xref.len() as i64),
    );

    // 8. Emit xref stream as the final object; capture its offset.
    let startxref = output.len();
    output.extend_from_slice(
        format!("{} 0 obj\n", xref_stream_objnum).as_bytes(),
    );
    output.extend_from_slice(serialize_dictionary(&xref_dict).as_bytes());
    output.extend_from_slice(b"\nstream\n");
    output.extend_from_slice(&compressed_xref);
    output.extend_from_slice(b"\nendstream\nendobj\n");

    // 9. Trailer + EOF.
    output.extend_from_slice(format!("startxref\n{startxref}\n%%EOF\n").as_bytes());
    output
}

fn write_indirect_object(output: &mut Vec<u8>, object_ref: ObjectRef, object: &PdfObject) {
    output.extend_from_slice(
        format!("{} {} obj\n", object_ref.object_number, object_ref.generation).as_bytes(),
    );
    match object {
        PdfObject::Value(value) => {
            output.extend_from_slice(serialize_value(value).as_bytes());
            output.extend_from_slice(b"\nendobj\n");
        }
        PdfObject::Stream(stream) => {
            let mut dict = stream.dict.clone();
            dict.insert(
                "Length".to_string(),
                PdfValue::Integer(stream.data.len() as i64),
            );
            output.extend_from_slice(serialize_dictionary(&dict).as_bytes());
            output.extend_from_slice(b"\nstream\n");
            output.extend_from_slice(&stream.data);
            if !stream.data.ends_with(b"\n") {
                output.push(b'\n');
            }
            output.extend_from_slice(b"endstream\nendobj\n");
        }
    }
}

fn write_objstm_container(output: &mut Vec<u8>, pack: &PackedObjStm) {
    let mut dict = PdfDictionary::new();
    dict.insert("Type".to_string(), PdfValue::Name("ObjStm".to_string()));
    dict.insert(
        "N".to_string(),
        PdfValue::Integer(pack.members.len() as i64),
    );
    dict.insert("First".to_string(), PdfValue::Integer(pack.first as i64));
    dict.insert(
        "Filter".to_string(),
        PdfValue::Name("FlateDecode".to_string()),
    );
    dict.insert(
        "Length".to_string(),
        PdfValue::Integer(pack.body.len() as i64),
    );
    output.extend_from_slice(format!("{} 0 obj\n", pack.container_objnum).as_bytes());
    output.extend_from_slice(serialize_dictionary(&dict).as_bytes());
    output.extend_from_slice(b"\nstream\n");
    output.extend_from_slice(&pack.body);
    if !pack.body.ends_with(b"\n") {
        output.push(b'\n');
    }
    output.extend_from_slice(b"endstream\nendobj\n");
}

fn pack_objstm_chunk(container_objnum: u32, chunk: &[(ObjectRef, &PdfValue)]) -> PackedObjStm {
    // Build the prefix "objnum1 offset1 objnum2 offset2 ..." and the
    // body of serialized values back-to-back. The header length is the
    // /First entry; each value's offset is its position in the body.
    let mut header = String::new();
    let mut body_text = String::new();
    let mut members: Vec<(ObjectRef, u32)> = Vec::new();
    let mut running_offset = 0usize;
    for (index, (object_ref, value)) in chunk.iter().enumerate() {
        write!(
            header,
            "{} {} ",
            object_ref.object_number, running_offset
        )
        .expect("string writes should succeed");
        let serialized = serialize_value(value);
        body_text.push_str(&serialized);
        body_text.push(' ');
        running_offset += serialized.len() + 1;
        members.push((*object_ref, index as u32));
    }
    let header_bytes = header.into_bytes();
    let first = header_bytes.len();
    let mut decompressed = header_bytes;
    decompressed.extend_from_slice(body_text.as_bytes());
    let body = flate_encode(&decompressed)
        .expect("flate_encode is infallible for in-memory buffers");
    PackedObjStm {
        container_objnum,
        body,
        first,
        members,
    }
}

fn xref_entry_widths(max_offset: usize, max_member_index: u32) -> [u8; 3] {
    let field2 = bytes_to_fit(max_offset as u64).max(1);
    let field3 = bytes_to_fit(u64::from(max_member_index)).max(1);
    [1, field2, field3]
}

fn bytes_to_fit(value: u64) -> u8 {
    if value == 0 {
        return 1;
    }
    let mut bits = 0u32;
    let mut v = value;
    while v > 0 {
        bits += 1;
        v >>= 1;
    }
    bits.div_ceil(8) as u8
}

fn build_xref_stream_data(rows: &[XrefRow], widths: [u8; 3]) -> Vec<u8> {
    let mut output = Vec::with_capacity(rows.len() * (widths[0] + widths[1] + widths[2]) as usize);
    for row in rows {
        match row {
            XrefRow::Free => {
                push_be(&mut output, 0, widths[0]);
                push_be(&mut output, 0, widths[1]);
                push_be(&mut output, 0, widths[2]);
            }
            XrefRow::Direct { offset, generation } => {
                push_be(&mut output, 1, widths[0]);
                push_be(&mut output, *offset as u64, widths[1]);
                push_be(&mut output, u64::from(*generation), widths[2]);
            }
            XrefRow::InObjStm {
                stream_objnum,
                index,
            } => {
                push_be(&mut output, 2, widths[0]);
                push_be(&mut output, u64::from(*stream_objnum), widths[1]);
                push_be(&mut output, u64::from(*index), widths[2]);
            }
        }
    }
    output
}

fn push_be(output: &mut Vec<u8>, value: u64, width: u8) {
    let width = width as usize;
    for i in (0..width).rev() {
        output.push(((value >> (i * 8)) & 0xff) as u8);
    }
}

pub fn serialize_value(value: &PdfValue) -> String {
    match value {
        PdfValue::Null => "null".to_string(),
        PdfValue::Bool(value) => value.to_string(),
        PdfValue::Integer(value) => value.to_string(),
        PdfValue::Number(value) => {
            if value.fract() == 0.0 {
                format!("{:.0}", value)
            } else {
                let mut number = format!("{value:.6}");
                while number.contains('.') && number.ends_with('0') {
                    number.pop();
                }
                if number.ends_with('.') {
                    number.pop();
                }
                number
            }
        }
        PdfValue::Name(name) => {
            let mut encoded = String::from("/");
            for byte in name.bytes() {
                if byte == b'#'
                    || byte <= b' '
                    || byte >= 0x7F
                    || matches!(
                        byte,
                        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
                    )
                {
                    encoded.push_str(&format!("#{:02X}", byte));
                } else {
                    encoded.push(byte as char);
                }
            }
            encoded
        }
        PdfValue::String(string) => serialize_string(string),
        PdfValue::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(serialize_value)
                .collect::<Vec<_>>()
                .join(" ")
        ),
        PdfValue::Dictionary(dictionary) => serialize_dictionary(dictionary),
        PdfValue::Reference(object_ref) => {
            format!("{} {} R", object_ref.object_number, object_ref.generation)
        }
    }
}

pub fn serialize_dictionary(dictionary: &PdfDictionary) -> String {
    let mut output = String::from("<<");
    for (key, value) in dictionary {
        write!(output, "/{} {}", key, serialize_value(value))
            .expect("string writes should succeed");
        output.push(' ');
    }
    output.push_str(">>");
    output
}

pub fn serialize_string(string: &PdfString) -> String {
    let mut output = String::from("(");
    for byte in &string.0 {
        match byte {
            b'(' | b')' | b'\\' => {
                output.push('\\');
                output.push(*byte as char);
            }
            b'\n' => output.push_str("\\n"),
            b'\r' => output.push_str("\\r"),
            b'\t' => output.push_str("\\t"),
            0x08 => output.push_str("\\b"),
            0x0C => output.push_str("\\f"),
            byte if byte.is_ascii_graphic() || *byte == b' ' => output.push(*byte as char),
            other => output.push_str(&format!("\\{:03o}", other)),
        }
    }
    output.push(')');
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xref_entry_widths_picks_minimal_field_widths() {
        // Small offsets and small member counts → 1/1/1.
        assert_eq!(xref_entry_widths(0, 0), [1, 1, 1]);
        assert_eq!(xref_entry_widths(255, 250), [1, 1, 1]);
        // Offsets crossing 8-bit boundary need 2 bytes.
        assert_eq!(xref_entry_widths(256, 0), [1, 2, 1]);
        // Offsets crossing 16-bit boundary need 3 bytes (typical small docs).
        assert_eq!(xref_entry_widths(65_535, 65_535), [1, 2, 2]);
        assert_eq!(xref_entry_widths(65_536, 65_536), [1, 3, 3]);
        // Offsets in 24-bit range need 3 bytes.
        assert_eq!(xref_entry_widths(16_777_215, 0), [1, 3, 1]);
        assert_eq!(xref_entry_widths(16_777_216, 0), [1, 4, 1]);
    }

    #[test]
    fn pack_objstm_chunk_preserves_member_indices() {
        let v1 = PdfValue::Integer(42);
        let v2 = PdfValue::Name("Foo".to_string());
        let v3 = PdfValue::Bool(true);
        let chunk: Vec<(ObjectRef, &PdfValue)> = vec![
            (ObjectRef::new(7, 0), &v1),
            (ObjectRef::new(8, 0), &v2),
            (ObjectRef::new(9, 0), &v3),
        ];
        let pack = pack_objstm_chunk(100, &chunk);
        assert_eq!(pack.container_objnum, 100);
        assert_eq!(pack.members.len(), 3);
        assert_eq!(pack.members[0].1, 0);
        assert_eq!(pack.members[1].1, 1);
        assert_eq!(pack.members[2].1, 2);
        assert_eq!(pack.members[0].0.object_number, 7);
        assert_eq!(pack.members[1].0.object_number, 8);
        assert_eq!(pack.members[2].0.object_number, 9);
        assert!(pack.first > 0, "ObjStm header must have positive length");
    }

    #[test]
    fn build_xref_stream_data_serialises_widths_big_endian() {
        let rows = vec![
            XrefRow::Free,
            XrefRow::Direct {
                offset: 0x1234,
                generation: 0,
            },
            XrefRow::InObjStm {
                stream_objnum: 5,
                index: 3,
            },
        ];
        let widths = [1u8, 2u8, 1u8];
        let data = build_xref_stream_data(&rows, widths);
        // Each row is 1+2+1 = 4 bytes.
        assert_eq!(data.len(), 12);
        // Free row.
        assert_eq!(&data[0..4], &[0, 0, 0, 0]);
        // Direct row: type=1, offset=0x1234 BE → 0x12 0x34, gen=0.
        assert_eq!(&data[4..8], &[1, 0x12, 0x34, 0]);
        // InObjStm row: type=2, stream_objnum=5, index=3.
        assert_eq!(&data[8..12], &[2, 0, 5, 3]);
    }
}
