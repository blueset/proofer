//! Glyph Grid layout — grid and compact modes with metric lines.
//!
//! Each glyph is shown in its own cell with:
//! - The glyph shape at the configured font size
//! - Horizontal metric lines (ascender, cap-height, baseline, descender)
//! - Vertical side bearing indicators (left/right at glyph origin and advance)
//! - Glyph name label below the glyph area
//!
//! Grid mode: uniform cell size, table-aligned.
//! Compact mode: per-glyph variable width, greedy row packing.

use content_resolver::ResolvedContent;
use font_inspector::{FontInspector, GlyphInfo};
use font_registry::{FontId, FontRegistry};
use layout_ir::{Color, DrawCommand, PositionedGlyph, StrokeStyle};
use proof_model::{DesignAttributes, GlyphGridMode};

use crate::{LayoutError, PageAllocator};

// ── Colors (subtle, non-distracting) ────────────────────────────────

const CELL_BORDER: Color = Color::LIGHT_GRAY;
const METRIC_BASELINE: Color = Color { r: 0.75, g: 0.75, b: 0.75, a: 1.0 };
const METRIC_LINES: Color = Color { r: 0.82, g: 0.82, b: 0.82, a: 1.0 };
const LABEL_COLOR: Color = Color { r: 0.5, g: 0.5, b: 0.5, a: 1.0 };
const LABEL_SIZE: f32 = 5.0;

// ── Per-glyph computed metrics ──────────────────────────────────────

struct CellData {
    glyph_id: u32,
    codepoint: Option<char>,
    label: String,
    /// Advance width in points.
    advance: f32,
    /// Left edge of ink relative to origin (negative = overshoot left).
    ink_left: f32,
    /// Right edge of ink relative to origin.
    ink_right: f32,
    /// Top edge of ink above baseline (positive up, in points).
    ink_top: f32,
    /// Bottom edge of ink below baseline (negative down, in points).
    ink_bottom: f32,
    /// Measured label width in points.
    label_width: f32,
}

impl CellData {
    fn content_width(&self) -> f32 {
        (self.ink_right - self.ink_left).max(self.advance)
    }

    fn content_height(&self, ascent: f32, descent: f32) -> f32 {
        let top = self.ink_top.max(ascent);
        let bottom = (-self.ink_bottom).max(descent);
        top + bottom
    }
}

fn compute_cell_data(
    glyphs: &[GlyphInfo],
    font_id: FontId,
    font_size: f32,
    inspector: &FontInspector<'_>,
    registry: &FontRegistry,
) -> Vec<CellData> {
    let scale = font_size
        / inspector.get_metrics(font_id).map(|m| m.units_per_em as f32).unwrap_or(1000.0);
    let label_font_id = registry.label_font_id();

    glyphs
        .iter()
        .map(|g| {
            let gm = inspector
                .get_glyph_metrics(font_id, g.glyph_id)
                .unwrap_or(font_inspector::GlyphMetrics {
                    advance_width: 0.0,
                    lsb: 0.0,
                    bbox: None,
                });

            let advance = gm.advance_width * scale;
            let lsb = gm.lsb * scale;

            let (ink_left, ink_right, ink_top, ink_bottom) = if let Some(bbox) = gm.bbox {
                (
                    (bbox.x_min * scale).min(0.0),
                    (bbox.x_max * scale).max(advance),
                    bbox.y_max * scale,  // above baseline
                    bbox.y_min * scale,  // below baseline (negative)
                )
            } else {
                (0.0_f32.min(lsb), advance, 0.0, 0.0)
            };

            let label = g.name.clone().unwrap_or_else(|| {
                if g.glyph_id == 0 {
                    ".notdef".to_string()
                } else {
                    format!("GID {}", g.glyph_id)
                }
            });

            let label_width = label_font_id
                .and_then(|id| registry.measure_text(id, &label, LABEL_SIZE).ok())
                .unwrap_or(label.len() as f32 * LABEL_SIZE * 0.5);

            CellData {
                glyph_id: g.glyph_id,
                codepoint: g.codepoint,
                label,
                advance,
                ink_left,
                ink_right,
                ink_top,
                ink_bottom,
                label_width,
            }
        })
        .collect()
}

