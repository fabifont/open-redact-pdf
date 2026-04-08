use std::fmt::Write;

use crate::types::{PdfDictionary, PdfFile, PdfObject, PdfString, PdfValue};

pub fn serialize_pdf(file: &PdfFile) -> Vec<u8> {
    let mut output = Vec::new();
    output.extend_from_slice(
        format!("%PDF-{}\n%\u{00FF}\u{00FF}\u{00FF}\u{00FF}\n", file.version).as_bytes(),
    );

    let mut offsets = std::collections::BTreeMap::new();
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
        PdfValue::Name(name) => format!("/{name}"),
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
