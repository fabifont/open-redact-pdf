//! Partial Image XObject rewriting.
//!
//! When a redaction target only partly overlaps an Image XObject's
//! bounding quad on the page, this module rewrites the image stream so
//! the targeted pixel region is replaced with the plan's `fill_color`
//! while the rest of the image survives. The caller falls back to
//! whole-invocation neutralization for any image whose format we cannot
//! safely decode and re-encode (returns [`PdfError::Unsupported`]).
//!
//! Supported formats:
//! - Raw (no `/Filter`) and `FlateDecode` raster with optional TIFF /
//!   PNG predictor, at 8 bits per component, in `/DeviceGray`,
//!   `/DeviceRGB`, or `/DeviceCMYK`.
//! - `DCTDecode` (JPEG) at 8 bits per component, decoded via
//!   `jpeg-decoder` and re-emitted via `jpeg-encoder` at quality 85.
//!
//! Anything else (`/Indexed`, `/ICCBased`, `BitsPerComponent != 8`,
//! `JBIG2Decode`, `JPXDecode`, `CCITTFaxDecode`, etc.) returns
//! `Unsupported`.

use jpeg_decoder::{Decoder as JpegDecoder, PixelFormat};
use jpeg_encoder::{ColorType as JpegColorType, Encoder as JpegEncoder};
use pdf_graphics::Color;
use pdf_objects::{PdfDictionary, PdfError, PdfResult, PdfStream, PdfValue, decode_stream, flate_encode};

/// Pixel-space rectangle. `(x, y)` is the top-left corner; `w` and `h`
/// are the inclusive width and height in pixels. Always clipped to the
/// image's bounds before being passed to [`mask_image_region`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ImagePixelRect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// Rewritten image stream produced by [`mask_image_region`]. The caller
/// builds a `PdfStream` from these fields and either replaces the
/// existing one (single-page reference) or copy-on-writes a fresh
/// indirect object (multi-page reference).
pub(crate) struct MaskedImage {
    pub new_dict: PdfDictionary,
    pub new_data: Vec<u8>,
}

/// Mask the rectangular pixel region inside the image with the given
/// fill colour. Returns [`PdfError::Unsupported`] when the image's
/// format is outside the supported set; the caller falls back to
/// whole-invocation drop in that case.
pub(crate) fn mask_image_region(
    stream: &PdfStream,
    pixel_rect: ImagePixelRect,
    fill_color: Color,
) -> PdfResult<MaskedImage> {
    let format = detect_format(&stream.dict)?;
    match format {
        ImageFormat::RawOrFlate {
            components,
            width,
            height,
        } => mask_raw_or_flate(stream, components, width, height, pixel_rect, fill_color),
        ImageFormat::Jpeg { width, height } => {
            mask_jpeg(stream, width, height, pixel_rect, fill_color)
        }
    }
}

#[derive(Debug)]
enum ImageFormat {
    RawOrFlate {
        components: u8, // 1, 3, or 4
        width: u32,
        height: u32,
    },
    Jpeg {
        width: u32,
        height: u32,
    },
}

fn detect_format(dict: &PdfDictionary) -> PdfResult<ImageFormat> {
    if dict.get("Subtype").and_then(PdfValue::as_name) != Some("Image") {
        return Err(PdfError::Unsupported(
            "image-mask path requires /Subtype /Image".to_string(),
        ));
    }
    let bpc = dict
        .get("BitsPerComponent")
        .and_then(PdfValue::as_integer)
        .unwrap_or(8);
    if bpc != 8 {
        return Err(PdfError::Unsupported(format!(
            "Image XObject /BitsPerComponent {bpc} is not supported (only 8)"
        )));
    }
    let width = dict
        .get("Width")
        .and_then(PdfValue::as_integer)
        .ok_or_else(|| PdfError::Corrupt("Image XObject is missing /Width".to_string()))?
        as u32;
    let height = dict
        .get("Height")
        .and_then(PdfValue::as_integer)
        .ok_or_else(|| PdfError::Corrupt("Image XObject is missing /Height".to_string()))?
        as u32;
    if width == 0 || height == 0 {
        return Err(PdfError::Corrupt(
            "Image XObject /Width or /Height is zero".to_string(),
        ));
    }
    let components = match dict.get("ColorSpace").and_then(PdfValue::as_name) {
        Some("DeviceGray") => 1u8,
        Some("DeviceRGB") => 3u8,
        Some("DeviceCMYK") => 4u8,
        Some(other) => {
            return Err(PdfError::Unsupported(format!(
                "Image XObject /ColorSpace /{other} is not supported (only DeviceGray, DeviceRGB, DeviceCMYK)"
            )));
        }
        None => {
            // /ImageMask images and untyped colour spaces are not supported.
            return Err(PdfError::Unsupported(
                "Image XObject is missing /ColorSpace (only DeviceGray/RGB/CMYK supported)"
                    .to_string(),
            ));
        }
    };

    let filters = collect_filter_names(dict)?;
    match filters.as_slice() {
        [] => Ok(ImageFormat::RawOrFlate {
            components,
            width,
            height,
        }),
        ["FlateDecode"] => Ok(ImageFormat::RawOrFlate {
            components,
            width,
            height,
        }),
        ["DCTDecode"] => {
            if components != 1 && components != 3 && components != 4 {
                return Err(PdfError::Unsupported(format!(
                    "DCTDecode image with {components} components is not supported"
                )));
            }
            Ok(ImageFormat::Jpeg { width, height })
        }
        other => Err(PdfError::Unsupported(format!(
            "Image XObject filter chain {other:?} is not supported (only [], [FlateDecode], [DCTDecode])"
        ))),
    }
}

