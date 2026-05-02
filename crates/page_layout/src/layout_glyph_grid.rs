//! Glyph Grid layout — grid and compact modes with metric lines.

use content_resolver::ResolvedContent;
use font_inspector::{FontInspector, GlyphInfo};
use font_registry::{FontId, FontRegistry};
use layout_ir::{Color, DrawCommand, PositionedGlyph, StrokeStyle};
use proof_model::{DesignAttributes, GlyphGridMode};

use crate::{LayoutError, PageAllocator};

/// A single cell in the glyph grid.
struct GlyphCell {
    glyph_id: u32,
    codepoint: Option<char>,
    label: String,
    width: f32,
    height: f32,
}

pub fn layout(
    allocator: &mut PageAllocator,
    content: &ResolvedContent,
    design_attrs: &DesignAttributes,
    mode: &GlyphGridMode,
    show_metrics: bool,
    show_names: bool,
    cell_padding: f32,
    font_id: FontId,
    registry: &FontRegistry,
) -> Result<(), LayoutError> {
    let glyphs = match content {
        ResolvedContent::Glyphs(g) => g.clone(),
        ResolvedContent::Text(text) => {
            // Convert text chars to glyph infos
            let inspector = FontInspector::new(registry);
            let all = inspector.enumerate_glyphs(font_id)?;
            let chars: std::collections::HashSet<char> = text.chars().collect();
            all.into_iter()
                .filter(|g| g.codepoint.is_some_and(|c| chars.contains(&c)))
                .collect()
        }
    };

    if glyphs.is_empty() {
        return Ok(());
    }

    let inspector = FontInspector::new(registry);
    let global_metrics = inspector.get_metrics(font_id)?;
    let font_size = design_attrs.font_size;
    let scale = font_size / global_metrics.units_per_em as f32;
    let label_size: f32 = 6.0;
    let label_height = if show_names { label_size + 4.0 } else { 0.0 };

    match mode {
        GlyphGridMode::Grid => {
            layout_uniform_grid(
                allocator,
                &glyphs,
                font_id,
                font_size,
                scale,
                &global_metrics,
                &inspector,
                show_metrics,
                show_names,
                label_height,
                cell_padding,
                registry,
            )?;
        }
        GlyphGridMode::Compact => {
            layout_compact_grid(
                allocator,
                &glyphs,
                font_id,
                font_size,
                scale,
                &global_metrics,
                &inspector,
                show_metrics,
                show_names,
                label_height,
                cell_padding,
                registry,
            )?;
        }
    }

    Ok(())
}

fn layout_uniform_grid(
    allocator: &mut PageAllocator,
    glyphs: &[GlyphInfo],
    font_id: FontId,
    font_size: f32,
    scale: f32,
    global_metrics: &font_inspector::GlobalMetrics,
    inspector: &FontInspector<'_>,
    show_metrics: bool,
    show_names: bool,
    label_height: f32,
    padding: f32,
    registry: &FontRegistry,
) -> Result<(), LayoutError> {
    // Compute uniform cell size from max advance/bbox
    let mut max_advance: f32 = 0.0;
    let mut max_bbox_width: f32 = 0.0;
    let mut max_bbox_height: f32 = 0.0;

    for glyph in glyphs {
        if let Ok(gm) = inspector.get_glyph_metrics(font_id, glyph.glyph_id) {
            max_advance = max_advance.max(gm.advance_width * scale);
            if let Some(bbox) = gm.bbox {
                let w = (bbox.x_max - bbox.x_min) * scale;
                let h = (bbox.y_max - bbox.y_min) * scale;
                max_bbox_width = max_bbox_width.max(w);
                max_bbox_height = max_bbox_height.max(h);
            }
        }
    }

    let cell_glyph_width = max_advance.max(max_bbox_width).max(font_size * 0.5);
    let cell_width = cell_glyph_width + padding * 2.0;
    let ascent = global_metrics.ascender * scale;
    let descent = -global_metrics.descender * scale;
    let cell_glyph_height = (ascent + descent).max(max_bbox_height);
    let cell_height = cell_glyph_height + padding * 2.0 + label_height;

    let body_width = allocator.body_width();
    let cols = (body_width / cell_width).floor().max(1.0) as usize;
    let actual_cell_width = body_width / cols as f32;

    // Layout rows
    let mut glyph_idx = 0;
    while glyph_idx < glyphs.len() {
        allocator.ensure_space(cell_height);

        let row_y = allocator.cursor_y();
        let row_end = (glyph_idx + cols).min(glyphs.len());

        for col in 0..(row_end - glyph_idx) {
            let glyph = &glyphs[glyph_idx + col];
            let cell_x = allocator.body_left() + col as f32 * actual_cell_width;
            let baseline_y = row_y + padding + ascent;

            // Cell border
            allocator.push_command(DrawCommand::Rect {
                x: cell_x,
                y: row_y,
                w: actual_cell_width,
                h: cell_height,
                fill: None,
                stroke: Some(StrokeStyle::hairline(Color::LIGHT_GRAY)),
            });

            // Metric lines
            if show_metrics {
                // Baseline
                allocator.push_command(DrawCommand::Line {
                    x1: cell_x,
                    y1: baseline_y,
                    x2: cell_x + actual_cell_width,
                    y2: baseline_y,
                    stroke: StrokeStyle::new(0.25, Color::BLUE),
                });

                // Ascender
                let asc_y = baseline_y - ascent;
                allocator.push_command(DrawCommand::Line {
                    x1: cell_x,
                    y1: asc_y,
                    x2: cell_x + actual_cell_width,
                    y2: asc_y,
                    stroke: StrokeStyle::hairline(Color::new(0.8, 0.2, 0.2, 0.5)),
                });

                // Descender
                let desc_y = baseline_y + descent;
                allocator.push_command(DrawCommand::Line {
                    x1: cell_x,
                    y1: desc_y,
                    x2: cell_x + actual_cell_width,
                    y2: desc_y,
                    stroke: StrokeStyle::hairline(Color::new(0.8, 0.2, 0.2, 0.5)),
                });
            }

            // Glyph
            let glyph_x = cell_x + actual_cell_width / 2.0; // centered
            allocator.push_command(DrawCommand::GlyphRun {
                font_id,
                size: font_size,
                glyphs: vec![PositionedGlyph {
                    glyph_id: glyph.glyph_id,
                    x: glyph_x,
                    y: baseline_y,
                }],
                text: glyph
                    .codepoint
                    .map(|c| c.to_string())
                    .unwrap_or_default(),
            });

            // Label
            if show_names {
                let label = glyph
                    .codepoint
                    .map(|c| format!("U+{:04X}", c as u32))
                    .unwrap_or_else(|| format!("#{}", glyph.glyph_id));
                allocator.push_command(DrawCommand::Label {
                    text: label,
                    x: cell_x + 2.0,
                    y: row_y + cell_height - 2.0,
                    size: 5.0,
                    color: Color::gray(0.5),
                });
            }
        }

        glyph_idx = row_end;
        allocator.advance(cell_height);
    }

    Ok(())
}

