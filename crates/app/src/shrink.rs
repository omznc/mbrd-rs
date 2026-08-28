//! Re-encoding one picture, smaller.
//!
//! The half with the decoder in it. What to try and what to leave alone is
//! [`mbrd_core::shrink`]'s, which is testable without a window and holds the
//! judgement; this is the four calls around `image` that turn a job into bytes,
//! and the three refusals only the bytes themselves can answer.
//!
//! ## Two formats out, and which one by what is in the picture
//!
//! JPEG for anything opaque, PNG for anything that is not. That is not a
//! preference, it is the whole choice available: `image` 0.25 dropped its WebP
//! *encoder* — the decoder stayed — and every other small format it can write
//! is either lossless or a niche nobody's other tools open. A photograph at
//! quality 82 is most of what WebP would have saved anyway; a screenshot with a
//! transparent corner would be ruined by it and goes out as PNG, where the
//! saving comes from the resize rather than from the encoder.
//!
//! Alpha is decided by **looking**, on the resized picture rather than the
//! original: a great many PNGs are stored as RGBA and are opaque in every
//! pixel, and taking the format's word for it would leave the commonest heavy
//! file on a board — a screenshot — as a PNG forever. The scan costs one pass
//! over an image that is already at most 1200 pixels on its long edge.
//!
//! ## Nothing here decides anything
//!
//! Every refusal is `None`, and the caller counts a `None` as "left alone" —
//! which is what it is. An animation, a file this build cannot decode, a
//! re-encode that came out bigger: none of those is a failure, and treating any
//! of them as one would turn a page that says "12 pictures could be smaller"
//! into a page that reports errors about files that are perfectly fine.

use image::ImageEncoder;

use mbrd_core::shrink::QUALITY;

/// A re-encoded picture, and the extension it should be stored under.
pub struct Made {
    pub bytes: Vec<u8>,
    /// `jpg` or `png`. A `&'static str` because there are exactly two, and the
    /// asset store wants an owned `String` it can build from either.
    pub ext: &'static str,
}

/// A smaller version of this picture, or `None` to leave it alone.
///
/// `edge` is the longest side it may keep — see `mbrd_core::shrink::LONG_EDGE`.
/// The picture is never enlarged: a card drawn bigger than its own file is a
/// card that looks soft, and re-sampling it up would make it a soft card that
/// also weighs more.
///
/// The `worth_it` test is **not** made here. This answers "here is a smaller
/// file"; whether a saving that small is worth a generation of quality is the
/// caller's question, and the one place it is asked is
/// `mbrd_core::shrink::worth_it`.
pub fn squeeze(bytes: &[u8], edge: u32) -> Option<Made> {
    if moves(bytes) {
        return None;
    }
    // Guessed from the bytes rather than from the extension the card carries.
    // A file named `.jpg` that is really a PNG is a thing that exists on every
    // disk in the world, and the decoder is the only honest reader of which it
    // is.
    let decoded = image::load_from_memory(bytes).ok()?;
    let (w, h) = (decoded.width(), decoded.height());
    if w == 0 || h == 0 {
        return None;
    }

    // `resize` keeps the aspect ratio and fits *inside* the box, so both sides
    // can be handed the same number — which is what "the longest edge" means.
    let decoded = match w.max(h) > edge {
        true => decoded.resize(edge, edge, image::imageops::FilterType::Lanczos3),
        false => decoded,
    };

    match see_through(&decoded) {
        true => {
            let rgba = decoded.to_rgba8();
            let mut out = Vec::new();
            image::codecs::png::PngEncoder::new_with_quality(
                &mut out,
                image::codecs::png::CompressionType::Best,
                image::codecs::png::FilterType::Adaptive,
            )
            .write_image(
                rgba.as_raw(),
                rgba.width(),
                rgba.height(),
                image::ExtendedColorType::Rgba8,
            )
            .ok()?;
            Some(Made { bytes: out, ext: "png" })
        }
        false => {
            let rgb = decoded.to_rgb8();
            let mut out = Vec::new();
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, QUALITY)
                .encode(rgb.as_raw(), rgb.width(), rgb.height(), image::ExtendedColorType::Rgb8)
                .ok()?;
            Some(Made { bytes: out, ext: "jpg" })
        }
    }
}

