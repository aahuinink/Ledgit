//! The logo and the app icon, drawn from the SVGs in `assets/`.
//!
//! Each comes in two copies: the original, black on a light surface, and a
//! `_dark` copy with its colours inverted for a dark one. Inside the app the
//! copy follows the app's theme. The window icon does not: it is whichever
//! file `build.rs` chose for the exe, so the title bar, the taskbar and the
//! pinned shortcut always show the same thing. All four files are
//! produced from the Canva export by `assets/rebuild.py`; edit the design
//! there, not by hand. All four are
//! compiled into the binary and rasterised once at start-up, so the SVGs stay
//! the only copy of the artwork - there is no exported PNG to fall out of date
//! when the design changes.

use resvg::{tiny_skia, usvg};

const ICON: &[u8] = include_bytes!("../../../assets/Icon.svg");
const ICON_DARK: &[u8] = include_bytes!("../../../assets/Icon_dark.svg");
const LOGO: &[u8] = include_bytes!("../../../assets/Ledgit_logo.svg");
const LOGO_DARK: &[u8] = include_bytes!("../../../assets/Ledgit_logo_dark.svg");
/// The app icon `build.rs` embeds in the exe; see `ICON` there to change it.
const APP_ICON: &[u8] = include_bytes!(env!("LEDGIT_APP_ICON"));

/// An RGBA image, straight (not premultiplied) alpha.
pub struct Raster {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// The window and taskbar icon: the same artwork as the exe's.
///
/// It deliberately ignores the theme. egui reports Windows' *app* mode, but
/// the taskbar follows the separate *Windows* mode; with light apps on a dark
/// taskbar, following the app mode put the light icon on the dark taskbar.
pub fn window_icon() -> egui::IconData {
    let mut pixmap = tiny_skia::Pixmap::new(256, 256).expect("non-zero size");
    draw(&mut pixmap, APP_ICON);
    let r = into_raster(&pixmap);
    egui::IconData { rgba: r.rgba, width: r.width, height: r.height }
}

/// The icon, `size` pixels square, on a transparent background.
pub fn icon(size: u32, dark: bool) -> Raster {
    let mut pixmap = tiny_skia::Pixmap::new(size, size).expect("non-zero size");
    draw(&mut pixmap, if dark { ICON_DARK } else { ICON });
    into_raster(&pixmap)
}

/// The full logo, cropped to its artwork plus a small margin, `width` pixels
/// wide before cropping. The SVG canvas is square with a lot of air above and
/// below the wordmark; cropping lets it sit in a layout at its real shape.
pub fn logo(width: u32, dark: bool) -> Raster {
    let mut pixmap = tiny_skia::Pixmap::new(width, width).expect("non-zero size");
    draw(&mut pixmap, if dark { LOGO_DARK } else { LOGO });
    crop_to_content(&pixmap, width / 32)
}

fn texture(ctx: &egui::Context, name: &str, r: &Raster) -> egui::TextureHandle {
    let size = [r.width as usize, r.height as usize];
    let image = egui::ColorImage::from_rgba_unmultiplied(size, &r.rgba);
    ctx.load_texture(name, image, egui::TextureOptions::LINEAR)
}

/// The textures the UI draws, both copies of each, made once per run.
pub struct Brand {
    logo: [egui::TextureHandle; 2],
    mark: [egui::TextureHandle; 2],
}

impl Brand {
    pub fn load(ctx: &egui::Context) -> Brand {
        // Rasterised well above display size, so they stay sharp when zoomed.
        Brand {
            logo: [false, true].map(|d| texture(ctx, &format!("logo_{d}"), &logo(640, d))),
            mark: [false, true].map(|d| texture(ctx, &format!("mark_{d}"), &icon(96, d))),
        }
    }

    /// The logo for the theme `ui` is drawing in.
    pub fn logo(&self, ui: &egui::Ui) -> &egui::TextureHandle {
        &self.logo[ui.visuals().dark_mode as usize]
    }