// ── Public entry point ──────────────────────────────────────────────

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
    let ascent = global_metrics.ascender * scale;
    let descent = -global_metrics.descender * scale; // positive value
    let cap_height = global_metrics.cap_height.map(|h| h * scale);
    let x_height = global_metrics.x_height.map(|h| h * scale);
    let label_height = if show_names { LABEL_SIZE + 6.0 } else { 0.0 };

    let cells = compute_cell_data(&glyphs, font_id, font_size, &inspector, registry);

    match mode {
        GlyphGridMode::Grid => layout_grid(
            allocator, &cells, font_id, font_size, ascent, descent,
            cap_height, x_height, show_metrics, show_names, label_height, cell_padding,
        ),
        GlyphGridMode::Compact => layout_compact(
            allocator, &cells, font_id, font_size, ascent, descent,
            cap_height, x_height, show_metrics, show_names, label_height, cell_padding,
        ),
    }
}

// ── Grid mode ───────────────────────────────────────────────────────

fn layout_grid(
    allocator: &mut PageAllocator,
    cells: &[CellData],
    font_id: FontId,
    font_size: f32,
    ascent: f32,
    descent: f32,
    cap_height: Option<f32>,
    x_height: Option<f32>,
    show_metrics: bool,
    show_names: bool,
    label_height: f32,
    padding: f32,
) -> Result<(), LayoutError> {
    // Uniform cell size: max across all glyphs
    let mut max_content_w: f32 = 0.0;
    let mut max_content_h: f32 = 0.0;
    let mut max_label_w: f32 = 0.0;

    for cell in cells {
        max_content_w = max_content_w.max(cell.content_width());
        max_content_h = max_content_h.max(cell.content_height(ascent, descent));
        max_label_w = max_label_w.max(cell.label_width);
    }

    let min_cell_width = font_size * 0.5;
    let cell_inner_w = max_content_w.max(max_label_w).max(min_cell_width);
    let cell_width = cell_inner_w + padding * 2.0;
    let cell_height = max_content_h + padding * 2.0 + label_height;

    let body_width = allocator.body_width();
    let cols = (body_width / cell_width).floor().max(1.0) as usize;
    let actual_cell_width = body_width / cols as f32;
    let content_area_w = actual_cell_width - padding * 2.0;

    let mut glyph_idx = 0;
    while glyph_idx < cells.len() {
        allocator.ensure_space(cell_height);
        let row_y = allocator.cursor_y();
        let row_end = (glyph_idx + cols).min(cells.len());

        for col in 0..(row_end - glyph_idx) {
            let cell = &cells[glyph_idx + col];
            let cell_x = allocator.body_left() + col as f32 * actual_cell_width;

            // Baseline position: top padding + ascent (or more if glyph exceeds)
            let top_extent = cell.ink_top.max(ascent);
            let baseline_y = row_y + padding + top_extent;

            // Glyph origin x: center the advance width in the content area
            let origin_x = cell_x + padding + (content_area_w - cell.advance) / 2.0;

            render_cell(
                allocator, cell, cell_x, row_y, actual_cell_width, cell_height,
                baseline_y, origin_x, font_id, font_size,
                ascent, descent, cap_height, x_height,
                show_metrics, show_names, label_height, padding,
            );
        }

        glyph_idx = row_end;
        allocator.advance(cell_height);
    }

    Ok(())
}

// ── Compact mode ────────────────────────────────────────────────────

fn layout_compact(
    allocator: &mut PageAllocator,
    cells: &[CellData],
    font_id: FontId,
    font_size: f32,
    ascent: f32,
    descent: f32,
    cap_height: Option<f32>,
    x_height: Option<f32>,
    show_metrics: bool,
    show_names: bool,
    label_height: f32,
    padding: f32,
) -> Result<(), LayoutError> {
    let min_cell_width = font_size * 0.5;
    let body_width = allocator.body_width();

    // Pre-compute per-cell dimensions
    let cell_dims: Vec<(f32, f32)> = cells
        .iter()
        .map(|cell| {
            let inner_w = cell.content_width().max(cell.label_width).max(min_cell_width);
            let w = inner_w + padding * 2.0;
            let h = cell.content_height(ascent, descent) + padding * 2.0 + label_height;
            (w, h)
        })
        .collect();

    let mut cell_idx = 0;
    while cell_idx < cells.len() {
        // Build a row
        let mut row_indices = Vec::new();
        let mut row_width = 0.0;
        let mut row_height: f32 = 0.0;

        while cell_idx < cells.len() {
            let (w, h) = cell_dims[cell_idx];
            if row_width + w > body_width && !row_indices.is_empty() {
                break;
            }
            row_indices.push(cell_idx);
            row_width += w;
            row_height = row_height.max(h);
            cell_idx += 1;
        }

        allocator.ensure_space(row_height);
        let row_y = allocator.cursor_y();
        let mut x = allocator.body_left();

        for &idx in &row_indices {
            let cell = &cells[idx];
            let (cell_w, _) = cell_dims[idx];

            let content_area_w = cell_w - padding * 2.0;
            let top_extent = cell.ink_top.max(ascent);
            let baseline_y = row_y + padding + top_extent;
            let origin_x = x + padding + (content_area_w - cell.advance) / 2.0;

            render_cell(
                allocator, cell, x, row_y, cell_w, row_height,
                baseline_y, origin_x, font_id, font_size,
                ascent, descent, cap_height, x_height,
                show_metrics, show_names, label_height, padding,
            );

            x += cell_w;
        }

        allocator.advance(row_height);
    }

    Ok(())
}

