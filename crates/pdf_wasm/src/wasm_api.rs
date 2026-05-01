use std::cell::RefCell;

use open_redact_pdf::{PdfDocument, RedactionPlan};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub struct PdfHandle {
    document: RefCell<PdfDocument>,
}

#[wasm_bindgen(js_name = openPdf)]
pub fn open_pdf(input: Vec<u8>) -> Result<PdfHandle, JsValue> {
    let document = PdfDocument::open(&input).map_err(to_js_error)?;
    Ok(PdfHandle {
        document: RefCell::new(document),
    })
}

#[wasm_bindgen(js_name = openPdfWithPassword)]
pub fn open_pdf_with_password(input: Vec<u8>, password: String) -> Result<PdfHandle, JsValue> {
    let document =
        PdfDocument::open_with_password(&input, password.as_bytes()).map_err(to_js_error)?;
    Ok(PdfHandle {
        document: RefCell::new(document),
    })
}

#[wasm_bindgen(js_name = openPdfWithCertificate)]
pub fn open_pdf_with_certificate(
    input: Vec<u8>,
    cert_der: Vec<u8>,
    private_key_der: Vec<u8>,
) -> Result<PdfHandle, JsValue> {
    let document = PdfDocument::open_with_certificate(&input, &cert_der, &private_key_der)
        .map_err(to_js_error)?;
    Ok(PdfHandle {
        document: RefCell::new(document),
    })
}

#[wasm_bindgen(js_name = getPageCount)]
pub fn get_page_count(handle: &PdfHandle) -> usize {
    handle.document.borrow().page_count()
}

#[wasm_bindgen(js_name = getPageSize)]
pub fn get_page_size(handle: &PdfHandle, page_index: usize) -> Result<JsValue, JsValue> {
    let page_size = handle
        .document
        .borrow()
        .page_size(page_index)
        .map_err(to_js_error)?;
    serde_wasm_bindgen::to_value(&page_size).map_err(|error| JsValue::from_str(&error.to_string()))
}

#[wasm_bindgen(js_name = extractText)]
pub fn extract_text(handle: &PdfHandle, page_index: usize) -> Result<JsValue, JsValue> {
    let page_text = handle
        .document
        .borrow()
        .extract_text(page_index)
        .map_err(to_js_error)?;
    serde_wasm_bindgen::to_value(&page_text).map_err(|error| JsValue::from_str(&error.to_string()))
}

#[wasm_bindgen(js_name = searchText)]
pub fn search_text(
    handle: &PdfHandle,
    page_index: usize,
    query: String,
) -> Result<JsValue, JsValue> {
    let matches = handle
        .document
        .borrow()
        .search_text(page_index, &query)
        .map_err(to_js_error)?;
    serde_wasm_bindgen::to_value(&matches).map_err(|error| JsValue::from_str(&error.to_string()))
}

#[wasm_bindgen(js_name = applyRedactions)]
pub fn apply_redactions(handle: &PdfHandle, plan: JsValue) -> Result<JsValue, JsValue> {
    let plan: RedactionPlan = serde_wasm_bindgen::from_value(plan)
        .map_err(|error| JsValue::from_str(&error.to_string()))?;
    let report = handle
        .document
        .borrow_mut()
        .apply_redactions(plan)
        .map_err(to_js_error)?;
    serde_wasm_bindgen::to_value(&report).map_err(|error| JsValue::from_str(&error.to_string()))
}

#[wasm_bindgen(js_name = savePdf)]
pub fn save_pdf(handle: &PdfHandle) -> Result<Vec<u8>, JsValue> {
    handle.document.borrow().save().map_err(to_js_error)
}

fn to_js_error(error: open_redact_pdf::PdfError) -> JsValue {
    JsValue::from_str(&error.to_string())
}
