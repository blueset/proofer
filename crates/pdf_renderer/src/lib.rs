//! PDF Renderer — walks Layout IR and emits PDF via krilla.

use font_registry::{FontId, FontRegistry};
use krilla::color::rgb;
use krilla::geom::{PathBuilder, Point, Rect, Transform};
use krilla::num::NormalizedF32;
use krilla::page::PageSettings;
use krilla::paint::{Fill, FillRule, Stroke};
use krilla::text::{Font, TextDirection};
use krilla::Document;
use layout_ir::{Color, DrawCommand, LayoutDocument, Page};
use std::collections::HashMap;
use std::sync::Arc;
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

/// Render a LayoutDocument to PDF bytes.
pub fn render_to_pdf(
    doc: &LayoutDocument,
    registry: &FontRegistry,
) -> Result<Vec<u8>, RenderError> {
    let mut document = Document::new();
    let mut font_cache: HashMap<FontId, Font> = HashMap::new();

    for page in &doc.pages {
        let settings = PageSettings::from_wh(page.width, page.height)
            .ok_or_else(|| RenderError::Krilla("invalid page dimensions".into()))?;
        let mut krilla_page = document.start_page_with(settings);
        let mut surface = krilla_page.surface();

        for cmd in &page.commands {
            render_command(&mut surface, cmd, registry, &mut font_cache)?;
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
    font_cache: &mut HashMap<FontId, Font>,
) -> Result<(), RenderError> {
    match cmd {
        DrawCommand::GlyphRun {
            font_id,
            size,
            glyphs,
            text,
        } => {
            let krilla_font = get_or_load_font(*font_id, registry, font_cache)?;
            // Use draw_glyphs for positioned glyphs
            // For now, use draw_text as a simpler approach for text runs
            if !text.is_empty() && !glyphs.is_empty() {
                let pos = Point::from_xy(glyphs[0].x, glyphs[0].y);
                let font_size = if *size > 0.0 { *size } else { 12.0 };
                surface.draw_text(pos, krilla_font, font_size, text, false, TextDirection::Auto);
            }
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
            // Labels use a system font — skip for now
            // TODO: Load a default system font for labels
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
                        render_command(surface, child, registry, font_cache)?;
                    }
                    surface.pop();
                }
            }
        }

        DrawCommand::Image { data, x, y, w, h } => {
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
                render_command(surface, child, registry, font_cache)?;
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
    cache: &mut HashMap<FontId, Font>,
) -> Result<Font, RenderError> {
    if let Some(font) = cache.get(&font_id) {
        return Ok(font.clone());
    }

    let (data, face_index) = registry.font_data(font_id)?;
    let data_vec: Vec<u8> = (*data).clone();
    let krilla_font = Font::new(data_vec.into(), face_index)
        .ok_or_else(|| RenderError::Krilla("failed to load font in krilla".into()))?;

    cache.insert(font_id, krilla_font.clone());
    Ok(krilla_font)
}

fn color_to_paint(c: Color) -> rgb::Color {
    rgb::Color::new(
        (c.r * 255.0) as u8,
        (c.g * 255.0) as u8,
        (c.b * 255.0) as u8,
    )
}
