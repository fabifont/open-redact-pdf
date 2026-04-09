use pdf_graphics::Size;
use pdf_objects::{PageInfo, ParsedDocument, PdfResult, parse_pdf};
use pdf_redact::{ApplyReport, apply_redactions};
use pdf_targets::{NormalizedRedactionPlan, normalize_plan};
use pdf_text::{TextItem, analyze_page_text, search_page_text};
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
}

impl PdfDocument {
    /// Opens an unencrypted PDF from raw bytes.
    pub fn open(bytes: &[u8]) -> PdfResult<Self> {
        let parsed = parse_pdf(bytes)?;
        Ok(Self { parsed })
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
        let page = self.get_page(page_index)?;
        let extracted = analyze_page_text(&self.parsed.file, page_index, page)?;
        Ok(PageText {
            page_index,
            text: extracted.text,
            items: extracted.items,
        })
    }

    /// Searches page text in visual glyph order and returns page-space match geometry.
    pub fn search_text(&self, page_index: usize, query: &str) -> PdfResult<Vec<TextMatch>> {
        let page = self.get_page(page_index)?;
        let extracted = analyze_page_text(&self.parsed.file, page_index, page)?;
        Ok(search_page_text(&extracted, query))
    }

    /// Applies a redaction plan in place to the opened document.
    pub fn apply_redactions(&mut self, plan: pdf_targets::RedactionPlan) -> PdfResult<ApplyReport> {
        let normalized = self.normalize_plan(plan)?;
        apply_redactions(&mut self.parsed.file, &mut self.parsed.pages, &normalized)
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