    /// The icon, for small places like the top bar.
    pub fn mark(&self, ui: &egui::Ui) -> &egui::TextureHandle {
        &self.mark[ui.visuals().dark_mode as usize]
    }
}

/// Render `svg` fitted and centred into the pixmap.
fn draw(pixmap: &mut tiny_skia::Pixmap, svg: &[u8]) {
    let tree = usvg::Tree::from_data(svg, &usvg::Options::default()).expect("bundled SVG parses");
    let size = tree.size();
    let (w, h) = (pixmap.width() as f32, pixmap.height() as f32);
    let scale = (w / size.width()).min(h / size.height());
    let dx = (w - size.width() * scale) / 2.0;
    let dy = (h - size.height() * scale) / 2.0;
    let t = tiny_skia::Transform::from_scale(scale, scale).post_translate(dx, dy);
    resvg::render(&tree, t, &mut pixmap.as_mut());
}

/// tiny-skia keeps premultiplied alpha; egui and window icons want straight.
fn into_raster(pixmap: &tiny_skia::Pixmap) -> Raster {
    let rgba = pixmap
        .pixels()
        .iter()
        .flat_map(|p| {
            let c = p.demultiply();
            [c.red(), c.green(), c.blue(), c.alpha()]
        })
        .collect();
    Raster { width: pixmap.width(), height: pixmap.height(), rgba }
}

/// Trim transparent rows and columns, leaving `pad` pixels of air.
fn crop_to_content(pixmap: &tiny_skia::Pixmap, pad: u32) -> Raster {
    let (w, h) = (pixmap.width(), pixmap.height());
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
    for y in 0..h {
        for x in 0..w {
            if pixmap.pixel(x, y).is_some_and(|p| p.alpha() > 8) {
                (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
            }
        }
    }
    if x0 > x1 {
        return into_raster(pixmap);
    }
    let (x0, y0) = (x0.saturating_sub(pad), y0.saturating_sub(pad));
    let (x1, y1) = ((x1 + pad).min(w - 1), (y1 + pad).min(h - 1));
    let rect = tiny_skia::IntRect::from_ltrb(x0 as i32, y0 as i32, x1 as i32 + 1, y1 as i32 + 1)
        .expect("non-empty crop");
    into_raster(&pixmap.clone_rect(rect).expect("crop lies inside the image"))
}

/// The `.ico` writer from `build.rs`, compiled here so its output is tested.
#[cfg(test)]
#[path = "../build/ico.rs"]
mod ico;

#[cfg(test)]
mod tests {
    use super::*;

    /// Mean luminance of the opaque pixels: the ink the artwork is drawn in.
    fn ink(r: &Raster) -> f32 {
        let (sum, n) = r
            .rgba
            .chunks(4)
            .filter(|p| p[3] == 255)
            .fold((0u64, 0u64), |(s, n), p| (s + p[0] as u64, n + 1));
        sum as f32 / n.max(1) as f32
    }

    #[test]
    fn every_bundled_svg_renders() {
        let i = window_icon();
        assert_eq!((i.width, i.height), (256, 256));
        assert_eq!(i.rgba[3], 0, "the icon's background is transparent");
        for dark in [false, true] {
            let l = logo(640, dark);
            assert!(
                l.width > l.height,
                "cropped to the wordmark's shape: {}x{}",
                l.width,
                l.height
            );
        }
    }

    /// The dark copies are the originals inverted, so their ink is light.
    #[test]
    fn the_dark_copies_are_inverted() {
        let (light, dark) = (icon(128, false), icon(128, true));
        // The originals are mostly white-filled circles with black strokes,
        // so compare the two readings rather than pin a threshold on either.
        assert!(
            (ink(&light) + ink(&dark) - 255.0).abs() < 40.0,
            "{} vs {}",
            ink(&light),
            ink(&dark)
        );
        assert!(logo(320, true).rgba.chunks(4).any(|p| p[3] == 255 && p[0] > 230));
    }

    /// The exe icon: a well-formed `.ico` whose directory points at a PNG of
    /// the right size for every entry.
    #[test]
    fn the_exe_icon_is_a_valid_ico() {
        let data = ico::ico(APP_ICON);
        let u16_at = |i: usize| u16::from_le_bytes([data[i], data[i + 1]]) as usize;
        let u32_at = |i: usize| u32::from_le_bytes(data[i..i + 4].try_into().unwrap()) as usize;
        assert_eq!((u16_at(0), u16_at(2), u16_at(4)), (0, 1, ico::SIZES.len()));
        for (k, size) in ico::SIZES.iter().enumerate() {
            let e = 6 + 16 * k;
            let dim = if *size == 256 { 0 } else { *size as u8 };
            assert_eq!((data[e], data[e + 1]), (dim, dim), "entry {k}");
            assert_eq!(u16_at(e + 6), 32, "32 bits per pixel");
            let (len, at) = (u32_at(e + 8), u32_at(e + 12));
            let png = &data[at..at + len];
            assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
            // IHDR width and height, big-endian, right after the signature.
            let w = u32::from_be_bytes(png[16..20].try_into().unwrap());
            let h = u32::from_be_bytes(png[20..24].try_into().unwrap());
            assert_eq!((w, h), (*size, *size));
        }
        let last = 6 + 16 * (ico::SIZES.len() - 1);
        assert_eq!(u32_at(last + 12) + u32_at(last + 8), data.len(), "no trailing bytes");
    }
}

/// The window icon is the exe's icon, not a copy picked by theme.
#[cfg(test)]
#[test]
fn the_window_icon_is_the_exe_icon() {
    let chosen = std::fs::read(env!("LEDGIT_APP_ICON")).unwrap();
    assert_eq!(APP_ICON, chosen.as_slice());
    assert!(APP_ICON == ICON || APP_ICON == ICON_DARK, "build.rs names one of the two copies");
}
