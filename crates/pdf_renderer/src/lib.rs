//! PDF Renderer — walks Layout IR and emits PDF via krilla.

use font_registry::{FontId, FontRegistry};
use krilla::color::rgb;
use krilla::geom::{PathBuilder, Point, Rect, Transform};
use krilla::num::NormalizedF32;
use krilla::page::PageSettings;
use krilla::paint::{Fill, FillRule, Stroke};
use krilla::text::{Font, GlyphId, KrillaGlyph, Tag, TextDirection};
use krilla::Document;
use layout_ir::{Color, DrawCommand, LayoutDocument};
use std::collections::HashMap;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum RenderError {
    #[error("font error: {0}")]
    Font(#[from] font_registry::FontError),
    #[error("krilla error: {0}")]
    Krilla(String),
    #[error("font not loaded in registry: {0:?}")]
    FontNotLoaded(FontId),
}

/// A font cache key encoding font ID + variation location.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum FontCacheKey {
    User(FontId, Vec<(String, i32)>), // i32 = f32 bits for hashing
    Label,
}

fn variation_cache_key(font_id: FontId, variations: &[(String, f32)]) -> FontCacheKey {
    let key: Vec<(String, i32)> = variations
        .iter()
        .map(|(tag, val)| (tag.clone(), val.to_bits() as i32))
        .collect();
    FontCacheKey::User(font_id, key)
}

/// Render a LayoutDocument to PDF bytes.
pub fn render_to_pdf(
    doc: &LayoutDocument,
    registry: &FontRegistry,
) -> Result<Vec<u8>, RenderError> {
    let mut document = Document::new();
    let mut font_cache: HashMap<FontCacheKey, Font> = HashMap::new();

    // Load bundled label font into krilla
    let label_font = Font::new(font_registry::LABEL_FONT_BYTES.to_vec().into(), 0);

    for page in &doc.pages {
        let settings = PageSettings::from_wh(page.width, page.height)
            .ok_or_else(|| RenderError::Krilla("invalid page dimensions".into()))?;
        let mut krilla_page = document.start_page_with(settings);
        let mut surface = krilla_page.surface();

        for cmd in &page.commands {
            render_command(
                &mut surface,
                cmd,
                registry,
                &mut font_cache,
                label_font.as_ref(),
            )?;
        }

        surface.finish();
        krilla_page.finish();
    }

    let pdf_bytes = document
        .finish()
        .map_err(|_| RenderError::Krilla("failed to finish PDF document".into()))?;
    Ok(pdf_bytes)
}