// ── Render a single cell ────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn render_cell(
    allocator: &mut PageAllocator,
    cell: &CellData,
    cell_x: f32,
    cell_y: f32,
    cell_w: f32,
    cell_h: f32,
    baseline_y: f32,
    origin_x: f32,
    font_id: FontId,
    font_size: f32,
    ascent: f32,
    descent: f32,
    cap_height: Option<f32>,
    x_height: Option<f32>,
    show_metrics: bool,
    show_names: bool,
    label_height: f32,
    _padding: f32,
) {
    // Cell border
    allocator.push_command(DrawCommand::Rect {
        x: cell_x,
        y: cell_y,
        w: cell_w,
        h: cell_h,
        fill: None,
        stroke: Some(StrokeStyle::hairline(CELL_BORDER)),
    });

    // Metric lines (all subtle)
    if show_metrics {
        let stroke_metric = StrokeStyle::new(0.25, METRIC_LINES);
        let stroke_baseline = StrokeStyle::new(0.25, METRIC_BASELINE);

        // Ascender
        let asc_y = baseline_y - ascent;
        allocator.push_command(DrawCommand::Line {
            x1: cell_x, y1: asc_y, x2: cell_x + cell_w, y2: asc_y,
            stroke: stroke_metric,
        });

        // Cap-height
        if let Some(ch) = cap_height {
            let ch_y = baseline_y - ch;
            allocator.push_command(DrawCommand::Line {
                x1: cell_x, y1: ch_y, x2: cell_x + cell_w, y2: ch_y,
                stroke: stroke_metric,
            });
        }

        // x-height
        if let Some(xh) = x_height {
            let xh_y = baseline_y - xh;
            allocator.push_command(DrawCommand::Line {
                x1: cell_x, y1: xh_y, x2: cell_x + cell_w, y2: xh_y,
                stroke: stroke_metric,
            });
        }

        // Baseline (slightly darker)
        allocator.push_command(DrawCommand::Line {
            x1: cell_x, y1: baseline_y, x2: cell_x + cell_w, y2: baseline_y,
            stroke: stroke_baseline,
        });

        // Descender
        let desc_y = baseline_y + descent;
        allocator.push_command(DrawCommand::Line {
            x1: cell_x, y1: desc_y, x2: cell_x + cell_w, y2: desc_y,
            stroke: stroke_metric,
        });

        // Side bearings (vertical lines at glyph origin and origin+advance)
        let glyph_area_top = baseline_y - ascent - 2.0;
        let glyph_area_bottom = baseline_y + descent + 2.0;

        // Left side bearing
        allocator.push_command(DrawCommand::Line {
            x1: origin_x, y1: glyph_area_top, x2: origin_x, y2: glyph_area_bottom,
            stroke: stroke_metric,
        });

        // Right side bearing (at advance width)
        let rsb_x = origin_x + cell.advance;
        allocator.push_command(DrawCommand::Line {
            x1: rsb_x, y1: glyph_area_top, x2: rsb_x, y2: glyph_area_bottom,
            stroke: stroke_metric,
        });
    }

    // Glyph
    allocator.push_command(DrawCommand::GlyphRun {
        font_id,
        size: font_size,
        glyphs: vec![PositionedGlyph {
            glyph_id: cell.glyph_id,
            x: origin_x,
            y: baseline_y,
        }],
        text: cell.codepoint.map(|c| c.to_string()).unwrap_or_default(),
        variations: vec![],
    });

    // Label (centered below glyph area)
    if show_names {
        let label_x = cell_x + (cell_w - cell.label_width) / 2.0;
        let label_y = cell_y + cell_h - 2.0;
        allocator.push_command(DrawCommand::Label {
            text: cell.label.clone(),
            x: label_x,
            y: label_y,
            size: LABEL_SIZE,
            color: LABEL_COLOR,
        });
    }
}
