use pdf_graphics::{PageBox, Rect};

use crate::error::{PdfError, PdfResult};
use crate::types::{ObjectRef, PdfDictionary, PdfFile, PdfObject, PdfValue};

#[derive(Debug, Clone)]
pub struct DocumentCatalog {
    pub catalog_ref: ObjectRef,
    pub pages_ref: ObjectRef,
}

#[derive(Debug, Clone)]
pub struct PageInfo {
    pub page_ref: ObjectRef,
    pub resources: PdfDictionary,
    pub page_box: PageBox,
    pub content_refs: Vec<ObjectRef>,
    pub annotation_refs: Vec<ObjectRef>,
}

#[derive(Debug, Clone)]
pub struct ParsedDocument {
    pub file: PdfFile,
    pub catalog: DocumentCatalog,
    pub pages: Vec<PageInfo>,
}

pub fn build_document(file: PdfFile) -> PdfResult<ParsedDocument> {
    if file.trailer.contains_key("Encrypt") {
        return Err(PdfError::Unsupported(
            "encrypted PDFs are not supported".to_string(),
        ));
    }

    let root = file
        .trailer
        .get("Root")
        .ok_or_else(|| PdfError::Corrupt("trailer is missing Root".to_string()))?;
    let root_ref = match root {
        PdfValue::Reference(object_ref) => *object_ref,
        _ => return Err(PdfError::Corrupt("Root is not a reference".to_string())),
    };
    let root_dict = file.get_dictionary(root_ref)?;
    if root_dict.get("Type").and_then(PdfValue::as_name) != Some("Catalog") {
        return Err(PdfError::Corrupt("Root catalog has wrong type".to_string()));
    }

    let pages_ref = match root_dict.get("Pages") {
        Some(PdfValue::Reference(object_ref)) => *object_ref,
        _ => return Err(PdfError::Corrupt("Catalog is missing Pages".to_string())),
    };
    let catalog = DocumentCatalog {
        catalog_ref: root_ref,
        pages_ref,
    };

    let mut pages = Vec::new();
    let mut visited = std::collections::BTreeSet::new();
    collect_pages(&file, pages_ref, &mut pages, None, None, None, 0, &mut visited)?;

    Ok(ParsedDocument {
        file,
        catalog,
        pages,
    })
}

const MAX_PAGE_TREE_DEPTH: usize = 64;

#[allow(clippy::too_many_arguments)]
fn collect_pages(
    file: &PdfFile,
    node_ref: ObjectRef,
    output: &mut Vec<PageInfo>,
    inherited_resources: Option<&PdfDictionary>,
    inherited_media_box: Option<Rect>,
    inherited_rotate: Option<i32>,
    depth: usize,
    visited: &mut std::collections::BTreeSet<ObjectRef>,
) -> PdfResult<()> {
    if depth > MAX_PAGE_TREE_DEPTH {
        return Err(PdfError::Corrupt("page tree exceeds maximum depth".to_string()));
    }
    if !visited.insert(node_ref) {
        return Err(PdfError::Corrupt("cycle detected in page tree".to_string()));
    }
    let dictionary = file.get_dictionary(node_ref)?;
    match dictionary.get("Type").and_then(PdfValue::as_name) {
        Some("Pages") => {
            let resources = dictionary
                .get("Resources")
                .map(|value| file.resolve_dict(value))
                .transpose()?
                .or(inherited_resources);
            let media_box = dictionary
                .get("MediaBox")
                .map(|value| parse_rect(file.resolve(value)?))
                .transpose()?
                .or(inherited_media_box);
            let rotate = dictionary
                .get("Rotate")
                .map(|value| parse_rotation(file.resolve(value)?))
                .transpose()?
                .or(inherited_rotate);
            let kids = dictionary
                .get("Kids")
                .and_then(PdfValue::as_array)
                .ok_or_else(|| PdfError::Corrupt("Pages node is missing Kids".to_string()))?;
            for kid in kids {
                let kid_ref = match kid {
                    PdfValue::Reference(object_ref) => *object_ref,
                    _ => {
                        return Err(PdfError::Corrupt(
                            "Pages Kids entry is not an object reference".to_string(),
                        ));
                    }
                };
                collect_pages(file, kid_ref, output, resources, media_box, rotate, depth + 1, visited)?;
            }
        }
        Some("Page") => {
            let resources = dictionary
                .get("Resources")
                .map(|value| file.resolve_dict(value))
                .transpose()?
                .or(inherited_resources)
                .cloned()
                .ok_or_else(|| PdfError::Corrupt("page is missing Resources".to_string()))?;
            let media_box = dictionary
                .get("MediaBox")
                .map(|value| parse_rect(file.resolve(value)?))
                .transpose()?
                .or(inherited_media_box)
                .ok_or_else(|| PdfError::Corrupt("page is missing MediaBox".to_string()))?;
            let crop_box = dictionary
                .get("CropBox")
                .map(|value| parse_rect(file.resolve(value)?))
                .transpose()?
                .unwrap_or(media_box);
            let rotate = dictionary
                .get("Rotate")
                .map(|value| parse_rotation(file.resolve(value)?))
                .transpose()?
                .or(inherited_rotate)
                .unwrap_or(0);
            let content_refs = parse_contents_refs(dictionary)?;
            let annotation_refs = dictionary
                .get("Annots")
                .and_then(PdfValue::as_array)
                .map(|entries| {
                    entries
                        .iter()
                        .map(|entry| match entry {
                            PdfValue::Reference(object_ref) => Ok(*object_ref),
                            _ => Err(PdfError::Corrupt(
                                "annotation entry is not a reference".to_string(),
                            )),
                        })
                        .collect::<PdfResult<Vec<_>>>()
                })
                .transpose()?
                .unwrap_or_default();
            output.push(PageInfo {
                page_ref: node_ref,
                resources,
                page_box: PageBox {
                    media_box,
                    crop_box,
                    rotate,
                },
                content_refs,
                annotation_refs,
            });
        }
        other => {
            return Err(PdfError::Corrupt(format!(
                "unexpected page tree node type: {other:?}"
            )));
        }
    }
    Ok(())
}