fn layout_compact_grid(
    allocator: &mut PageAllocator,
    glyphs: &[GlyphInfo],
    font_id: FontId,
    font_size: f32,
    scale: f32,
    global_metrics: &font_inspector::GlobalMetrics,
    inspector: &FontInspector<'_>,
    show_metrics: bool,
    show_names: bool,
    label_height: f32,
    padding: f32,
    registry: &FontRegistry,
) -> Result<(), LayoutError> {
    let ascent = global_metrics.ascender * scale;
    let descent = -global_metrics.descender * scale;
    let min_cell_width = font_size * 0.8;

    // Compute per-glyph cell widths
    let mut cells: Vec<GlyphCell> = Vec::with_capacity(glyphs.len());
    for glyph in glyphs {
        let gm = inspector
            .get_glyph_metrics(font_id, glyph.glyph_id)
            .unwrap_or(font_inspector::GlyphMetrics {
                advance_width: 0.0,
                lsb: 0.0,
                bbox: None,
            });

        let glyph_width = gm.advance_width * scale;
        let bbox_width = gm
            .bbox
            .map(|b| (b.x_max - b.x_min) * scale)
            .unwrap_or(0.0);
        let label = glyph
            .codepoint
            .map(|c| format!("U+{:04X}", c as u32))
            .unwrap_or_else(|| format!("#{}", glyph.glyph_id));
        let label_width = label.len() as f32 * 3.5; // approximate

        let cell_w = glyph_width
            .max(bbox_width)
            .max(label_width)
            .max(min_cell_width)
            + padding * 2.0;

        let cell_h = (ascent + descent).max(
            gm.bbox
                .map(|b| (b.y_max - b.y_min) * scale)
                .unwrap_or(0.0),
        ) + padding * 2.0
            + label_height;

        cells.push(GlyphCell {
            glyph_id: glyph.glyph_id,
            codepoint: glyph.codepoint,
            label,
            width: cell_w,
            height: cell_h,
        });
    }

    // Bin-pack cells into rows
    let body_width = allocator.body_width();
    let mut cell_idx = 0;

    while cell_idx < cells.len() {
        // Build a row
        let mut row_cells = Vec::new();
        let mut row_width = 0.0;
        let mut row_height: f32 = 0.0;

        while cell_idx < cells.len() {
            let cell = &cells[cell_idx];
            if row_width + cell.width > body_width && !row_cells.is_empty() {
                break;
            }
            row_width += cell.width;
            row_height = row_height.max(cell.height);
            row_cells.push(cell_idx);
            cell_idx += 1;
        }

        allocator.ensure_space(row_height);
        let row_y = allocator.cursor_y();
        let mut x = allocator.body_left();

        for &idx in &row_cells {
            let cell = &cells[idx];
            let baseline_y = row_y + padding + ascent;

            // Cell border
            allocator.push_command(DrawCommand::Rect {
                x,
                y: row_y,
                w: cell.width,
                h: row_height,
                fill: None,
                stroke: Some(StrokeStyle::hairline(Color::LIGHT_GRAY)),
            });

            // Glyph
            allocator.push_command(DrawCommand::GlyphRun {
                font_id,
                size: font_size,
                glyphs: vec![PositionedGlyph {
                    glyph_id: cell.glyph_id,
                    x: x + cell.width / 2.0,
                    y: baseline_y,
                }],
                text: cell.codepoint.map(|c| c.to_string()).unwrap_or_default(),
            });

            // Label
            if show_names {
                allocator.push_command(DrawCommand::Label {
                    text: cell.label.clone(),
                    x: x + 2.0,
                    y: row_y + row_height - 2.0,
                    size: 5.0,
                    color: Color::gray(0.5),
                });
            }

            x += cell.width;
        }

        allocator.advance(row_height);
    }

    Ok(())
}