fn collect_filter_names(dict: &PdfDictionary) -> PdfResult<Vec<&str>> {
    match dict.get("Filter") {
        None => Ok(Vec::new()),
        Some(PdfValue::Name(name)) => Ok(vec![name.as_str()]),
        Some(PdfValue::Array(values)) => values
            .iter()
            .map(|v| {
                v.as_name().ok_or_else(|| {
                    PdfError::Corrupt("Image /Filter array entry is not a name".to_string())
                })
            })
            .collect(),
        Some(_) => Err(PdfError::Corrupt(
            "Image /Filter has unexpected type".to_string(),
        )),
    }
}

fn mask_raw_or_flate(
    stream: &PdfStream,
    components: u8,
    width: u32,
    height: u32,
    pixel_rect: ImagePixelRect,
    fill_color: Color,
) -> PdfResult<MaskedImage> {
    let mut pixels = decode_stream(stream)?;
    let expected_len = (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(components as usize))
        .ok_or_else(|| {
            PdfError::Corrupt("Image XObject pixel count overflow".to_string())
        })?;
    if pixels.len() < expected_len {
        return Err(PdfError::Corrupt(format!(
            "Image XObject decoded length {} is less than expected {expected_len}",
            pixels.len()
        )));
    }
    pixels.truncate(expected_len);
    paint_mask_rect(&mut pixels, width, components, pixel_rect, fill_color);
    let encoded = flate_encode(&pixels)?;
    let mut new_dict = stream.dict.clone();
    new_dict.insert("Filter".to_string(), PdfValue::Name("FlateDecode".to_string()));
    // The new bytes are predictor-free raw pixels; drop any prior
    // /DecodeParms that referenced a predictor, /Columns, /Colors,
    // /BitsPerComponent (PNG/TIFF predictor knobs).
    new_dict.remove("DecodeParms");
    new_dict.remove("Length"); // recomputed by the writer
    Ok(MaskedImage {
        new_dict,
        new_data: encoded,
    })
}

fn mask_jpeg(
    stream: &PdfStream,
    declared_width: u32,
    declared_height: u32,
    pixel_rect: ImagePixelRect,
    fill_color: Color,
) -> PdfResult<MaskedImage> {
    let mut decoder = JpegDecoder::new(stream.data.as_slice());
    let mut pixels = decoder.decode().map_err(|err| {
        PdfError::Unsupported(format!("DCTDecode JPEG decode failed: {err}"))
    })?;
    let info = decoder.info().ok_or_else(|| {
        PdfError::Corrupt("JPEG decoder produced bytes but no ImageInfo".to_string())
    })?;
    let (components, jpeg_color_type) = match info.pixel_format {
        PixelFormat::L8 => (1u8, JpegColorType::Luma),
        PixelFormat::RGB24 => (3u8, JpegColorType::Rgb),
        PixelFormat::CMYK32 => (4u8, JpegColorType::Cmyk),
        PixelFormat::L16 => {
            return Err(PdfError::Unsupported(
                "16-bit JPEG decode is not supported (Image XObject must be 8 bpc)".to_string(),
            ));
        }
    };
    let width = u32::from(info.width);
    let height = u32::from(info.height);
    if width != declared_width || height != declared_height {
        return Err(PdfError::Corrupt(format!(
            "JPEG decoder reports {width}x{height} but Image XObject /Width /Height say {declared_width}x{declared_height}"
        )));
    }
    let expected_len = (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(components as usize))
        .ok_or_else(|| {
            PdfError::Corrupt("Image XObject pixel count overflow".to_string())
        })?;
    if pixels.len() < expected_len {
        return Err(PdfError::Corrupt(format!(
            "JPEG decode produced {} bytes, expected {expected_len}",
            pixels.len()
        )));
    }
    pixels.truncate(expected_len);
    paint_mask_rect(&mut pixels, width, components, pixel_rect, fill_color);

    let mut encoded: Vec<u8> = Vec::new();
    {
        let encoder = JpegEncoder::new(&mut encoded, 85);
        encoder
            .encode(
                &pixels,
                width as u16,
                height as u16,
                color_type_for_encoder(jpeg_color_type),
            )
            .map_err(|err| PdfError::Corrupt(format!("JPEG re-encode failed: {err}")))?;
    }
    let mut new_dict = stream.dict.clone();
    // /Filter stays /DCTDecode; just drop the cached /Length.
    new_dict.remove("Length");
    new_dict.remove("DecodeParms");
    Ok(MaskedImage {
        new_dict,
        new_data: encoded,
    })
}

