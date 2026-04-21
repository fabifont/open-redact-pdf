pub mod crypto;
pub mod document;
pub mod error;
pub mod parser;
pub mod serializer;
pub mod stream;
pub mod types;

pub use document::{DocumentCatalog, PageInfo, ParsedDocument};
pub use error::{PdfError, PdfResult};
pub use parser::{parse_pdf, parse_pdf_with_password};
pub use serializer::{serialize_dictionary, serialize_pdf, serialize_string, serialize_value};
pub use stream::{decode_stream, flate_encode};
pub use types::{
    ObjectRef, PdfDictionary, PdfFile, PdfObject, PdfStream, PdfString, PdfValue, XrefEntry,
};
