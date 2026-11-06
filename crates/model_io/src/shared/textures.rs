//! Cooking pictures: how big they may stay, and the files they are sent as.
//!
//! Opaque pictures go as JPEG (a fraction of a PNG's size), pictures with see-through pixels as
//! PNG, which keeps their alpha.

use std::borrow::Cow;

use crate::image::Image;

/// The JPEG quality a picture starts at, and the one its canonical copy on a member's side has.
pub(super) const BEST_QUALITY: u8 = 90;

/// The worst JPEG quality we cook with.
pub(super) const WORST_QUALITY: u8 = 60;

/// A picture as it is sent.
pub(super) struct Encoded {
    /// A PNG or JPEG file.
    pub file: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// Whether it was made smaller or worse than the original.
    pub lowered: bool,
}

/// `value` times `factor`, rounded down, at least 1 (a hair over, so that a longest side scaled by
/// "side over side" doesn't lose a pixel to rounding).
fn scaled(value: u32, factor: f64) -> u32 {
    if factor >= 1.0 {
        return value;
    }
    ((f64::from(value) * factor + 1e-6).floor() as u32).max(1)
}

/// The sizes the `pictures` (width, height) are given so that each side is at most `side`, no
/// picture is over `scale` times its own size, and all together they are at most `pixels`, shrunk
/// in proportion. `None` if not even one pixel each fits.
pub(super) fn sizes_within(
    pictures: &[(u32, u32)],
    scale: f64,
    side: u32,
    pixels: u64,
) -> Option<Vec<(u32, u32)>> {
    let own: Vec<f64> = pictures
        .iter()
        .map(|&(width, height)| {
            let longest = f64::from(width.max(height).max(1));
            scale.min(f64::from(side) / longest)
        })
        .collect();
    let mut shrink = 1.0f64;
    for _ in 0..64 {
        let sizes: Vec<(u32, u32)> = pictures
            .iter()
            .zip(&own)
            .map(|(&(width, height), own)| {
                (scaled(width, own * shrink), scaled(height, own * shrink))
            })
            .collect();
        let total: u64 = sizes
            .iter()
            .map(|&(w, h)| u64::from(w) * u64::from(h))
            .sum();
        if total <= pixels {
            return Some(sizes);
        }
        // The pixels follow the square of the factor; a little more, for the rounding.
        shrink *= (pixels as f64 / total as f64).sqrt().min(0.98);
    }
    None
}

/// `image` at `width` x `height` (no bigger than its own size). The original if the size is its own.
pub(super) fn shrunk(image: &Image, width: u32, height: u32) -> Cow<'_, Image> {
    if (width, height) == (image.width, image.height) {
        Cow::Borrowed(image)
    } else {
        Cow::Owned(image.scaled_down(width, height))
    }
}

/// `picture` as a file: a JPEG of `quality` if it is opaque, else a PNG. `resized` says it is
/// smaller than the original it comes from.
pub(super) fn encode(picture: &Image, quality: u8, resized: bool) -> Result<Encoded, String> {
    let opaque = picture.is_opaque();
    let file = if opaque {
        picture.to_jpeg(quality)?
    } else {
        picture.to_small_png()?
    };
    Ok(Encoded {
        file,
        width: picture.width,
        height: picture.height,
        lowered: resized || (opaque && quality < BEST_QUALITY),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_follow_side_scale_and_pixels() {
        // Within everything: unchanged.
        assert_eq!(
            sizes_within(&[(100, 50)], 1.0, 4096, 1 << 20),
            Some(vec![(100, 50)])
        );
        // The side.
        assert_eq!(
            sizes_within(&[(400, 100)], 1.0, 200, 1 << 20),
            Some(vec![(200, 50)])
        );
        // The scale.
        assert_eq!(
            sizes_within(&[(400, 100)], 0.5, 4096, 1 << 20),
            Some(vec![(200, 50)])
        );
        // Pixels shared in proportion: 100x100 and 200x200 into 10,000 pixels.
        let sizes = sizes_within(&[(100, 100), (200, 200)], 1.0, 4096, 10_000).unwrap();
        let total: u64 = sizes.iter().map(|&(w, h)| u64::from(w * h)).sum();
        assert!(total <= 10_000 && total > 9_000, "{sizes:?}");
        assert!(sizes[1].0.abs_diff(sizes[0].0 * 2) <= 1, "{sizes:?}");
        // Not a pixel each.
        assert_eq!(sizes_within(&[(10, 10), (10, 10)], 1.0, 100, 1), None);
    }
}