/// Whether this file animates.
///
/// Asked of the decoders rather than of the extension, because the two formats
/// that can move here can also sit perfectly still: an APNG is a `.png` and an
/// animated WebP is a `.webp`, and there is no way to tell from the name. A
/// still frame of either would be a picture that has stopped moving, which is
/// not an optimisation — see the module note in `mbrd_core::shrink`.
///
/// The same question `images::moving` asks on the way to drawing one, and asked
/// the same way; it is two lines rather than a shared helper because that one
/// wants the frames and this one wants only the answer.
fn moves(bytes: &[u8]) -> bool {
    use image::codecs::{png::PngDecoder, webp::WebPDecoder};

    let at = || std::io::Cursor::new(bytes);
    let format = image::guess_format(bytes).ok();
    match format {
        // Not offered in the first place — see `shrink::shrinkable` — but
        // answered here too, because a file named `.png` that is really a GIF
        // reaches this function and must not come back as one frame.
        Some(image::ImageFormat::Gif) => true,
        Some(image::ImageFormat::WebP) => {
            WebPDecoder::new(at()).map(|decoder| decoder.has_animation()).unwrap_or(true)
        }
        Some(image::ImageFormat::Png) => {
            PngDecoder::new(at()).and_then(|decoder| decoder.is_apng()).unwrap_or(true)
        }
        // A format that cannot move, or one nothing here can read. The decode
        // below is what refuses the second, which is the right place for it:
        // this function's answer is about movement, not about readability.
        _ => false,
    }
}

/// Whether any pixel is less than fully opaque.
///
/// The format's own word is the fast half — a picture with no alpha channel
/// cannot have a transparent pixel — and the scan is the half that matters,
/// for the reason the module note gives.
fn see_through(image: &image::DynamicImage) -> bool {
    if !image.color().has_alpha() {
        return false;
    }
    match image {
        // The two that carry alpha at eight bits, which is every picture that
        // reaches here from a file somebody dropped on a board.
        image::DynamicImage::ImageRgba8(rgba) => rgba.pixels().any(|p| p.0[3] < u8::MAX),
        image::DynamicImage::ImageLumaA8(la) => la.pixels().any(|p| p.0[1] < u8::MAX),
        // Sixteen-bit and floating point, which arrive from TIFF and EXR. Rare
        // enough that converting once and scanning the copy is cheaper than a
        // third and fourth spelling of the same loop.
        other => other.to_rgba8().pixels().any(|p| p.0[3] < u8::MAX),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageFormat, Rgba, RgbaImage};

    /// A picture with something in it, so the JPEG has something to compress.
    fn photo(w: u32, h: u32, alpha: u8) -> RgbaImage {
        RgbaImage::from_fn(w, h, |x, y| {
            Rgba([(x % 256) as u8, (y % 256) as u8, ((x + y) % 256) as u8, alpha])
        })
    }

    fn written(image: RgbaImage, format: ImageFormat) -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image).write_to(&mut out, format).unwrap();
        out.into_inner()
    }

    #[test]
    fn a_picture_past_the_ceiling_comes_back_inside_it() {
        let bytes = written(photo(2000, 1000, 255), ImageFormat::Png);
        let made = squeeze(&bytes, 500).expect("a 2000px picture is shrinkable");
        let back = image::load_from_memory(&made.bytes).unwrap();
        assert_eq!(back.width(), 500);
        assert_eq!(back.height(), 250, "the shape is kept");
    }

    /// Never enlarged: a soft card that also weighs more is the worst of both.
    #[test]
    fn a_picture_already_inside_the_ceiling_keeps_its_size() {
        let bytes = written(photo(100, 80, 255), ImageFormat::Png);
        let made = squeeze(&bytes, 1200).expect("a small picture still re-encodes");
        let back = image::load_from_memory(&made.bytes).unwrap();
        assert_eq!((back.width(), back.height()), (100, 80));
    }

    #[test]
    fn an_opaque_picture_goes_out_as_a_photograph_and_a_transparent_one_does_not() {
        let opaque = written(photo(200, 200, 255), ImageFormat::Png);
        assert_eq!(squeeze(&opaque, 1200).unwrap().ext, "jpg");

        let ghostly = written(photo(200, 200, 128), ImageFormat::Png);
        assert_eq!(squeeze(&ghostly, 1200).unwrap().ext, "png", "alpha would be thrown away");
    }

    /// The load-bearing one: an RGBA picture that happens to be opaque in
    /// every pixel is a photograph, and taking the format's word for it would
    /// leave every screenshot on every board a PNG forever.
    #[test]
    fn transparency_is_decided_by_looking_and_not_by_the_channel_count() {
        let rgba_but_opaque = image::DynamicImage::ImageRgba8(photo(20, 20, 255));
        assert!(!see_through(&rgba_but_opaque));
        assert!(see_through(&image::DynamicImage::ImageRgba8(photo(20, 20, 254))));
    }

    #[test]
    fn something_that_is_not_a_picture_at_all_is_left_alone() {
        assert!(squeeze(b"this is a note, not a photograph", 1200).is_none());
        assert!(squeeze(&[], 1200).is_none());
    }
}
