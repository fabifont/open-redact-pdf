use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use pdf_graphics::Size;
use pdf_objects::{
    PageInfo, ParsedDocument, PdfResult, parse_pdf, parse_pdf_with_certificate,
    parse_pdf_with_password,
};
use pdf_redact::{ApplyReport, apply_redactions};
use pdf_targets::{NormalizedRedactionPlan, normalize_plan};
use pdf_text::{ExtractedPageText, TextItem, analyze_page_text, search_page_text};
use pdf_writer::save_document;
use serde::{Deserialize, Serialize};

/// Normalized page size in PDF user-space units.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageSize {
    /// Page width after crop-box translation and page rotation normalization.
    pub width: f64,
    /// Page height after crop-box translation and page rotation normalization.
    pub height: f64,
}

impl From<Size> for PageSize {
    fn from(value: Size) -> Self {
        Self {
            width: value.width,
            height: value.height,
        }
    }
}

/// Extracted text content and geometry for a single page.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageText {
    /// Zero-based page index.
    pub page_index: usize,
    /// Human-readable page text assembled from the supported extraction subset.
    pub text: String,
    /// Structured text items with bounding geometry suitable for later authoring tools.
    pub items: Vec<TextItem>,
}

/// Parsed PDF document handle used for inspection, redaction, and save.
pub struct PdfDocument {
    parsed: ParsedDocument,
    /// Per-page cached results of [`analyze_page_text`]. Successive
    /// `extract_text` / `search_text` calls on the same page reuse the
    /// cached extraction instead of walking the content stream again;
    /// `apply_redactions` clears the cache since the underlying content
    /// streams are about to be rewritten.
    text_cache: Mutex<HashMap<usize, Arc<ExtractedPageText>>>,
}

impl PdfDocument {
    /// Opens an unencrypted PDF from raw bytes, or an encrypted PDF
    /// whose user password is empty. For encrypted PDFs that require a
    /// user- or owner-supplied password, use
    /// [`PdfDocument::open_with_password`].
    pub fn open(bytes: &[u8]) -> PdfResult<Self> {
        Ok(Self::with_parsed(parse_pdf(bytes)?))
    }

    /// Opens an encrypted PDF from raw bytes using the supplied password.
    /// The password is tried first as the user password, then as the
    /// owner password; if neither authenticates, the function returns
    /// [`pdf_objects::PdfError::InvalidPassword`]. For unencrypted
    /// documents the password is ignored.
    pub fn open_with_password(bytes: &[u8], password: &[u8]) -> PdfResult<Self> {
        Ok(Self::with_parsed(parse_pdf_with_password(bytes, password)?))
    }

    /// Opens an Adobe.PubSec-encrypted PDF using the recipient's X.509
    /// certificate and matching RSA private key, both DER-encoded
    /// (PKCS#8 for the private key, the standard form returned by most
    /// browser key-management APIs). Returns
    /// [`pdf_objects::PdfError::InvalidPassword`] when no recipient blob
    /// in the PDF unwraps with the supplied private key. For
    /// password-encrypted or unencrypted documents this returns
    /// [`pdf_objects::PdfError::Unsupported`] — use
    /// [`PdfDocument::open_with_password`] / [`PdfDocument::open`]
    /// respectively.
    pub fn open_with_certificate(
        bytes: &[u8],
        cert_der: &[u8],
        private_key_der: &[u8],
    ) -> PdfResult<Self> {
        Ok(Self::with_parsed(parse_pdf_with_certificate(
            bytes,
            cert_der,
            private_key_der,
        )?))
    }

    fn with_parsed(parsed: ParsedDocument) -> Self {
        Self {
            parsed,
            text_cache: Mutex::new(HashMap::new()),
        }
    }

    /// Returns the number of pages in the parsed document.
    pub fn page_count(&self) -> usize {
        self.parsed.pages.len()
    }

    /// Returns the normalized page size for a zero-based page index.
    pub fn page_size(&self, page_index: usize) -> PdfResult<PageSize> {
        let page = self
            .parsed
            .pages
            .get(page_index)
            .ok_or(pdf_objects::PdfError::InvalidPageIndex(page_index))?;
        Ok(page.page_box.size().into())
    }

    /// Extracts page text and geometry for the current supported subset.
    pub fn extract_text(&self, page_index: usize) -> PdfResult<PageText> {
        let extracted = self.cached_page_text(page_index)?;
        Ok(PageText {
            page_index,
            text: extracted.text.clone(),
            items: extracted.items.clone(),
        })
    }

    /// Searches page text in visual glyph order and returns page-space match geometry.
    pub fn search_text(&self, page_index: usize, query: &str) -> PdfResult<Vec<TextMatch>> {
        let extracted = self.cached_page_text(page_index)?;
        Ok(search_page_text(&extracted, query))
    }

    /// Applies a redaction plan in place to the opened document.
    pub fn apply_redactions(&mut self, plan: pdf_targets::RedactionPlan) -> PdfResult<ApplyReport> {
        let normalized = self.normalize_plan(plan)?;
        let report = apply_redactions(&mut self.parsed.file, &mut self.parsed.pages, &normalized)?;
        // Content streams have been rewritten, so any cached extraction
        // from before the plan applied is stale.
        if let Ok(mut cache) = self.text_cache.lock() {
            cache.clear();
        }
        Ok(report)
    }

    fn cached_page_text(&self, page_index: usize) -> PdfResult<Arc<ExtractedPageText>> {
        if let Ok(cache) = self.text_cache.lock() {
            if let Some(entry) = cache.get(&page_index) {
                return Ok(entry.clone());
            }
        }
        let page = self.get_page(page_index)?;
        let extracted = Arc::new(analyze_page_text(&self.parsed.file, page_index, page)?);
        if let Ok(mut cache) = self.text_cache.lock() {
            cache.insert(page_index, extracted.clone());
        }
        Ok(extracted)
    }

    /// Saves the current document state as a new deterministic full-save PDF.
    pub fn save(&self) -> PdfResult<Vec<u8>> {
        Ok(save_document(&self.parsed.file))
    }

    fn normalize_plan(
        &self,
        plan: pdf_targets::RedactionPlan,
    ) -> PdfResult<NormalizedRedactionPlan> {
        let sizes = self
            .parsed
            .pages
            .iter()
            .map(|page| page.page_box.size())
            .collect::<Vec<_>>();
        normalize_plan(plan, &sizes)
    }

    fn get_page(&self, page_index: usize) -> PdfResult<&PageInfo> {
        self.parsed
            .pages
            .get(page_index)
            .ok_or(pdf_objects::PdfError::InvalidPageIndex(page_index))
    }
}

pub use pdf_objects::PdfError;
pub use pdf_targets::{FillColor, RedactionMode, RedactionPlan, RedactionTarget};
pub use pdf_text::TextMatch;