fn color_type_for_encoder(decoder_type: JpegColorType) -> jpeg_encoder::ColorType {
    match decoder_type {
        JpegColorType::Luma => jpeg_encoder::ColorType::Luma,
        JpegColorType::Rgb => jpeg_encoder::ColorType::Rgb,
        JpegColorType::Cmyk => jpeg_encoder::ColorType::Cmyk,
        // Other variants are not produced for our supported PixelFormats.
        _ => jpeg_encoder::ColorType::Rgb,
    }
}

fn paint_mask_rect(
    pixels: &mut [u8],
    width: u32,
    components: u8,
    rect: ImagePixelRect,
    fill_color: Color,
) {
    let template = pixel_template(components, fill_color);
    let row_stride = width as usize * components as usize;
    let pix_size = components as usize;
    let x_end = rect.x.saturating_add(rect.w) as usize;
    let y_end = rect.y.saturating_add(rect.h) as usize;
    for y in (rect.y as usize)..y_end {
        let row_base = y * row_stride;
        for x in (rect.x as usize)..x_end {
            let off = row_base + x * pix_size;
            if off + pix_size > pixels.len() {
                continue;
            }
            pixels[off..off + pix_size].copy_from_slice(&template[..pix_size]);
        }
    }
}

fn pixel_template(components: u8, fill_color: Color) -> [u8; 4] {
    let r = fill_color.r;
    let g = fill_color.g;
    let b = fill_color.b;
    match components {
        1 => {
            // ITU-R BT.601 luminance.
            let y = (0.299 * f64::from(r) + 0.587 * f64::from(g) + 0.114 * f64::from(b))
                .round() as u8;
            [y, 0, 0, 0]
        }
        3 => [r, g, b, 0],
        4 => {
            // Naive RGB → CMYK (no ICC). For default black fill (0,0,0)
            // this gives C=M=Y=0, K=255, which renders as ink black.
            let rf = f64::from(r) / 255.0;
            let gf = f64::from(g) / 255.0;
            let bf = f64::from(b) / 255.0;
            let k = 1.0 - rf.max(gf).max(bf);
            let denom = (1.0 - k).max(1e-9);
            let c = (1.0 - rf - k) / denom;
            let m = (1.0 - gf - k) / denom;
            let yy = (1.0 - bf - k) / denom;
            [
                (c.clamp(0.0, 1.0) * 255.0).round() as u8,
                (m.clamp(0.0, 1.0) * 255.0).round() as u8,
                (yy.clamp(0.0, 1.0) * 255.0).round() as u8,
                (k.clamp(0.0, 1.0) * 255.0).round() as u8,
            ]
        }
        _ => [0, 0, 0, 0],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paint_mask_rect_writes_only_within_rect() {
        // 8x8 RGB image initialized to (255, 255, 255). Mask the rect
        // (2, 2, 4, 4) → bytes inside that rect become (0, 0, 0); bytes
        // outside stay (255, 255, 255).
        let mut pixels = vec![255u8; 8 * 8 * 3];
        paint_mask_rect(
            &mut pixels,
            8,
            3,
            ImagePixelRect {
                x: 2,
                y: 2,
                w: 4,
                h: 4,
            },
            Color { r: 0, g: 0, b: 0 },
        );
        for y in 0..8 {
            for x in 0..8 {
                let off = (y * 8 + x) * 3;
                let inside = (2..6).contains(&x) && (2..6).contains(&y);
                let expected = if inside { 0u8 } else { 255u8 };
                assert_eq!(
                    pixels[off], expected,
                    "pixel ({x}, {y}) R channel"
                );
                assert_eq!(pixels[off + 1], expected);
                assert_eq!(pixels[off + 2], expected);
            }
        }
    }

    #[test]
    fn pixel_template_default_black_for_each_color_space() {
        let black = Color::BLACK;
        // Gray: luminance 0.
        assert_eq!(pixel_template(1, black), [0, 0, 0, 0]);
        // RGB: (0, 0, 0).
        assert_eq!(pixel_template(3, black), [0, 0, 0, 0]);
        // CMYK: (0, 0, 0, 255) — pure ink black.
        assert_eq!(pixel_template(4, black), [0, 0, 0, 255]);
    }
}
