//! PDF Renderer — walks Layout IR and emits PDF via krilla.

use font_registry::{FontId, FontRegistry};
use krilla::color::rgb;
use krilla::geom::{PathBuilder, Point, Rect, Transform};
use krilla::num::NormalizedF32;
use krilla::page::PageSettings;
use krilla::paint::{Fill, FillRule, Stroke};
use krilla::text::{Font, GlyphId, KrillaGlyph, TextDirection};
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

/// A font ID key that also encodes "label font" vs "user font".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum FontCacheKey {
    User(FontId),
    Label,
}

/// Render a LayoutDocument to PDF bytes.
pub fn render_to_pdf(
    doc: &LayoutDocument,
    registry: &FontRegistry,
) -> Result<Vec<u8>, RenderError> {
    let mut document = Document::new();
    let mut font_cache: HashMap<FontCacheKey, Font> = HashMap::new();

    // Pre-load a system font for labels
    let label_font = load_label_font();

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
        } => {
            if glyphs.is_empty() {
                return Ok(());
            }

            let krilla_font = get_or_load_font(*font_id, registry, font_cache)?;
            let font_size = if *size > 0.0 { *size } else { 12.0 };
            let upm = krilla_font.units_per_em() as f32;

            // Build KrillaGlyphs with proper positioning
            // Our glyphs store absolute (x, y) positions. We convert these into
            // relative advances for krilla's draw_glyphs API which takes a start
            // point and relative advances.
            let start = Point::from_xy(glyphs[0].x, glyphs[0].y);

            let mut krilla_glyphs: Vec<KrillaGlyph> = Vec::with_capacity(glyphs.len());
            for (i, g) in glyphs.iter().enumerate() {
                // Compute x_advance: distance to next glyph (or 0 for last)
                let x_advance_pts = if i + 1 < glyphs.len() {
                    glyphs[i + 1].x - g.x
                } else {
                    0.0
                };

                // Compute offsets relative to the expected position
                let x_offset_pts = if i == 0 {
                    0.0
                } else {
                    // offset from where the cursor would be after previous advances
                    let expected_x: f32 = glyphs[0].x
                        + glyphs[..i]
                            .windows(2)
                            .map(|w| w[1].x - w[0].x)
                            .sum::<f32>();
                    g.x - expected_x
                };
                let y_offset_pts = if i == 0 { 0.0 } else { g.y - glyphs[0].y };

                // Normalize by UPM (KrillaGlyph values are multiplied by font_size internally)
                let norm = upm / font_size;

                // Determine text range for this glyph
                let text_len = text.len();
                let range_start = (i * text_len / glyphs.len().max(1)).min(text_len);
                let range_end = ((i + 1) * text_len / glyphs.len().max(1)).min(text_len);

                krilla_glyphs.push(KrillaGlyph::new(
                    GlyphId::new(g.glyph_id),
                    x_advance_pts * norm / font_size,
                    x_offset_pts * norm / font_size,
                    y_offset_pts * norm / font_size,
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
            surface.draw_glyphs(start, &krilla_glyphs, krilla_font, text, font_size, false);
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
    registry: &FontRegistry,
    cache: &mut HashMap<FontCacheKey, Font>,
) -> Result<Font, RenderError> {
    let key = FontCacheKey::User(font_id);
    if let Some(font) = cache.get(&key) {
        return Ok(font.clone());
    }

    let (data, face_index) = registry.font_data(font_id)?;
    let data_vec: Vec<u8> = (*data).clone();
    let krilla_font = Font::new(data_vec.into(), face_index)
        .ok_or_else(|| RenderError::Krilla("failed to load font in krilla".into()))?;

    cache.insert(key, krilla_font.clone());
    Ok(krilla_font)
}

/// Try to load a system font for label rendering.
fn load_label_font() -> Option<Font> {
    // Try common system font paths
    let candidates = [
        r"C:\Windows\Fonts\arial.ttf",
        r"C:\Windows\Fonts\segoeui.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/System/Library/Fonts/Helvetica.ttc",
        "/System/Library/Fonts/SFNSText.ttf",
    ];

    for path in &candidates {
        if let Ok(data) = std::fs::read(path) {
            if let Some(font) = Font::new(data.into(), 0) {
                return Some(font);
            }
        }
    }

    None
}

fn color_to_paint(c: Color) -> rgb::Color {
    rgb::Color::new(
        (c.r * 255.0) as u8,
        (c.g * 255.0) as u8,
        (c.b * 255.0) as u8,
    )
}
