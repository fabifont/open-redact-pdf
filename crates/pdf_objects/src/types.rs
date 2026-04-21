use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::{PdfError, PdfResult};

pub type PdfDictionary = BTreeMap<String, PdfValue>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ObjectRef {
    pub object_number: u32,
    pub generation: u16,
}

impl ObjectRef {
    pub const fn new(object_number: u32, generation: u16) -> Self {
        Self {
            object_number,
            generation,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PdfString(pub Vec<u8>);

impl PdfString {
    pub fn to_lossy_string(&self) -> String {
        String::from_utf8_lossy(&self.0).into_owned()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PdfValue {
    Null,
    Bool(bool),
    Integer(i64),
    Number(f64),
    Name(String),
    String(PdfString),
    Array(Vec<PdfValue>),
    Dictionary(PdfDictionary),
    Reference(ObjectRef),
}

impl PdfValue {
    pub fn as_name(&self) -> Option<&str> {
        match self {
            PdfValue::Name(value) => Some(value.as_str()),
            _ => None,
        }
    }

    pub fn as_integer(&self) -> Option<i64> {
        match self {
            PdfValue::Integer(value) => Some(*value),
            PdfValue::Number(value) if value.fract() == 0.0 => Some(*value as i64),
            _ => None,
        }
    }

    pub fn as_number(&self) -> Option<f64> {
        match self {
            PdfValue::Integer(value) => Some(*value as f64),
            PdfValue::Number(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[PdfValue]> {
        match self {
            PdfValue::Array(values) => Some(values),
            _ => None,
        }
    }

    pub fn as_dictionary(&self) -> Option<&PdfDictionary> {
        match self {
            PdfValue::Dictionary(dictionary) => Some(dictionary),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PdfStream {
    pub dict: PdfDictionary,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PdfObject {
    Value(PdfValue),
    Stream(PdfStream),
}

#[derive(Debug, Clone, PartialEq)]
pub enum XrefEntry {
    Free,
    Uncompressed {
        offset: usize,
        generation: u16,
    },
    Compressed {
        stream_object_number: u32,
        index: u32,
    },
}

#[derive(Debug, Clone)]
pub struct PdfFile {
    pub version: String,
    pub objects: BTreeMap<ObjectRef, PdfObject>,
    pub trailer: PdfDictionary,
    pub max_object_number: u32,
}

impl PdfFile {
    pub fn get_object(&self, object_ref: ObjectRef) -> PdfResult<&PdfObject> {
        self.objects.get(&object_ref).ok_or_else(|| {
            PdfError::MissingObject(format!(
                "{} {}",
                object_ref.object_number, object_ref.generation
            ))
        })
    }

    pub fn get_object_mut(&mut self, object_ref: ObjectRef) -> PdfResult<&mut PdfObject> {
        self.objects.get_mut(&object_ref).ok_or_else(|| {
            PdfError::MissingObject(format!(
                "{} {}",
                object_ref.object_number, object_ref.generation
            ))
        })
    }

    pub fn get_value(&self, object_ref: ObjectRef) -> PdfResult<&PdfValue> {
        match self.get_object(object_ref)? {
            PdfObject::Value(value) => Ok(value),
            PdfObject::Stream(_) => Err(PdfError::Corrupt(format!(
                "expected value object at {} {}",
                object_ref.object_number, object_ref.generation
            ))),
        }
    }

    pub fn get_dictionary(&self, object_ref: ObjectRef) -> PdfResult<&PdfDictionary> {
        match self.get_value(object_ref)? {
            PdfValue::Dictionary(dictionary) => Ok(dictionary),
            _ => Err(PdfError::Corrupt(format!(
                "expected dictionary at {} {}",
                object_ref.object_number, object_ref.generation
            ))),
        }
    }

    pub fn resolve<'a>(&'a self, value: &'a PdfValue) -> PdfResult<&'a PdfValue> {
        match value {
            PdfValue::Reference(object_ref) => self.get_value(*object_ref),
            _ => Ok(value),
        }
    }

    pub fn resolve_dict<'a>(&'a self, value: &'a PdfValue) -> PdfResult<&'a PdfDictionary> {
        self.resolve(value)?
            .as_dictionary()
            .ok_or_else(|| PdfError::Corrupt("expected dictionary value".to_string()))
    }

    pub fn allocate_object_ref(&mut self) -> ObjectRef {
        self.max_object_number += 1;
        ObjectRef::new(self.max_object_number, 0)
    }

    pub fn insert_object(&mut self, object_ref: ObjectRef, object: PdfObject) {
        self.max_object_number = self.max_object_number.max(object_ref.object_number);
        self.objects.insert(object_ref, object);
    }
}
