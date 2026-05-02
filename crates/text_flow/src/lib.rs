//! Text Flow — text layout with threading via parley.
//!
//! Wraps parley to provide the imperative text-threading primitive (vsplit).
//! This is the key abstraction that enables flowing text across containers and pages.

use font_registry::{FontId, FontRegistry};
use layout_ir::{DrawCommand, PositionedGlyph};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum TextFlowError {
    #[error("font error: {0}")]
    Font(#[from] font_registry::FontError),
    #[error("layout error: {0}")]
    Layout(String),
}

/// Metrics for a single shaped line.
#[derive(Debug, Clone)]
pub struct LineMetrics {
    pub ascent: f32,
    pub descent: f32,
    pub leading: f32,
    pub width: f32,
}

impl LineMetrics {
    pub fn height(&self) -> f32 {
        self.ascent + self.descent + self.leading
    }
}

/// A single shaped glyph run within a line.
#[derive(Debug, Clone)]
pub struct ShapedRun {
    pub font_id: FontId,
    pub glyphs: Vec<PositionedGlyph>,
    pub text_range: std::ops::Range<usize>,
}

/// A single shaped line ready for rendering.
#[derive(Debug, Clone)]
pub struct ShapedLine {
    pub runs: Vec<ShapedRun>,
    pub metrics: LineMetrics,
}

/// Style parameters for text shaping.
#[derive(Debug, Clone)]
pub struct TextStyle {
    pub font_id: FontId,
    pub font_size: f32,
    pub line_height: Option<f32>,
    pub tracking: f32,
    pub kerning: bool,
    pub features: Vec<(String, u32)>,
    pub language: Option<String>,
    /// Variation axis settings (tag string, value) for variable fonts.
    pub variations: Vec<(String, f32)>,
}

/// A text flow represents shaped text that can be consumed line by line.
///
/// The core vsplit primitive: shape text once at a given width, then
/// consume lines incrementally into containers of various heights.
pub struct TextFlow {
    lines: Vec<ShapedLine>,
    cursor: usize,
    style: TextStyle,
    original_text: String,
}

impl TextFlow {
    /// Create a new text flow by shaping text at the given width.
    ///
    /// Uses parley for proper text shaping with OpenType features,
    /// ligatures, and complex script support. Falls back to a simple
    /// word-wrapper if parley shaping fails.
    pub fn new(
        text: &str,
        style: TextStyle,
        max_width: f32,
        registry: &FontRegistry,
    ) -> Result<Self, TextFlowError> {
        // Try parley-based shaping first
        match shape_with_parley(text, &style, max_width, registry) {
            Ok(lines) => Ok(Self {
                lines,
                cursor: 0,
                style: style.clone(),
                original_text: text.to_string(),
            }),
            Err(_) => {
                // Fall back to simple shaping
                let lines = shape_simple(text, &style, max_width, registry)?;
                Ok(Self {
                    lines,
                    cursor: 0,
                    style,
                    original_text: text.to_string(),
                })
            }
        }
    }

    /// Check if there are remaining unconsumed lines.
    pub fn has_remaining(&self) -> bool {
        self.cursor < self.lines.len()
    }

    /// Peek at the height of the next unconsumed line.
    pub fn peek_line_height(&self) -> Option<f32> {
        self.lines.get(self.cursor).map(|l| l.metrics.height())
    }

    /// The vsplit primitive: consume lines until `height` is exhausted.
    /// Returns the lines that fit and the remaining height.
    pub fn consume_into(&mut self, height: f32) -> (Vec<ShapedLine>, f32) {
        let mut consumed = Vec::new();
        let mut remaining = height;

        while self.cursor < self.lines.len() {
            let line = &self.lines[self.cursor];
            let line_h = line.metrics.height();
            if line_h > remaining && !consumed.is_empty() {
                break;
            }
            consumed.push(self.lines[self.cursor].clone());
            remaining -= line_h;
            self.cursor += 1;
            if remaining <= 0.0 {
                break;
            }
        }

        (consumed, remaining.max(0.0))
    }

    /// Get the total number of lines.
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// Get the cursor position.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Reset the cursor to the beginning.
    pub fn reset(&mut self) {
        self.cursor = 0;
    }

    /// Get the text style.
    pub fn style(&self) -> &TextStyle {
        &self.style
    }

    /// Get the original text.
    pub fn text(&self) -> &str {
        &self.original_text
    }

    /// Convert consumed lines to draw commands at a given position.
    pub fn lines_to_commands(
        lines: &[ShapedLine],
        x_offset: f32,
        y_start: f32,
        text: &str,
    ) -> Vec<DrawCommand> {
        let mut commands = Vec::new();
        let mut y = y_start;

        for line in lines {
            y += line.metrics.ascent;

            for run in &line.runs {
                let positioned: Vec<PositionedGlyph> = run
                    .glyphs
                    .iter()
                    .map(|g| PositionedGlyph {
                        glyph_id: g.glyph_id,
                        x: g.x + x_offset,
                        y,
                    })
                    .collect();

                let run_text = if run.text_range.end <= text.len() {
                    text[run.text_range.clone()].to_string()
                } else {
                    String::new()
                };

                commands.push(DrawCommand::GlyphRun {
                    font_id: run.font_id,
                    size: 0.0, // size is baked into glyph positions
                    glyphs: positioned,
                    text: run_text,
                });
            }

            y += line.metrics.descent + line.metrics.leading;
        }

        commands
    }
}

/// Synchronized consumption from multiple flows.
/// Stops when ANY flow fills its allocation.
pub fn sync_consume(
    flows: &mut [TextFlow],
    height: f32,
) -> Vec<Vec<ShapedLine>> {
    let mut results: Vec<Vec<ShapedLine>> = flows.iter().map(|_| Vec::new()).collect();
    let mut remaining = height;

    loop {
        // Find the max line height across all flows for the next line
        let mut max_line_h: f32 = 0.0;
        let mut any_remaining = false;

        for flow in flows.iter() {
            if flow.has_remaining() {
                any_remaining = true;
                if let Some(h) = flow.peek_line_height() {
                    max_line_h = max_line_h.max(h);
                }
            }
        }

        if !any_remaining || max_line_h > remaining {
            break;
        }

        // Consume one line from each flow
        for (i, flow) in flows.iter_mut().enumerate() {
            if flow.has_remaining() {
                let (lines, _) = flow.consume_into(max_line_h);
                results[i].extend(lines);
            }
        }

        remaining -= max_line_h;
        if remaining <= 0.0 {
            break;
        }
    }

    results
}

fn make_shaped_line(
    glyphs: &[PositionedGlyph],
    font_id: FontId,
    text_start: usize,
    text_end: usize,
    ascent: f32,
    descent: f32,
    leading: f32,
    line_height: f32,
    width: f32,
) -> ShapedLine {
    // Adjust ascent/descent to match desired line height
    let natural_height = ascent + descent + leading;
    let extra = (line_height - natural_height).max(0.0);
    let adjusted_ascent = ascent + extra / 2.0;
    let adjusted_descent = descent + extra / 2.0;

    ShapedLine {
        runs: vec![ShapedRun {
            font_id,
            glyphs: glyphs.to_vec(),
            text_range: text_start..text_end,
        }],
        metrics: LineMetrics {
            ascent: adjusted_ascent,
            descent: adjusted_descent,
            leading: 0.0, // folded into ascent/descent
            width,
        },
    }
}

use skrifa::MetadataProvider;

/// Shape text using parley for proper OpenType shaping, ligatures, and complex scripts.
fn shape_with_parley(
    text: &str,
    style: &TextStyle,
    max_width: f32,
    registry: &FontRegistry,
) -> Result<Vec<ShapedLine>, TextFlowError> {
    use parley::FontContext;
    use parley::LayoutContext;
    use parley::style::{FontStack, FontFeature, FontVariation, FontSettings};

    // Get font data and metadata
    let (font_data, face_index) = registry.font_data(style.font_id)?;
    let font_meta = registry.metadata(style.font_id)?;

    // Create fontique collection and register our font
    let mut font_ctx = FontContext::new();
    let blob: fontique::Blob<u8> = fontique::Blob::new(std::sync::Arc::new((*font_data).clone()));
    let registered = font_ctx.collection.register_fonts(blob, None);

    if registered.is_empty() {
        return Err(TextFlowError::Layout("failed to register font with fontique".into()));
    }

    // Use the font's family name for the font stack
    let family_name = font_meta.family.clone();

    // Build variation settings
    let var_settings: Vec<FontVariation> = style
        .variations
        .iter()
        .filter_map(|(tag, val)| {
            let tag_bytes = tag.as_bytes();
            if tag_bytes.len() == 4 {
                let arr: [u8; 4] = [tag_bytes[0], tag_bytes[1], tag_bytes[2], tag_bytes[3]];
                Some(FontVariation {
                    tag: swash::tag_from_bytes(&arr),
                    value: *val,
                })
            } else {
                None
            }
        })
        .collect();

    // Build feature settings
    let feat_settings: Vec<FontFeature> = style
        .features
        .iter()
        .filter_map(|(tag, val)| {
            let tag_bytes = tag.as_bytes();
            if tag_bytes.len() == 4 {
                let arr: [u8; 4] = [tag_bytes[0], tag_bytes[1], tag_bytes[2], tag_bytes[3]];
                Some(FontFeature {
                    tag: swash::tag_from_bytes(&arr),
                    value: *val as u16,
                })
            } else {
                None
            }
        })
        .collect();

    // Build parley layout using ranged builder with individual style properties
    let mut layout_ctx: LayoutContext<[u8; 4]> = LayoutContext::new();
    let mut builder = layout_ctx.ranged_builder(&mut font_ctx, text, 1.0, false);
    builder.push_default(parley::style::StyleProperty::FontStack(
        FontStack::Source(std::borrow::Cow::Owned(family_name)),
    ));
    builder.push_default(parley::style::StyleProperty::FontSize(style.font_size));
    builder.push_default(parley::style::StyleProperty::LetterSpacing(
        style.tracking * style.font_size,
    ));
    builder.push_default(parley::style::StyleProperty::LineHeight(
        style.line_height.unwrap_or(1.2),
    ));
    if !var_settings.is_empty() {
        builder.push_default(parley::style::StyleProperty::FontVariations(
            FontSettings::List(std::borrow::Cow::Owned(var_settings)),
        ));
    }
    if !feat_settings.is_empty() {
        builder.push_default(parley::style::StyleProperty::FontFeatures(
            FontSettings::List(std::borrow::Cow::Owned(feat_settings)),
        ));
    }
    let mut layout = builder.build(text);
    layout.break_all_lines(Some(max_width));

    // Extract lines from parley layout
    let mut result_lines = Vec::new();

    for line in layout.lines() {
        let line_metrics = line.metrics();
        let mut runs = Vec::new();

        for item in line.items() {
            if let parley::layout::PositionedLayoutItem::GlyphRun(glyph_run) = item {
                let mut glyphs = Vec::new();

                for g in glyph_run.positioned_glyphs() {
                    glyphs.push(PositionedGlyph {
                        glyph_id: g.id as u32,
                        x: g.x,
                        y: 0.0, // y is handled during consumption via baseline
                    });
                }

                let text_range = glyph_run.run().text_range();
                runs.push(ShapedRun {
                    font_id: style.font_id,
                    glyphs,
                    text_range,
                });
            }
        }

        result_lines.push(ShapedLine {
            runs,
            metrics: LineMetrics {
                ascent: line_metrics.ascent,
                descent: line_metrics.descent,
                leading: line_metrics.leading,
                width: line_metrics.advance,
            },
        });
    }

    if result_lines.is_empty() && !text.is_empty() {
        return Err(TextFlowError::Layout("parley produced no lines".into()));
    }

    Ok(result_lines)
}

/// Simple fallback shaper using skrifa charmap and glyph metrics.
/// Used when parley shaping fails (e.g., font not recognized by fontique).
fn shape_simple(
    text: &str,
    style: &TextStyle,
    max_width: f32,
    registry: &FontRegistry,
) -> Result<Vec<ShapedLine>, TextFlowError> {
    let font = registry.font_ref(style.font_id)?;

    let var_settings: Vec<(&str, f32)> = style
        .variations
        .iter()
        .map(|(tag, val)| (tag.as_str(), *val))
        .collect();
    let location = font.axes().location(var_settings.iter().copied());
    let loc_ref = location.coords();
    let size = skrifa::prelude::Size::new(style.font_size);

    let metrics = font.metrics(size, loc_ref);
    let ascent = metrics.ascent;
    let descent = -metrics.descent;
    let leading = metrics.leading;
    let line_height = style
        .line_height
        .map(|lh| lh * style.font_size)
        .unwrap_or(ascent + descent + leading);

    let glyph_metrics = font.glyph_metrics(size, loc_ref);
    let charmap = font.charmap();

    let mut lines = Vec::new();
    let mut current_line_glyphs = Vec::new();
    let mut current_x: f32 = 0.0;
    let mut line_start = 0usize;
    let mut word_start = 0usize;
    let mut word_glyphs = Vec::new();
    let mut word_width: f32 = 0.0;

    let chars: Vec<char> = text.chars().collect();
    let mut char_idx = 0;
    let mut byte_offset = 0;

    while char_idx < chars.len() {
        let ch = chars[char_idx];
        let ch_len = ch.len_utf8();

        if ch == '\n' {
            if !word_glyphs.is_empty() {
                current_line_glyphs.append(&mut word_glyphs);
                current_x += word_width;
                word_width = 0.0;
            }
            lines.push(make_shaped_line(
                &current_line_glyphs, style.font_id, line_start, byte_offset + ch_len,
                ascent, descent, leading, line_height, current_x,
            ));
            current_line_glyphs.clear();
            current_x = 0.0;
            line_start = byte_offset + ch_len;
            word_start = line_start;
            byte_offset += ch_len;
            char_idx += 1;
            continue;
        }

        let gid = charmap.map(ch);
        let advance = gid
            .map(|g| glyph_metrics.advance_width(g).unwrap_or(0.0))
            .unwrap_or(style.font_size * 0.5);

        let tracking_offset = if char_idx > 0 { style.tracking * style.font_size } else { 0.0 };

        if ch.is_whitespace() {
            if !word_glyphs.is_empty() {
                current_line_glyphs.append(&mut word_glyphs);
                current_x += word_width;
                word_width = 0.0;
            }
            if let Some(gid) = gid {
                current_line_glyphs.push(PositionedGlyph {
                    glyph_id: gid.to_u32(),
                    x: current_x + tracking_offset,
                    y: 0.0,
                });
            }
            current_x += advance + tracking_offset;
            word_start = byte_offset + ch_len;
        } else {
            if current_x + word_width + advance + tracking_offset > max_width
                && !current_line_glyphs.is_empty()
            {
                lines.push(make_shaped_line(
                    &current_line_glyphs, style.font_id, line_start, word_start,
                    ascent, descent, leading, line_height, current_x - word_width,
                ));
                current_line_glyphs.clear();
                let offset = word_glyphs.first().map(|g| g.x).unwrap_or(0.0);
                for g in &mut word_glyphs { g.x -= offset; }
                current_x = word_width;
                line_start = word_start;
            }

            if let Some(gid) = gid {
                word_glyphs.push(PositionedGlyph {
                    glyph_id: gid.to_u32(),
                    x: current_x + word_width + tracking_offset,
                    y: 0.0,
                });
            }
            word_width += advance + tracking_offset;
        }

        byte_offset += ch_len;
        char_idx += 1;
    }

    if !word_glyphs.is_empty() {
        current_line_glyphs.append(&mut word_glyphs);
        current_x += word_width;
    }
    if !current_line_glyphs.is_empty() || line_start < text.len() {
        lines.push(make_shaped_line(
            &current_line_glyphs, style.font_id, line_start, text.len(),
            ascent, descent, leading, line_height, current_x,
        ));
    }

    Ok(lines)
}