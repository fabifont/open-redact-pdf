use pdf_objects::{PdfFile, serialize_pdf};

pub fn save_document(file: &PdfFile) -> Vec<u8> {
    serialize_pdf(file)
}