fn parse_rotation(value: &PdfValue) -> PdfResult<i32> {
    value
        .as_integer()
        .map(|value| value as i32)
        .ok_or_else(|| PdfError::Corrupt("Rotate is not an integer".to_string()))
}

fn parse_rect(value: &PdfValue) -> PdfResult<Rect> {
    let array = value
        .as_array()
        .ok_or_else(|| PdfError::Corrupt("expected box array".to_string()))?;
    if array.len() != 4 {
        return Err(PdfError::Corrupt(
            "box array must contain four numbers".to_string(),
        ));
    }
    let left = array[0]
        .as_number()
        .ok_or_else(|| PdfError::Corrupt("invalid box value".to_string()))?;
    let bottom = array[1]
        .as_number()
        .ok_or_else(|| PdfError::Corrupt("invalid box value".to_string()))?;
    let right = array[2]
        .as_number()
        .ok_or_else(|| PdfError::Corrupt("invalid box value".to_string()))?;
    let top = array[3]
        .as_number()
        .ok_or_else(|| PdfError::Corrupt("invalid box value".to_string()))?;
    Ok(Rect {
        x: left,
        y: bottom,
        width: right - left,
        height: top - bottom,
    }
    .normalize())
}

fn parse_contents_refs(page: &PdfDictionary) -> PdfResult<Vec<ObjectRef>> {
    match page.get("Contents") {
        Some(PdfValue::Reference(object_ref)) => Ok(vec![*object_ref]),
        Some(PdfValue::Array(entries)) => entries
            .iter()
            .map(|entry| match entry {
                PdfValue::Reference(object_ref) => Ok(*object_ref),
                _ => Err(PdfError::Unsupported(
                    "direct content streams are not supported".to_string(),
                )),
            })
            .collect(),
        Some(PdfValue::Dictionary(_)) => Err(PdfError::Unsupported(
            "direct content streams are not supported".to_string(),
        )),
        Some(_) => Err(PdfError::Corrupt(
            "page Contents entry is not a reference or array".to_string(),
        )),
        None => Ok(Vec::new()),
    }
}

pub fn get_stream(file: &PdfFile, object_ref: ObjectRef) -> PdfResult<&crate::types::PdfStream> {
    match file.get_object(object_ref)? {
        PdfObject::Stream(stream) => Ok(stream),
        PdfObject::Value(_) => Err(PdfError::Corrupt(format!(
            "expected stream object at {} {}",
            object_ref.object_number, object_ref.generation
        ))),
    }
}
