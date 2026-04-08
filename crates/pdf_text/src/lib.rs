pub mod text;

pub use text::{
    ExtractedPageText, Glyph, GlyphLocation, TextItem, TextMatch,
    analyze_page_text, search_page_text,
};