fn render_command(
    surface: &mut krilla::surface::Surface<'_>,
    cmd: &DrawCommand,
    registry: &FontRegistry,
    font_cache: &mut HashMap<FontCacheKey, Font>,
    label_font: Option<&Font>,
) -> Result<(), RenderError> {
    match cmd {
        DrawCommand::GlyphRun {
            font_id,
            size,
            glyphs,
            text,
            variations,
        } => {
            if glyphs.is_empty() {
                return Ok(());
            }

            let krilla_font = get_or_load_font(*font_id, variations, registry, font_cache)?;
            let font_size = if *size > 0.0 { *size } else { 12.0 };

            // Our glyphs have absolute (x, y) positions in points (from parley).
            // krilla's draw_glyphs takes a start point + relative advances/offsets.
            // KrillaGlyph fields are normalized: multiplied by font_size internally.
            // So: normalized_value = point_value / font_size
            let start = Point::from_xy(glyphs[0].x, glyphs[0].y);

            let mut krilla_glyphs: Vec<KrillaGlyph> = Vec::with_capacity(glyphs.len());
            for (i, g) in glyphs.iter().enumerate() {
                let x_advance_pts = if i + 1 < glyphs.len() {
                    glyphs[i + 1].x - g.x
                } else {
                    0.0
                };
                let y_offset_pts = if i == 0 { 0.0 } else { g.y - glyphs[0].y };

                // Text range: distribute text bytes across glyphs, ensuring
                // ranges fall on char boundaries (critical for multi-byte UTF-8).
                let text_len = text.len();
                let raw_start = (i * text_len / glyphs.len().max(1)).min(text_len);
                let raw_end = ((i + 1) * text_len / glyphs.len().max(1)).min(text_len);
                // Snap to nearest char boundary
                let range_start = snap_to_char_boundary(text, raw_start);
                let range_end = snap_to_char_boundary(text, raw_end);

                krilla_glyphs.push(KrillaGlyph::new(
                    GlyphId::new(g.glyph_id),
                    x_advance_pts / font_size,  // normalized advance
                    0.0,                         // x_offset already in start position
                    y_offset_pts / font_size,    // normalized y offset
                    0.0,
                    range_start..range_end,
                    None,
                ));
            }

            surface.set_fill(Some(Fill {
                paint: rgb::Color::new(0, 0, 0).into(),
                rule: FillRule::NonZero,
                opacity: NormalizedF32::ONE,
            }));
            surface.set_stroke(None);
            // Use outlined rendering for single-glyph runs (glyph grid cells)
            // to ensure metric lines and glyph shapes align exactly.
            // Multi-glyph runs (text) use the standard CID text path.
            let outlined = glyphs.len() == 1;
            surface.draw_glyphs(start, &krilla_glyphs, krilla_font, text, font_size, outlined);
        }

        DrawCommand::Line {
            x1,
            y1,
            x2,
            y2,
            stroke,
        } => {
            let mut pb = PathBuilder::new();
            pb.move_to(*x1, *y1);
            pb.line_to(*x2, *y2);
            if let Some(path) = pb.finish() {
                surface.set_stroke(Some(Stroke {
                    paint: color_to_paint(stroke.color).into(),
                    width: stroke.width,
                    ..Default::default()
                }));
                surface.set_fill(None);
                surface.draw_path(&path);
            }
        }

        DrawCommand::Rect {
            x,
            y,
            w,
            h,
            fill,
            stroke,
        } => {
            if let Some(rect) = Rect::from_xywh(*x, *y, *w, *h) {
                let mut pb = PathBuilder::new();
                pb.push_rect(rect);
                if let Some(path) = pb.finish() {
                    if let Some(fill_color) = fill {
                        surface.set_fill(Some(Fill {
                            paint: color_to_paint(*fill_color).into(),
                            rule: FillRule::NonZero,
                            opacity: NormalizedF32::ONE,
                        }));
                    } else {
                        surface.set_fill(None);
                    }
                    if let Some(stroke_style) = stroke {
                        surface.set_stroke(Some(Stroke {
                            paint: color_to_paint(stroke_style.color).into(),
                            width: stroke_style.width,
                            ..Default::default()
                        }));
                    } else {
                        surface.set_stroke(None);
                    }
                    surface.draw_path(&path);
                }
            }
        }

        DrawCommand::Label {
            text,
            x,
            y,
            size,
            color,
        } => {
            if let Some(font) = label_font {
                let c = color_to_paint(*color);
                surface.set_fill(Some(Fill {
                    paint: c.into(),
                    rule: FillRule::NonZero,
                    opacity: NormalizedF32::ONE,
                }));
                surface.set_stroke(None);
                surface.draw_text(
                    Point::from_xy(*x, *y),
                    font.clone(),
                    *size,
                    text,
                    false,
                    TextDirection::Auto,
                );
            }
        }

        DrawCommand::Clip {
            x,
            y,
            w,
            h,
            children,
        } => {
            if let Some(rect) = Rect::from_xywh(*x, *y, *w, *h) {
                let mut pb = PathBuilder::new();
                pb.push_rect(rect);
                if let Some(clip) = pb.finish() {
                    surface.push_clip_path(&clip, &FillRule::NonZero);
                    for child in children {
                        render_command(surface, child, registry, font_cache, label_font)?;
                    }
                    surface.pop();
                }
            }
        }

        DrawCommand::Image { .. } => {
            // TODO: Image rendering
        }

        DrawCommand::Group {
            transform,
            children,
        } => {
            if let Some(t) = transform {
                let tr = Transform::from_row(t.a, t.b, t.c, t.d, t.e, t.f);
                surface.push_transform(&tr);
            }
            for child in children {
                render_command(surface, child, registry, font_cache, label_font)?;
            }
            if transform.is_some() {
                surface.pop();
            }
        }
    }

    Ok(())
}

fn get_or_load_font(
    font_id: FontId,
    variations: &[(String, f32)],
    registry: &FontRegistry,
    cache: &mut HashMap<FontCacheKey, Font>,
) -> Result<Font, RenderError> {
    let key = variation_cache_key(font_id, variations);
    if let Some(font) = cache.get(&key) {
        return Ok(font.clone());
    }

    let (data, face_index) = registry.font_data(font_id)?;
    let data_vec: Vec<u8> = (*data).clone();

    let krilla_font = if variations.is_empty() {
        Font::new(data_vec.into(), face_index)
    } else {
        // Convert (String, f32) to (Tag, f32) for krilla
        let var_coords: Vec<(Tag, f32)> = variations
            .iter()
            .filter_map(|(tag, val)| {
                let bytes = tag.as_bytes();
                if bytes.len() == 4 {
                    Some((Tag::new(&[bytes[0], bytes[1], bytes[2], bytes[3]]), *val))
                } else {
                    None
                }
            })
            .collect();
        Font::new_variable(data_vec.into(), face_index, var_coords.as_slice())
    }
    .ok_or_else(|| RenderError::Krilla("failed to load font in krilla".into()))?;

    cache.insert(key, krilla_font.clone());
    Ok(krilla_font)
}

fn color_to_paint(c: Color) -> rgb::Color {
    rgb::Color::new(
        (c.r * 255.0) as u8,
        (c.g * 255.0) as u8,
        (c.b * 255.0) as u8,
    )
}

/// Snap a byte index to the nearest valid char boundary in a UTF-8 string.
fn snap_to_char_boundary(s: &str, index: usize) -> usize {
    if index >= s.len() {
        return s.len();
    }
    if s.is_char_boundary(index) {
        return index;
    }
    // Walk backwards to find the start of the current character
    let mut i = index;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}
