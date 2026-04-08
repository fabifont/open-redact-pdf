use pdf_graphics::Size;
use pdf_objects::{PageInfo, ParsedDocument, PdfResult, parse_pdf};
use pdf_redact::{ApplyReport, apply_redactions};
use pdf_targets::{NormalizedRedactionPlan, normalize_plan};
use pdf_text::{TextItem, analyze_page_text, search_page_text};
use pdf_writer::save_document;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageSize {
    pub width: f64,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageText {
    pub page_index: usize,
    pub text: String,
    pub items: Vec<TextItem>,
}

pub struct PdfDocument {
    parsed: ParsedDocument,
}

impl PdfDocument {
    pub fn open(bytes: &[u8]) -> PdfResult<Self> {
        let parsed = parse_pdf(bytes)?;
        Ok(Self { parsed })
    }

    pub fn page_count(&self) -> usize {
        self.parsed.pages.len()
    }

    pub fn page_size(&self, page_index: usize) -> PdfResult<PageSize> {
        let page = self
            .parsed
            .pages
            .get(page_index)
            .ok_or(pdf_objects::PdfError::InvalidPageIndex(page_index))?;
        Ok(page.page_box.size().into())
    }

    pub fn extract_text(&self, page_index: usize) -> PdfResult<PageText> {
        let page = self.get_page(page_index)?;
        let extracted = analyze_page_text(&self.parsed.file, page_index, page)?;
        Ok(PageText {
            page_index,
            text: extracted.text,
            items: extracted.items,
        })
    }

    pub fn search_text(&self, page_index: usize, query: &str) -> PdfResult<Vec<TextMatch>> {
        let page = self.get_page(page_index)?;
        let extracted = analyze_page_text(&self.parsed.file, page_index, page)?;
        Ok(search_page_text(&extracted, query))
    }

    pub fn apply_redactions(&mut self, plan: pdf_targets::RedactionPlan) -> PdfResult<ApplyReport> {
        let normalized = self.normalize_plan(plan)?;
        apply_redactions(&mut self.parsed.file, &mut self.parsed.pages, &normalized)
    }

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
pub use pdf_targets::{FillColor, RedactionPlan, RedactionTarget};
pub use pdf_text::TextMatch;
