pub mod text;

pub use text::{
    ExtractedPageText, Glyph, GlyphLocation, GlyphRange, PageSearchIndex, TextItem, TextMatch,
    analyze_page_text, search_page_text,
};
