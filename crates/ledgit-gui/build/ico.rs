//! Turn the icon SVG into a Windows `.ico`.
//!
//! Shared by `build.rs`, which embeds the result in the exe, and by the
//! crate's tests, which check it - so the part that can be verified off
//! Windows is.
//!
//! Every size is rendered from the SVG at that size rather than scaled down
//! from one large bitmap, so the 16px taskbar-list icon is drawn for 16px.
//! Each image is stored as PNG, which Windows has read inside `.ico` files
//! since Vista.

use resvg::{tiny_skia, usvg};

/// The sizes Windows asks for: list views and title bars at 16-32, the
/// taskbar and Explorer tiles at 32-64, large icon views up to 256.
pub const SIZES: [u32; 7] = [16, 24, 32, 48, 64, 128, 256];

/// One PNG of the SVG, fitted and centred into a `size`-pixel square.
pub fn png(svg: &[u8], size: u32) -> Vec<u8> {
    let tree = usvg::Tree::from_data(svg, &usvg::Options::default()).expect("icon SVG parses");
    let s = tree.size();
    let scale = size as f32 / s.width().max(s.height());
    let dx = (size as f32 - s.width() * scale) / 2.0;
    let dy = (size as f32 - s.height() * scale) / 2.0;
    let mut pixmap = tiny_skia::Pixmap::new(size, size).expect("non-zero size");
    let t = tiny_skia::Transform::from_scale(scale, scale).post_translate(dx, dy);
    resvg::render(&tree, t, &mut pixmap.as_mut());
    pixmap.encode_png().expect("PNG encodes")
}

/// A complete `.ico` file holding every size in [`SIZES`].
pub fn ico(svg: &[u8]) -> Vec<u8> {
    let images: Vec<(u32, Vec<u8>)> = SIZES.iter().map(|s| (*s, png(svg, *s))).collect();
    const HEADER: usize = 6;
    const ENTRY: usize = 16;
    let mut offset = HEADER + ENTRY * images.len();

    let mut out = Vec::new();
    // ICONDIR: reserved, type 1 = icon, image count.
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&(images.len() as u16).to_le_bytes());
    for (size, data) in &images {
        // ICONDIRENTRY. A dimension of 256 does not fit a byte; 0 means 256.
        let dim = if *size >= 256 { 0 } else { *size as u8 };
        out.extend_from_slice(&[dim, dim, 0, 0]); // width, height, palette, reserved
        out.extend_from_slice(&1u16.to_le_bytes()); // colour planes
        out.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += data.len();
    }
    for (_, data) in &images {
        out.extend_from_slice(data);
    }
    out
}
