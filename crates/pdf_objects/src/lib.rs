pub mod document;
pub mod error;
pub mod parser;
pub mod serializer;
pub mod stream;
pub mod types;

pub use document::{DocumentCatalog, PageInfo, ParsedDocument};
pub use error::{PdfError, PdfResult};
pub use parser::parse_pdf;
pub use serializer::{serialize_dictionary, serialize_pdf, serialize_string, serialize_value};
pub use stream::decode_stream;
pub use types::{
    ObjectRef, PdfDictionary, PdfFile, PdfObject, PdfStream, PdfString, PdfValue, XrefEntry,
};
