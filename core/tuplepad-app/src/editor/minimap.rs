//! The minimap: the document in miniature at the editor's right edge, two pixels a line and one
//! a character, in the theme's syntax colors. It's one image, drawn again only when the text,
//! colors or the window of lines it shows change. Long documents show the stretch around the
//! view, moving with the scroll position.

use std::hash::{Hash, Hasher};

use slint::{Color, Image, Rgba8Pixel, SharedPixelBuffer};
use tuplepad_core::TextBuffer;

use super::geometry::LONG_LINE;
use super::style_index;

/// Logical width of the minimap, and of the strip beside it for the scrollbar.
pub const WIDTH: f32 = 92.0;
pub const SCROLLBAR: f32 = 14.0;
/// Logical height of a line, and width of a character cell.
pub const LINE: f32 = 2.0;
const CELL: f32 = 1.0;
/// Characters drawn per line.
const COLUMNS: u64 = 120;

#[derive(Default)]
pub struct Minimap {
    key: Option<u64>,
    pub image: Image,
}

pub struct Request<'a> {
    pub buffer: &'a TextBuffer,
    pub doc: &'a str,
    pub first_line: u64,
    pub lines: u64,
    pub height: f32,
    pub scale: f32,
    pub tab_width: usize,
    pub colors: &'a [Color],
    pub dark: bool,
}

impl Minimap {
    /// Draws the image again if anything it shows has changed. Returns whether it did.
    pub fn update(&mut self, r: &Request) -> bool {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        (r.doc, r.buffer.version(), r.first_line, r.lines, r.height.to_bits(), r.scale.to_bits(), r.tab_width, r.dark).hash(&mut hasher);
        for color in r.colors {
            (color.red(), color.green(), color.blue()).hash(&mut hasher);
        }
        let key = hasher.finish();
        if self.key == Some(key) {
            return false;
        }
        self.key = Some(key);
        self.image = render(r);
        true
    }
}

fn render(r: &Request) -> Image {
    let width = (WIDTH * r.scale).round().max(1.0) as u32;
    let height = (r.height * r.scale).round().max(1.0) as u32;
    let mut pixels = SharedPixelBuffer::<Rgba8Pixel>::new(width, height);
    let line_count = r.buffer.line_count();
    let count = r.lines.min(line_count.saturating_sub(r.first_line));
    if count == 0 {
        return Image::from_rgba8_premultiplied(pixels);
    }
    let lines: Vec<u64> = (r.first_line..r.first_line + count).collect();
    // Colors for each line: one query for ordinary lines, the start only of very long ones.
    let mut spans: Vec<Vec<(u64, u64, i32)>> = vec![Vec::new(); lines.len()];
    let long: Vec<bool> = lines.iter().map(|&l| r.buffer.line_length(l) as usize > LONG_LINE).collect();
    let mut add = |found: Vec<tuplepad_core::HighlightSpan>| {
        for span in found {
            if let Some(list) = span.line.checked_sub(r.first_line).and_then(|i| spans.get_mut(i as usize)) {
                list.push((span.start, span.end, style_index(span.style)));
            }
        }
    };
    if long.iter().any(|&l| l) {
        for (i, &line) in lines.iter().enumerate() {
            add(if long[i] { r.buffer.highlights_in_line(line, 0, COLUMNS * 2) } else { r.buffer.highlights(line, 1) });
        }
    } else {
        add(r.buffer.highlights(r.first_line, count));
    }
    let alpha = if r.dark { 0.62 } else { 0.55 };
    let line_px = LINE * r.scale;
    let ink_px = (line_px * 0.7).round().max(1.0) as u32;
    let cell_px = CELL * r.scale;
    let buffer = pixels.make_mut_slice();
    for (i, &line) in lines.iter().enumerate() {
        let y0 = (i as f32 * line_px).round() as u32;
        if y0 >= height {
            break;
        }
        let text = r.buffer.line_slice(line, 0, COLUMNS * 2);
        let mut cell = 0usize;
        let mut column = 0u64;
        for ch in text.chars() {
            let cells = if ch == '\t' { r.tab_width - cell % r.tab_width.max(1) } else { 1 };
            if !ch.is_whitespace() {
                let style = spans[i].iter().find(|&&(s, e, _)| s <= column && column < e).map_or(0, |&(_, _, style)| style);
                let color = r.colors.get(style as usize).or(r.colors.first()).copied().unwrap_or(Color::from_rgb_u8(128, 128, 128));
                let x0 = (cell as f32 * cell_px).round() as u32;
                let x1 = (((cell + cells) as f32) * cell_px).round() as u32;
                fill(buffer, width, height, x0, x1, y0, y0 + ink_px, color, alpha);
            }
            cell += cells;
            column += ch.len_utf16() as u64;
            if cell as f32 * cell_px >= width as f32 {
                break;
            }
        }
    }
    Image::from_rgba8_premultiplied(pixels)
}

#[allow(clippy::too_many_arguments)]
fn fill(buffer: &mut [Rgba8Pixel], width: u32, height: u32, x0: u32, x1: u32, y0: u32, y1: u32, color: Color, alpha: f32) {
    let a = (alpha * 255.0) as u8;
    let premultiply = |c: u8| ((c as f32) * alpha) as u8;
    let pixel = Rgba8Pixel { r: premultiply(color.red()), g: premultiply(color.green()), b: premultiply(color.blue()), a };
    for y in y0..y1.min(height) {
        let row = (y * width) as usize;
        for x in x0..x1.min(width) {
            buffer[row + x as usize] = pixel;
        }
    }
}

/// The first line the minimap shows, so that it follows the scroll position through documents
/// too long to fit.
pub fn first_line(line_count: u64, fits: u64, scroll_fraction: f64) -> u64 {
    if line_count <= fits {
        0
    } else {
        ((line_count - fits) as f64 * scroll_fraction.clamp(0.0, 1.0)).round() as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follows_the_scroll_position() {
        assert_eq!(first_line(100, 300, 0.5), 0);
        assert_eq!(first_line(1300, 300, 0.0), 0);
        assert_eq!(first_line(1300, 300, 0.5), 500);
        assert_eq!(first_line(1300, 300, 1.0), 1000);
    }

    #[test]
    fn draws_characters_in_their_colors() {
        let buffer = TextBuffer::with_text("ab c\n\tx".into());
        let colors = vec![Color::from_rgb_u8(255, 0, 0)];
        let request = Request { buffer: &buffer, doc: "d", first_line: 0, lines: 10, height: 20.0, scale: 1.0, tab_width: 4, colors: &colors, dark: false };
        let image = render(&request);
        let pixels = image.to_rgba8_premultiplied().unwrap();
        let at = |x: u32, y: u32| pixels.as_slice()[(y * pixels.width() + x) as usize].a;
        // "ab c": cells 0, 1 and 3 are ink, 2 is a space.
        assert!(at(0, 0) > 0 && at(1, 0) > 0 && at(3, 0) > 0);
        assert_eq!(at(2, 0), 0);
        // "\tx": the tab takes four cells, then x.
        assert_eq!(at(0, 2), 0);
        assert!(at(4, 2) > 0);
    }
}
