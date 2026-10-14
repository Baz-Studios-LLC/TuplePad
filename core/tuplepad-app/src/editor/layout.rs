//! Lays out editor lines with parley over Slint's own font collection, so the positions computed
//! here (carets, selections, where each colored run starts) match what Slint draws.

use std::borrow::Cow;
use std::collections::HashMap;
use std::rc::Rc;

use parley::fontique::GenericFamily;
use parley::style::{FontFamily, FontFamilyName, FontStyle, FontWeight, StyleProperty, TextWrapMode};
use parley::{FontContext, Layout, LayoutContext, PositionedLayoutItem};

/// Lines longer than this are cut short on screen (the rest isn't laid out).
pub const MAX_LAYOUT_COLUMNS: usize = 16_384;

/// Where every UTF-16 column of a line sits.
pub struct LineLayout {
    /// The x of each column boundary (one more entry than the line has columns), in logical pixels.
    pub col_x: Vec<f32>,
    /// Whether a caret may stop at a column (the start of a cluster, or the end of the line).
    pub stop: Vec<bool>,
    pub width: f32,
}

impl LineLayout {
    pub fn x(&self, col: usize) -> f32 {
        match self.col_x.get(col) {
            Some(x) => *x,
            None => self.width,
        }
    }

    /// The caret stop nearest to `x`.
    pub fn column_at(&self, x: f32) -> usize {
        let last = self.col_x.len() - 1;
        if x <= 0.0 {
            return 0;
        }
        if x >= self.width {
            return last;
        }
        // The first boundary past x, then whichever stop on either side is closer.
        let after = self.col_x.partition_point(|&cx| cx <= x).min(last);
        let mut right = after;
        while right < last && !self.stop[right] {
            right += 1;
        }
        let mut left = after.saturating_sub(1);
        while left > 0 && !self.stop[left] {
            left -= 1;
        }
        if (x - self.col_x[left]).abs() <= (self.col_x[right] - x).abs() { left } else { right }
    }
}

pub struct Layouter {
    fonts: FontContext,
    layouts: LayoutContext<()>,
    family: String,
    size: f32,
    scale: f32,
    cache: HashMap<String, Rc<LineLayout>>,
}

impl Layouter {
    pub fn new(family: &str, size: f32, scale: f32) -> Self {
        let fonts = FontContext {
            collection: slint::fontique_011::shared_collection(),
            source_cache: Default::default(),
        };
        Layouter { fonts, layouts: LayoutContext::new(), family: family.to_string(), size, scale, cache: HashMap::new() }
    }

    pub fn configure(&mut self, family: &str, size: f32, scale: f32) {
        if family != self.family || size != self.size || scale != self.scale {
            self.family = family.to_string();
            self.size = size;
            self.scale = scale;
            self.cache.clear();
        }
    }

    /// Whether the collection knows a family by this name.
    pub fn has_family(&mut self, name: &str) -> bool {
        self.fonts.collection.family_by_name(name).is_some()
    }

    /// The installed font families whose letters are all one width, by name (hidden system
    /// families left out).
    pub fn monospace_families(&mut self) -> Vec<String> {
        let names: Vec<String> = self.fonts.collection.family_names().filter(|n| !n.starts_with('.')).map(str::to_string).collect();
        let mut families: Vec<String> = names
            .into_iter()
            .filter(|name| {
                let narrow = self.measure_in(name, "iiiiiiii");
                let wide = self.measure_in(name, "MMMMMMMM");
                narrow > 1.0 && (narrow - wide).abs() < 0.5
            })
            .collect();
        families.sort_by_key(|n| n.to_lowercase());
        families.dedup();
        families
    }

    /// The width of text in one family (no fallbacks), for comparing families.
    fn measure_in(&mut self, family: &str, text: &str) -> f32 {
        let families = [FontFamilyName::named(family)];
        let mut builder = self.layouts.ranged_builder(&mut self.fonts, text, 1.0, false);
        builder.push_default(FontFamily::List(Cow::Borrowed(&families)));
        builder.push_default(StyleProperty::FontSize(16.0));
        let mut layout: Layout<()> = builder.build(text);
        layout.break_all_lines(None);
        layout.full_width()
    }

    /// The layout of one line (tabs already expanded), cached by its text.
    pub fn line(&mut self, text: &str) -> Rc<LineLayout> {
        if let Some(layout) = self.cache.get(text) {
            return layout.clone();
        }
        if self.cache.len() > 20_000 {
            self.cache.clear();
        }
        let layout = Rc::new(self.measure(text));
        self.cache.insert(text.to_string(), layout.clone());
        layout
    }

    /// The advance of a short string, such as one digit for the gutter.
    pub fn advance(&mut self, text: &str) -> f32 {
        self.line(text).width
    }

    fn measure(&mut self, text: &str) -> LineLayout {
        if text.is_empty() {
            return LineLayout { col_x: vec![0.0], stop: vec![true], width: 0.0 };
        }
        // Same style as Slint gives a Text element: the family first, then its fallbacks.
        let families = [
            FontFamilyName::named(self.family.as_str()),
            FontFamilyName::Generic(GenericFamily::SansSerif),
            FontFamilyName::Generic(GenericFamily::SystemUi),
        ];
        let mut builder = self.layouts.ranged_builder(&mut self.fonts, text, self.scale, false);
        builder.push_default(FontFamily::List(Cow::Borrowed(&families)));
        builder.push_default(StyleProperty::FontWeight(FontWeight::new(400.0)));
        builder.push_default(StyleProperty::FontStyle(FontStyle::Normal));
        builder.push_default(StyleProperty::FontSize(self.size));
        builder.push_default(StyleProperty::TextWrapMode(TextWrapMode::NoWrap));
        let mut layout: Layout<()> = builder.build(text);
        layout.break_all_lines(None);

        // Column of each byte offset that starts a character.
        let columns: usize = text.chars().map(char::len_utf16).sum();
        let mut col_x = vec![f32::NAN; columns + 1];
        let mut stop = vec![false; columns + 1];
        let mut byte_col = vec![usize::MAX; text.len() + 1];
        let mut col = 0;
        for (byte, ch) in text.char_indices() {
            byte_col[byte] = col;
            col += ch.len_utf16();
        }
        let scale = self.scale;
        let mut width: f32 = 0.0;
        for line in layout.lines() {
            for item in line.items() {
                let PositionedLayoutItem::GlyphRun(glyph_run) = item else { continue };
                let run = glyph_run.run();
                let mut x = glyph_run.offset();
                for cluster in run.visual_clusters() {
                    let col = byte_col.get(cluster.text_range().start).copied().unwrap_or(usize::MAX);
                    if col != usize::MAX {
                        col_x[col] = x / scale;
                        stop[col] = !cluster.is_ligature_continuation();
                    }
                    x += cluster.advance();
                }
                width = width.max(x / scale);
            }
        }
        width = width.max(layout.full_width() / scale);
        col_x[columns] = width;
        stop[columns] = true;
        stop[0] = true;
        // Columns inside a cluster (the low half of a surrogate pair, a combining mark) take the
        // cluster's start.
        let mut last = 0.0;
        for x in col_x.iter_mut() {
            if x.is_nan() {
                *x = last;
            } else {
                last = *x;
            }
        }
        LineLayout { col_x, stop, width }
    }
}
