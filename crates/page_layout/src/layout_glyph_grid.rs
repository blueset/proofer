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
use skrifa::MetadataProvider;

use crate::{LayoutError, PageAllocator};

// ── Colors (subtle, non-distracting) ────────────────────────────────

const METRIC_BASELINE: Color = Color { r: 0.75, g: 0.75, b: 0.75, a: 1.0 };
const METRIC_LINES: Color = Color { r: 0.82, g: 0.82, b: 0.82, a: 1.0 };
const LABEL_COLOR: Color = Color { r: 0.5, g: 0.5, b: 0.5, a: 1.0 };
const LABEL_SIZE: f32 = 5.0;
const ROW_GAP: f32 = 8.0;

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
    compute_cell_data_with_location(glyphs, font_id, font_size, skrifa::prelude::LocationRef::default(), registry)
}

fn compute_cell_data_with_location(
    glyphs: &[GlyphInfo],
    font_id: FontId,
    font_size: f32,
    location: skrifa::prelude::LocationRef<'_>,
    registry: &FontRegistry,
) -> Vec<CellData> {
    use skrifa::MetadataProvider;

    let font = match registry.font_ref(font_id) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };

    let glyph_metrics = font.glyph_metrics(skrifa::prelude::Size::unscaled(), location);
    let upm = font.metrics(skrifa::prelude::Size::unscaled(), location).units_per_em as f32;
    let scale = font_size / upm;
    let label_font_id = registry.label_font_id();

    glyphs
        .iter()
        .map(|g| {
            let gid = skrifa::GlyphId::new(g.glyph_id);
            let advance_raw = glyph_metrics.advance_width(gid).unwrap_or(0.0);
            let lsb_raw = glyph_metrics.left_side_bearing(gid).unwrap_or(0.0);
            let bbox = glyph_metrics.bounds(gid);

            let advance = advance_raw * scale;
            let lsb = lsb_raw * scale;

            let (ink_left, ink_right, ink_top, ink_bottom) = if let Some(bbox) = bbox {
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

    // Build variation location for variation-aware metrics
    let font = registry.font_ref(font_id).map_err(|e| LayoutError::Font(e))?;
    let var_settings: Vec<(&str, f32)> = design_attrs
        .variations
        .iter()
        .map(|(tag, val)| (tag.as_str(), *val))
        .collect();
    let location = font.axes().location(var_settings.iter().copied());
    let loc_ref: skrifa::prelude::LocationRef<'_> = (&location).into();

    let font_size = design_attrs.font_size;
    let size = skrifa::prelude::Size::new(font_size);
    let metrics = font.metrics(skrifa::prelude::Size::unscaled(), loc_ref);
    let scale = font_size / metrics.units_per_em as f32;
    let ascent = metrics.ascent * scale;
    let descent = -metrics.descent * scale;
    let cap_height = metrics.cap_height.map(|h| h * scale);
    let x_height = metrics.x_height.map(|h| h * scale);
    let label_height = if show_names { LABEL_SIZE + 2.0 } else { 0.0 };

    // Variation vec for DrawCommands
    let var_vec: Vec<(String, f32)> = design_attrs.variations.iter().map(|(k, v)| (k.clone(), *v)).collect();

    let cells = compute_cell_data_with_location(&glyphs, font_id, font_size, loc_ref, registry);

    match mode {
        GlyphGridMode::Grid => layout_grid(
            allocator, &cells, font_id, font_size, ascent, descent,
            cap_height, x_height, show_metrics, show_names, label_height, cell_padding, &var_vec,
        ),
        GlyphGridMode::Compact => layout_compact(
            allocator, &cells, font_id, font_size, ascent, descent,
            cap_height, x_height, show_metrics, show_names, label_height, cell_padding, &var_vec,
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
    variations: &[(String, f32)],
) -> Result<(), LayoutError> {
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

    // Fixed baseline offset for uniform row alignment
    let max_top_extent = cells.iter()
        .map(|c| c.ink_top.max(ascent))
        .fold(0.0_f32, f32::max);

    let mut glyph_idx = 0;
    while glyph_idx < cells.len() {
        allocator.ensure_space(cell_height + ROW_GAP);
        let row_y = allocator.cursor_y();
        let row_end = (glyph_idx + cols).min(cells.len());

        // All glyphs in the row share the same baseline
        let baseline_y = row_y + padding + max_top_extent;

        for col in 0..(row_end - glyph_idx) {
            let cell = &cells[glyph_idx + col];
            let cell_x = allocator.body_left() + col as f32 * actual_cell_width;

            // Glyph origin x: center the advance width in the content area
            let origin_x = cell_x + padding + (content_area_w - cell.advance) / 2.0;

            render_cell(
                allocator, cell, cell_x, row_y, actual_cell_width, cell_height,
                baseline_y, origin_x, font_id, font_size,
                ascent, descent, cap_height, x_height,
                show_metrics, show_names, label_height, padding, variations,
            );
        }

        glyph_idx = row_end;
        allocator.advance(cell_height + ROW_GAP);
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
    variations: &[(String, f32)],
) -> Result<(), LayoutError> {
    let min_cell_width = font_size * 0.5;
    let body_width = allocator.body_width();
    let base_gap = font_size * 0.3; // minimum gap between cells

    // Pre-compute per-cell natural dimensions
    let cell_dims: Vec<(f32, f32)> = cells
        .iter()
        .map(|cell| {
            let inner_w = cell.content_width().max(cell.label_width).max(min_cell_width);
            let w = inner_w + padding * 2.0;
            let h = cell.content_height(ascent, descent) + padding * 2.0 + label_height;
            (w, h)
        })
        .collect();

    // First pass: bin-pack ALL cells into rows (with base_gap between cells)
    struct RowInfo {
        indices: Vec<usize>,
        natural_width: f32, // sum of cell widths (no gaps)
        height: f32,
    }

    let mut rows: Vec<RowInfo> = Vec::new();
    let mut cell_idx = 0;
    while cell_idx < cells.len() {
        let mut indices = Vec::new();
        let mut row_width = 0.0;
        let mut row_height: f32 = 0.0;

        while cell_idx < cells.len() {
            let (w, h) = cell_dims[cell_idx];
            let needed = if indices.is_empty() { w } else { w + base_gap };
            if row_width + needed > body_width && !indices.is_empty() {
                break;
            }
            indices.push(cell_idx);
            row_width += needed;
            row_height = row_height.max(h);
            cell_idx += 1;
        }

        let natural_width: f32 = indices.iter().map(|&i| cell_dims[i].0).sum();
        rows.push(RowInfo { indices, natural_width, height: row_height });
    }

    let total_rows = rows.len();
    let is_single_line = total_rows == 1;

    // Second pass: paginate rows and render with vertical centering per page
    let mut row_idx = 0;
    while row_idx < rows.len() {
        // Collect rows that fit on this page
        let available = allocator.remaining_height();
        let mut page_rows: Vec<usize> = Vec::new();
        let mut page_content_height: f32 = 0.0;

        while row_idx < rows.len() {
            let rh = rows[row_idx].height;
            let needed = if page_rows.is_empty() { rh } else { rh + ROW_GAP };
            if page_content_height + needed > available && !page_rows.is_empty() {
                break;
            }
            page_rows.push(row_idx);
            page_content_height += needed;
            row_idx += 1;
        }

        if page_rows.is_empty() {
            allocator.new_page();
            continue;
        }

        // Vertical centering: even space before first and after last row
        let vertical_padding = (available - page_content_height).max(0.0) / 2.0;
        allocator.advance(vertical_padding);

        for (page_row_i, &ri) in page_rows.iter().enumerate() {
            let row = &rows[ri];
            let row_y = allocator.cursor_y();
            let body_left = allocator.body_left();

            // Baseline alignment for this row
            let max_top_extent = row.indices.iter()
                .map(|&idx| cells[idx].ink_top.max(ascent))
                .fold(0.0_f32, f32::max);
            let baseline_y = row_y + padding + max_top_extent;

            let n = row.indices.len();

            // Determine horizontal positioning
            let (cell_positions, effective_cell_widths) = if is_single_line {
                // Single line: center with uniform gap
                let total_natural: f32 = row.indices.iter().map(|&i| cell_dims[i].0).sum();
                let total_gaps = if n > 1 { (n - 1) as f32 * base_gap } else { 0.0 };
                let total_with_gaps = total_natural + total_gaps;
                let start_x = body_left + (body_width - total_with_gaps).max(0.0) / 2.0;

                let mut positions = Vec::with_capacity(n);
                let mut widths = Vec::with_capacity(n);
                let mut x = start_x;
                for (i, &idx) in row.indices.iter().enumerate() {
                    positions.push(x);
                    widths.push(cell_dims[idx].0);
                    x += cell_dims[idx].0;
                    if i + 1 < n { x += base_gap; }
                }
                (positions, widths)
            } else if ri < rows.len() - 1 {
                // Multi-line, not last row: justify (stretch gaps to fill body_width)
                let extra_space = body_width - row.natural_width;
                let gap = if n > 1 { extra_space / (n - 1) as f32 } else { 0.0 };

                let mut positions = Vec::with_capacity(n);
                let mut widths = Vec::with_capacity(n);
                let mut x = body_left;
                for (i, &idx) in row.indices.iter().enumerate() {
                    positions.push(x);
                    widths.push(cell_dims[idx].0);
                    x += cell_dims[idx].0;
                    if i + 1 < n { x += gap; }
                }
                (positions, widths)
            } else {
                // Last row of multi-line: left-aligned with base_gap
                let mut positions = Vec::with_capacity(n);
                let mut widths = Vec::with_capacity(n);
                let mut x = body_left;
                for (i, &idx) in row.indices.iter().enumerate() {
                    positions.push(x);
                    widths.push(cell_dims[idx].0);
                    x += cell_dims[idx].0;
                    if i + 1 < n { x += base_gap; }
                }
                (positions, widths)
            };

            // Render cells at computed positions
            for (i, &idx) in row.indices.iter().enumerate() {
                let cell = &cells[idx];
                let cell_x = cell_positions[i];
                let cell_w = effective_cell_widths[i];
                let content_area_w = cell_w - padding * 2.0;
                let origin_x = cell_x + padding + (content_area_w - cell.advance) / 2.0;

                render_cell(
                    allocator, cell, cell_x, row_y, cell_w, row.height,
                    baseline_y, origin_x, font_id, font_size,
                    ascent, descent, cap_height, x_height,
                    show_metrics, show_names, label_height, padding, variations,
                );
            }

            allocator.advance(row.height);
            if page_row_i + 1 < page_rows.len() {
                allocator.advance(ROW_GAP);
            }
        }

        // Advance past the bottom vertical padding
        allocator.advance(vertical_padding);

        // Start new page if more rows remain
        if row_idx < rows.len() {
            allocator.new_page();
        }
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
    variations: &[(String, f32)],
) {
    // Metric lines — only span the glyph's extent
    if show_metrics {
        let stroke_metric = StrokeStyle::new(0.25, METRIC_LINES);
        let stroke_baseline = StrokeStyle::new(0.25, METRIC_BASELINE);

        // Horizontal extent: from leftmost of (origin, ink_left) to rightmost of (origin+advance, ink_right)
        let line_left = origin_x + cell.ink_left.min(0.0);
        let line_right = (origin_x + cell.ink_right).max(origin_x + cell.advance);

        // Ascender
        let asc_y = baseline_y - ascent;
        allocator.push_command(DrawCommand::Line {
            x1: line_left, y1: asc_y, x2: line_right, y2: asc_y,
            stroke: stroke_metric,
        });

        // Cap-height
        if let Some(ch) = cap_height {
            let ch_y = baseline_y - ch;
            allocator.push_command(DrawCommand::Line {
                x1: line_left, y1: ch_y, x2: line_right, y2: ch_y,
                stroke: stroke_metric,
            });
        }

        // x-height
        if let Some(xh) = x_height {
            let xh_y = baseline_y - xh;
            allocator.push_command(DrawCommand::Line {
                x1: line_left, y1: xh_y, x2: line_right, y2: xh_y,
                stroke: stroke_metric,
            });
        }

        // Baseline (slightly darker)
        allocator.push_command(DrawCommand::Line {
            x1: line_left, y1: baseline_y, x2: line_right, y2: baseline_y,
            stroke: stroke_baseline,
        });

        // Descender
        let desc_y = baseline_y + descent;
        allocator.push_command(DrawCommand::Line {
            x1: line_left, y1: desc_y, x2: line_right, y2: desc_y,
            stroke: stroke_metric,
        });

        // Side bearings (vertical lines at glyph origin and origin+advance)
        let glyph_area_top = baseline_y - ascent - 2.0;
        let glyph_area_bottom = baseline_y + descent + 2.0;

        allocator.push_command(DrawCommand::Line {
            x1: origin_x, y1: glyph_area_top, x2: origin_x, y2: glyph_area_bottom,
            stroke: stroke_metric,
        });

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
        variations: variations.to_vec(),
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
