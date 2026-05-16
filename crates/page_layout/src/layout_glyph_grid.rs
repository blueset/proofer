//! Glyph Grid layout — grid and compact modes with metric lines.
//!
//! Each glyph is shown in its own cell with:
//! - The glyph shape at the configured font size
//! - Horizontal metric lines (ascender, cap-height, baseline, descender)
//! - Side bearing indicators (left/right at glyph origin and advance)
//! - Glyph name label below the glyph area
//!
//! Grid mode: uniform cell size, table-aligned.
//! Compact mode: per-glyph variable width, greedy row packing.
//!
//! Optionally, each glyph cell can be rendered as a **subgrid** of variation
//! instances by supplying `subgrid_x` and/or `subgrid_y`. Each sub-cell shows
//! the same glyph at a different point in design space; a single shared label
//! is drawn beneath the matrix. Variation tags listed in a subgrid override
//! matching tags in `design_attrs.variations`; tags not specified by a subgrid
//! fall back to `design_attrs.variations`, then to the font's axis defaults.

use std::collections::BTreeMap;

use content_resolver::ResolvedContent;
use font_inspector::{FontInspector, GlyphInfo};
use font_registry::{FontId, FontRegistry};
use layout_ir::{Color, DrawCommand, PositionedGlyph, StrokeStyle};
use proof_model::{DesignAttributes, GlyphGridMode, SubgridAxis};
use skrifa::raw::TableProvider;
use skrifa::MetadataProvider;

use crate::{LayoutError, PageAllocator};

// ── Colors and constants ────────────────────────────────────────────

const METRIC_BASELINE: Color = Color { r: 0.75, g: 0.75, b: 0.75, a: 1.0 };
const METRIC_LINES: Color = Color { r: 0.82, g: 0.82, b: 0.82, a: 1.0 };
const LABEL_COLOR: Color = Color { r: 0.5, g: 0.5, b: 0.5, a: 1.0 };
const LABEL_SIZE: f32 = 5.0;
const ROW_GAP: f32 = 8.0;
const SLNT_AXIS_TAG: &str = "slnt";
const SLANT_EPSILON: f32 = 0.001;
/// Spacing between sub-cells within a single glyph's matrix, as a fraction
/// of the proof's font size.
const SUB_GAP_FRACTION: f32 = 0.15;

// ── Per-(glyph, sub-cell) ink/advance data ─────────────────────────

#[derive(Clone)]
struct SubCellData {
    /// Advance width in points.
    advance: f32,
    /// Left edge of metric extent relative to origin (≤ 0).
    ink_left: f32,
    /// Right edge of metric extent relative to origin (≥ advance).
    ink_right: f32,
    /// Top edge of ink above baseline, in points (positive up).
    ink_top: f32,
    /// Bottom edge of ink below baseline, in points (negative).
    ink_bottom: f32,
}

impl SubCellData {
    /// Width of the metric / ink extent in points.
    fn extent_width(&self) -> f32 {
        self.ink_right - self.ink_left
    }
}

/// Per-(row, column) font metrics evaluated at a sub-cell's variation location.
#[derive(Clone, Copy)]
struct SubMetrics {
    ascent: f32,
    descent: f32,
    cap_height: Option<f32>,
    x_height: Option<f32>,
    slant: Option<MetricSlant>,
}

/// Slant geometry for metric guides in a single sub-cell.
#[derive(Clone, Copy)]
struct MetricSlant {
    slope: f32,
    caret_offset: f32,
}

impl MetricSlant {
    fn new(angle_degrees: f32, caret_offset: f32) -> Option<Self> {
        if angle_degrees.abs() <= SLANT_EPSILON {
            return None;
        }

        let slope = angle_degrees.to_radians().tan();
        Self::from_slope(slope, caret_offset)
    }

    fn from_slope(slope: f32, caret_offset: f32) -> Option<Self> {
        if slope.abs() <= SLANT_EPSILON {
            return None;
        }

        slope.is_finite().then_some(Self { slope, caret_offset })
    }

    fn x_at_y(&self, base_x: f32, baseline_y: f32, guide_y: f32) -> f32 {
        let y_above_baseline = baseline_y - guide_y;
        base_x + self.caret_offset + self.slope * y_above_baseline
    }
}

/// Per-glyph data: identity, label, and the R×C matrix of sub-cell ink data.
struct GlyphCellData {
    glyph_id: u32,
    codepoint: Option<char>,
    label: String,
    label_width: f32,
    /// `[n_rows][n_cols]` sub-cell metrics for this glyph.
    sub: Vec<Vec<SubCellData>>,
}

// ── Variation matrix construction ───────────────────────────────────

/// Build `[n_rows][n_cols]` variation lists by merging the base
/// `design_attrs.variations` with per-row/column subgrid overrides.
///
/// Subgrid axis values silently override matching tags in the base.
fn build_variation_matrix(
    base: &BTreeMap<String, f32>,
    subgrid_x: Option<&SubgridAxis>,
    subgrid_y: Option<&SubgridAxis>,
) -> Vec<Vec<Vec<(String, f32)>>> {
    let n_rows = subgrid_y.map(|s| s.values.len()).unwrap_or(1);
    let n_cols = subgrid_x.map(|s| s.values.len()).unwrap_or(1);

    let mut matrix = Vec::with_capacity(n_rows);
    for r in 0..n_rows {
        let mut row = Vec::with_capacity(n_cols);
        for c in 0..n_cols {
            let mut merged: BTreeMap<String, f32> = base.clone();
            if let Some(sy) = subgrid_y {
                merged.insert(sy.axis.clone(), sy.values[r]);
            }
            if let Some(sx) = subgrid_x {
                merged.insert(sx.axis.clone(), sx.values[c]);
            }
            row.push(merged.into_iter().collect());
        }
        matrix.push(row);
    }
    matrix
}

fn slnt_axis_angle(variations: &[(String, f32)]) -> Option<f32> {
    variations
        .iter()
        .find(|(tag, _)| tag == SLNT_AXIS_TAG)
        .map(|(_, value)| *value)
}

fn metric_slant_for(
    variations: &[(String, f32)],
    fallback_angle: Option<f32>,
    caret_offset: f32,
    caret_slope: Option<f32>,
) -> Option<MetricSlant> {
    match slnt_axis_angle(variations) {
        // OpenType slnt uses negative values for right-leaning designs; the
        // guide geometry uses positive angles to move tops rightward. Prefer
        // MVAR's varied caret slope when present because it is the font's
        // exact instance metric for this geometry.
        Some(slnt_angle) => caret_slope
            .and_then(|slope| MetricSlant::from_slope(slope, caret_offset))
            .or_else(|| MetricSlant::new(-slnt_angle, caret_offset)),
        None => MetricSlant::new(fallback_angle?, caret_offset),
    }
}

fn mvar_metric_delta(
    font: &skrifa::prelude::FontRef<'_>,
    tag: skrifa::Tag,
    coords: &[skrifa::instance::NormalizedCoord],
) -> f32 {
    font.mvar()
        .ok()
        .and_then(|mvar| mvar.metric_delta(tag, coords).ok())
        .map(|delta| delta.to_f64() as f32)
        .unwrap_or(0.0)
}

fn varied_caret_slope(
    font: &skrifa::prelude::FontRef<'_>,
    coords: &[skrifa::instance::NormalizedCoord],
    base_rise: f32,
    base_run: f32,
) -> Option<f32> {
    let rise = base_rise
        + mvar_metric_delta(font, skrifa::raw::tables::mvar::tags::HCRS, coords);
    if rise.abs() <= SLANT_EPSILON {
        return None;
    }

    let run = base_run
        + mvar_metric_delta(font, skrifa::raw::tables::mvar::tags::HCRN, coords);
    MetricSlant::from_slope(run / rise, 0.0).map(|slant| slant.slope)
}

// ── Per-(glyph, sub-cell) metric computation ────────────────────────

/// Compute `GlyphCellData` for every glyph, evaluating ink/advance under
/// each sub-cell's variation location. `locations` is `[n_rows][n_cols]`.
fn compute_glyph_cells(
    glyphs: &[GlyphInfo],
    font_id: FontId,
    font_size: f32,
    locations: &[Vec<skrifa::instance::Location>],
    registry: &FontRegistry,
) -> Vec<GlyphCellData> {
    let font = match registry.font_ref(font_id) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };

    let n_rows = locations.len();
    let n_cols = locations.first().map(|r: &Vec<_>| r.len()).unwrap_or(0);
    if n_rows == 0 || n_cols == 0 {
        return Vec::new();
    }

    // Pre-build glyph_metrics + scale per (r, c) so we don't redo per glyph.
    let mut sub_metrics: Vec<Vec<(skrifa::metrics::GlyphMetrics<'_>, f32)>> =
        Vec::with_capacity(n_rows);
    for row_locs in locations {
        let mut row = Vec::with_capacity(n_cols);
        for loc in row_locs {
            let loc_ref = skrifa::prelude::LocationRef::from(loc);
            let gm = font.glyph_metrics(skrifa::prelude::Size::unscaled(), loc_ref);
            let upm = font
                .metrics(skrifa::prelude::Size::unscaled(), loc_ref)
                .units_per_em as f32;
            row.push((gm, font_size / upm));
        }
        sub_metrics.push(row);
    }

    let label_font_id = registry.label_font_id();

    glyphs
        .iter()
        .map(|g| {
            let gid = skrifa::GlyphId::new(g.glyph_id);

            let mut sub: Vec<Vec<SubCellData>> = Vec::with_capacity(n_rows);
            for row in &sub_metrics {
                let mut sub_row = Vec::with_capacity(n_cols);
                for (gm, scale) in row {
                    let advance_raw = gm.advance_width(gid).unwrap_or(0.0);
                    let lsb_raw = gm.left_side_bearing(gid).unwrap_or(0.0);
                    let bbox = gm.bounds(gid);

                    let advance = advance_raw * scale;
                    let lsb = lsb_raw * scale;

                    let (ink_left, ink_right, ink_top, ink_bottom) =
                        if let Some(bbox) = bbox {
                            (
                                (bbox.x_min * scale).min(0.0),
                                (bbox.x_max * scale).max(advance),
                                bbox.y_max * scale,
                                bbox.y_min * scale,
                            )
                        } else {
                            (0.0_f32.min(lsb), advance, 0.0, 0.0)
                        };

                    sub_row.push(SubCellData {
                        advance,
                        ink_left,
                        ink_right,
                        ink_top,
                        ink_bottom,
                    });
                }
                sub.push(sub_row);
            }

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

            GlyphCellData {
                glyph_id: g.glyph_id,
                codepoint: g.codepoint,
                label,
                label_width,
                sub,
            }
        })
        .collect()
}

// ── Public entry point ──────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
pub fn layout(
    allocator: &mut PageAllocator,
    content: &ResolvedContent,
    design_attrs: &DesignAttributes,
    mode: &GlyphGridMode,
    show_metrics: bool,
    show_names: bool,
    cell_padding: f32,
    subgrid_x: Option<&SubgridAxis>,
    subgrid_y: Option<&SubgridAxis>,
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

    let font = registry.font_ref(font_id).map_err(LayoutError::Font)?;
    let font_size = design_attrs.font_size;
    let label_height = if show_names { LABEL_SIZE + 2.0 } else { 0.0 };
    let sub_gap = font_size * SUB_GAP_FRACTION;

    // Build the [n_rows][n_cols] variation matrix and corresponding Locations.
    let variation_matrix =
        build_variation_matrix(&design_attrs.variations, subgrid_x, subgrid_y);
    let n_rows = variation_matrix.len();
    let n_cols = variation_matrix.first().map(|r| r.len()).unwrap_or(0);

    let locations: Vec<Vec<skrifa::instance::Location>> = variation_matrix
        .iter()
        .map(|row| {
            row.iter()
                .map(|vars| {
                    let pairs: Vec<(&str, f32)> =
                        vars.iter().map(|(t, v)| (t.as_str(), *v)).collect();
                    font.axes().location(pairs.iter().copied())
                })
                .collect()
        })
        .collect();

    let fallback_italic_angle = font
        .post()
        .ok()
        .map(|post| post.italic_angle().to_f32())
        .filter(|angle| angle.abs() > SLANT_EPSILON);
    let caret_offset_units = font
        .hhea()
        .ok()
        .map(|hhea| hhea.caret_offset() as f32)
        .unwrap_or(0.0);
    let (caret_slope_rise_units, caret_slope_run_units) = font
        .hhea()
        .ok()
        .map(|hhea| {
            (
                hhea.caret_slope_rise() as f32,
                hhea.caret_slope_run() as f32,
            )
        })
        .unwrap_or((1.0, 0.0));

    // Per-(r, c) font metrics evaluated at each sub-cell's location.
    let mut metrics_matrix: Vec<Vec<SubMetrics>> = Vec::with_capacity(n_rows);
    for (row_index, row_locs) in locations.iter().enumerate() {
        let mut row = Vec::with_capacity(n_cols);
        for (col_index, loc) in row_locs.iter().enumerate() {
            let loc_ref = skrifa::prelude::LocationRef::from(loc);
            let m = font.metrics(skrifa::prelude::Size::unscaled(), loc_ref);
            let scale = font_size / m.units_per_em as f32;
            let caret_offset = (caret_offset_units
                + mvar_metric_delta(
                    &font,
                    skrifa::raw::tables::mvar::tags::HCOF,
                    loc_ref.coords(),
                )) * scale;
            let caret_slope = varied_caret_slope(
                &font,
                loc_ref.coords(),
                caret_slope_rise_units,
                caret_slope_run_units,
            );
            row.push(SubMetrics {
                ascent: m.ascent * scale,
                descent: -m.descent * scale,
                cap_height: m.cap_height.map(|h| h * scale),
                x_height: m.x_height.map(|h| h * scale),
                slant: metric_slant_for(
                    &variation_matrix[row_index][col_index],
                    fallback_italic_angle,
                    caret_offset,
                    caret_slope,
                ),
            });
        }
        metrics_matrix.push(row);
    }

    let cells = compute_glyph_cells(&glyphs, font_id, font_size, &locations, registry);
    if cells.is_empty() {
        return Ok(());
    }

    match mode {
        GlyphGridMode::Grid => layout_grid(
            allocator,
            &cells,
            font_id,
            font_size,
            &metrics_matrix,
            &variation_matrix,
            n_rows,
            n_cols,
            sub_gap,
            show_metrics,
            show_names,
            label_height,
            cell_padding,
        ),
        GlyphGridMode::Compact => layout_compact(
            allocator,
            &cells,
            font_id,
            font_size,
            &metrics_matrix,
            &variation_matrix,
            n_rows,
            n_cols,
            sub_gap,
            show_metrics,
            show_names,
            label_height,
            cell_padding,
        ),
    }
}

// ── Sub-grid sizing helpers ─────────────────────────────────────────

/// Per-row top/bottom extents and sub-row heights, computed from the supplied
/// glyph indices (page-global in Grid mode; just one glyph in Compact mode).
struct RowExtents {
    /// Per sub-row top extent (max of ink_top and ascent over considered cells).
    top: Vec<f32>,
    /// Per sub-row bottom extent (max of -ink_bottom and descent).
    bottom: Vec<f32>,
}

impl RowExtents {
    fn row_height(&self, r: usize) -> f32 {
        self.top[r] + self.bottom[r]
    }

    fn total_height(&self, sub_gap: f32) -> f32 {
        let n = self.top.len();
        let sum: f32 = (0..n).map(|r| self.row_height(r)).sum();
        sum + (n.saturating_sub(1) as f32) * sub_gap
    }
}

fn compute_row_extents<'a>(
    cells: impl IntoIterator<Item = &'a GlyphCellData>,
    metrics_matrix: &[Vec<SubMetrics>],
    n_rows: usize,
    n_cols: usize,
) -> RowExtents {
    let mut top = vec![0.0_f32; n_rows];
    let mut bottom = vec![0.0_f32; n_rows];
    let cell_list: Vec<&GlyphCellData> = cells.into_iter().collect();
    for r in 0..n_rows {
        for c in 0..n_cols {
            let m = metrics_matrix[r][c];
            for cell in &cell_list {
                let s = &cell.sub[r][c];
                top[r] = top[r].max(s.ink_top.max(m.ascent));
                bottom[r] = bottom[r].max((-s.ink_bottom).max(m.descent));
            }
        }
    }
    RowExtents { top, bottom }
}

fn compute_col_widths<'a>(
    cells: impl IntoIterator<Item = &'a GlyphCellData>,
    n_rows: usize,
    n_cols: usize,
) -> Vec<f32> {
    let mut widths = vec![0.0_f32; n_cols];
    let cell_list: Vec<&GlyphCellData> = cells.into_iter().collect();
    for c in 0..n_cols {
        for r in 0..n_rows {
            for cell in &cell_list {
                widths[c] = widths[c].max(cell.sub[r][c].extent_width());
            }
        }
    }
    widths
}

fn matrix_natural_width(col_widths: &[f32], sub_gap: f32) -> f32 {
    let sum: f32 = col_widths.iter().sum();
    sum + (col_widths.len().saturating_sub(1) as f32) * sub_gap
}

// ── Grid mode ───────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn layout_grid(
    allocator: &mut PageAllocator,
    cells: &[GlyphCellData],
    font_id: FontId,
    font_size: f32,
    metrics_matrix: &[Vec<SubMetrics>],
    variation_matrix: &[Vec<Vec<(String, f32)>>],
    n_rows: usize,
    n_cols: usize,
    sub_gap: f32,
    show_metrics: bool,
    show_names: bool,
    label_height: f32,
    padding: f32,
) -> Result<(), LayoutError> {
    // Page-global sub-row / sub-column sizing.
    let row_extents = compute_row_extents(cells.iter(), metrics_matrix, n_rows, n_cols);
    let col_widths = compute_col_widths(cells.iter(), n_rows, n_cols);
    let max_label_w = cells.iter().map(|c| c.label_width).fold(0.0_f32, f32::max);

    let matrix_w = matrix_natural_width(&col_widths, sub_gap);
    let matrix_h = row_extents.total_height(sub_gap);

    let min_cell_width = font_size * 0.5;
    let cell_inner_w = matrix_w.max(max_label_w).max(min_cell_width);
    let cell_width = cell_inner_w + padding * 2.0;
    let cell_height = matrix_h + padding * 2.0 + label_height;

    let body_width = allocator.body_width();
    let cols = (body_width / cell_width).floor().max(1.0) as usize;
    let actual_cell_width = body_width / cols as f32;
    let content_area_w = actual_cell_width - padding * 2.0;

    let mut glyph_idx = 0;
    while glyph_idx < cells.len() {
        allocator.ensure_space(cell_height + ROW_GAP);
        let row_y = allocator.cursor_y();
        let row_end = (glyph_idx + cols).min(cells.len());

        for col in 0..(row_end - glyph_idx) {
            let cell = &cells[glyph_idx + col];
            let cell_x = allocator.body_left() + col as f32 * actual_cell_width;
            render_glyph_cell(
                allocator,
                cell,
                cell_x,
                row_y,
                actual_cell_width,
                cell_height,
                content_area_w,
                &col_widths,
                &row_extents,
                metrics_matrix,
                variation_matrix,
                font_id,
                font_size,
                sub_gap,
                show_metrics,
                show_names,
                padding,
            );
        }

        glyph_idx = row_end;
        allocator.advance(cell_height + ROW_GAP);
    }

    Ok(())
}

// ── Compact mode ────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn layout_compact(
    allocator: &mut PageAllocator,
    cells: &[GlyphCellData],
    font_id: FontId,
    font_size: f32,
    metrics_matrix: &[Vec<SubMetrics>],
    variation_matrix: &[Vec<Vec<(String, f32)>>],
    n_rows: usize,
    n_cols: usize,
    sub_gap: f32,
    show_metrics: bool,
    show_names: bool,
    label_height: f32,
    padding: f32,
) -> Result<(), LayoutError> {
    let min_cell_width = font_size * 0.5;
    let body_width = allocator.body_width();
    let base_gap = font_size * 0.3;

    // Per-glyph natural dims (matrix sized to that single glyph) and per-glyph
    // row extents (used at render time to compute baselines).
    struct GlyphDims {
        outer_w: f32,
        outer_h: f32,
        col_widths: Vec<f32>,
    }

    let glyph_dims: Vec<GlyphDims> = cells
        .iter()
        .map(|cell| {
            let row_extents = compute_row_extents(
                std::iter::once(cell),
                metrics_matrix,
                n_rows,
                n_cols,
            );
            let col_widths =
                compute_col_widths(std::iter::once(cell), n_rows, n_cols);
            let matrix_w = matrix_natural_width(&col_widths, sub_gap);
            let matrix_h = row_extents.total_height(sub_gap);
            let inner_w = matrix_w.max(cell.label_width).max(min_cell_width);
            GlyphDims {
                outer_w: inner_w + padding * 2.0,
                outer_h: matrix_h + padding * 2.0 + label_height,
                col_widths,
            }
        })
        .collect();

    // Bin-pack outer cells into rows.
    struct RowInfo {
        indices: Vec<usize>,
        natural_width: f32,
        height: f32,
        row_extents: RowExtents,
    }
    let mut rows: Vec<RowInfo> = Vec::new();
    let mut cell_idx = 0;
    while cell_idx < cells.len() {
        let mut indices = Vec::new();
        let mut row_width = 0.0;
        let mut row_height: f32 = 0.0;

        while cell_idx < cells.len() {
            let w = glyph_dims[cell_idx].outer_w;
            let h = glyph_dims[cell_idx].outer_h;
            let needed = if indices.is_empty() { w } else { w + base_gap };
            if row_width + needed > body_width && !indices.is_empty() {
                break;
            }
            indices.push(cell_idx);
            row_width += needed;
            row_height = row_height.max(h);
            cell_idx += 1;
        }

        let natural_width: f32 = indices.iter().map(|&i| glyph_dims[i].outer_w).sum();
        let row_extents = compute_row_extents(
            indices.iter().map(|&i| &cells[i]),
            metrics_matrix,
            n_rows,
            n_cols,
        );
        let shared_matrix_h = row_extents.total_height(sub_gap);
        row_height = row_height.max(shared_matrix_h + padding * 2.0 + label_height);
        rows.push(RowInfo { indices, natural_width, height: row_height, row_extents });
    }

    let total_rows = rows.len();
    let is_single_line = total_rows == 1;

    // Paginate rows; vertically center per page.
    let mut row_idx = 0;
    while row_idx < rows.len() {
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

        let vertical_padding = (available - page_content_height).max(0.0) / 2.0;
        allocator.advance(vertical_padding);

        for (page_row_i, &ri) in page_rows.iter().enumerate() {
            let row = &rows[ri];
            let row_y = allocator.cursor_y();
            let body_left = allocator.body_left();
            let n = row.indices.len();

            let (cell_positions, effective_cell_widths) = if is_single_line {
                let total_natural: f32 =
                    row.indices.iter().map(|&i| glyph_dims[i].outer_w).sum();
                let total_gaps = if n > 1 { (n - 1) as f32 * base_gap } else { 0.0 };
                let total_with_gaps = total_natural + total_gaps;
                let start_x =
                    body_left + (body_width - total_with_gaps).max(0.0) / 2.0;

                let mut positions = Vec::with_capacity(n);
                let mut widths = Vec::with_capacity(n);
                let mut x = start_x;
                for (i, &idx) in row.indices.iter().enumerate() {
                    positions.push(x);
                    widths.push(glyph_dims[idx].outer_w);
                    x += glyph_dims[idx].outer_w;
                    if i + 1 < n {
                        x += base_gap;
                    }
                }
                (positions, widths)
            } else if ri < rows.len() - 1 {
                let extra_space = body_width - row.natural_width;
                let gap = if n > 1 { extra_space / (n - 1) as f32 } else { 0.0 };

                let mut positions = Vec::with_capacity(n);
                let mut widths = Vec::with_capacity(n);
                let mut x = body_left;
                for (i, &idx) in row.indices.iter().enumerate() {
                    positions.push(x);
                    widths.push(glyph_dims[idx].outer_w);
                    x += glyph_dims[idx].outer_w;
                    if i + 1 < n {
                        x += gap;
                    }
                }
                (positions, widths)
            } else {
                let mut positions = Vec::with_capacity(n);
                let mut widths = Vec::with_capacity(n);
                let mut x = body_left;
                for (i, &idx) in row.indices.iter().enumerate() {
                    positions.push(x);
                    widths.push(glyph_dims[idx].outer_w);
                    x += glyph_dims[idx].outer_w;
                    if i + 1 < n {
                        x += base_gap;
                    }
                }
                (positions, widths)
            };

            for (i, &idx) in row.indices.iter().enumerate() {
                let cell = &cells[idx];
                let dims = &glyph_dims[idx];
                let cell_x = cell_positions[i];
                let cell_w = effective_cell_widths[i];
                let content_area_w = cell_w - padding * 2.0;

                render_glyph_cell(
                    allocator,
                    cell,
                    cell_x,
                    row_y,
                    cell_w,
                    row.height,
                    content_area_w,
                    &dims.col_widths,
                    &row.row_extents,
                    metrics_matrix,
                    variation_matrix,
                    font_id,
                    font_size,
                    sub_gap,
                    show_metrics,
                    show_names,
                    padding,
                );
            }

            allocator.advance(row.height);
            if page_row_i + 1 < page_rows.len() {
                allocator.advance(ROW_GAP);
            }
        }

        allocator.advance(vertical_padding);

        if row_idx < rows.len() {
            allocator.new_page();
        }
    }

    Ok(())
}

// ── Render an outer glyph cell (matrix of sub-cells + shared label) ──

#[allow(clippy::too_many_arguments)]
fn render_glyph_cell(
    allocator: &mut PageAllocator,
    cell: &GlyphCellData,
    cell_x: f32,
    cell_y: f32,
    cell_w: f32,
    cell_h: f32,
    content_area_w: f32,
    col_widths: &[f32],
    row_extents: &RowExtents,
    metrics_matrix: &[Vec<SubMetrics>],
    variation_matrix: &[Vec<Vec<(String, f32)>>],
    font_id: FontId,
    font_size: f32,
    sub_gap: f32,
    show_metrics: bool,
    show_names: bool,
    padding: f32,
) {
    let n_rows = row_extents.top.len();
    let n_cols = col_widths.len();
    let matrix_w = matrix_natural_width(col_widths, sub_gap);

    // Center the matrix horizontally within the content area.
    let matrix_offset_x = (content_area_w - matrix_w) / 2.0;
    let matrix_left = cell_x + padding + matrix_offset_x;

    // Per-column left offsets within the matrix.
    let mut col_lefts = Vec::with_capacity(n_cols);
    {
        let mut x = matrix_left;
        for (c, &w) in col_widths.iter().enumerate() {
            col_lefts.push(x);
            x += w;
            if c + 1 < n_cols {
                x += sub_gap;
            }
        }
    }

    // Per-row top offsets and baselines within the matrix.
    let matrix_top = cell_y + padding;
    let mut row_baselines = Vec::with_capacity(n_rows);
    {
        let mut y = matrix_top;
        for r in 0..n_rows {
            let baseline_y = y + row_extents.top[r];
            row_baselines.push(baseline_y);
            y += row_extents.row_height(r);
            if r + 1 < n_rows {
                y += sub_gap;
            }
        }
    }

    // Render each sub-cell.
    for r in 0..n_rows {
        for c in 0..n_cols {
            let s = &cell.sub[r][c];
            let m = metrics_matrix[r][c];
            let baseline_y = row_baselines[r];
            let col_w = col_widths[c];
            let col_left = col_lefts[c];

            // Center metric extent in the sub-column.
            let origin_x = col_left + (col_w - s.extent_width()) / 2.0 - s.ink_left;

            render_sub_cell(
                allocator,
                cell.glyph_id,
                cell.codepoint,
                s,
                m,
                baseline_y,
                origin_x,
                font_id,
                font_size,
                show_metrics,
                &variation_matrix[r][c],
            );
        }
    }

    // Single shared label, centered horizontally beneath the matrix.
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

// ── Render a single sub-cell (metric lines, side bearings, glyph) ───

fn slanted_x(base_x: f32, guide_y: f32, baseline_y: f32, slant: Option<MetricSlant>) -> f32 {
    slant.map_or(base_x, |metric_slant| {
        metric_slant.x_at_y(base_x, baseline_y, guide_y)
    })
}

fn push_metric_guide(
    allocator: &mut PageAllocator,
    left_x: f32,
    right_x: f32,
    guide_y: f32,
    baseline_y: f32,
    slant: Option<MetricSlant>,
    stroke: StrokeStyle,
) {
    allocator.push_command(DrawCommand::Line {
        x1: slanted_x(left_x, guide_y, baseline_y, slant),
        y1: guide_y,
        x2: slanted_x(right_x, guide_y, baseline_y, slant),
        y2: guide_y,
        stroke,
    });
}

fn push_side_bearing_guide(
    allocator: &mut PageAllocator,
    base_x: f32,
    top_y: f32,
    bottom_y: f32,
    baseline_y: f32,
    slant: Option<MetricSlant>,
    stroke: StrokeStyle,
) {
    allocator.push_command(DrawCommand::Line {
        x1: slanted_x(base_x, top_y, baseline_y, slant),
        y1: top_y,
        x2: slanted_x(base_x, bottom_y, baseline_y, slant),
        y2: bottom_y,
        stroke,
    });
}

#[allow(clippy::too_many_arguments)]
fn render_sub_cell(
    allocator: &mut PageAllocator,
    glyph_id: u32,
    codepoint: Option<char>,
    sub_cell: &SubCellData,
    metrics: SubMetrics,
    baseline_y: f32,
    origin_x: f32,
    font_id: FontId,
    font_size: f32,
    show_metrics: bool,
    variations: &[(String, f32)],
) {
    if show_metrics {
        let stroke_metric = StrokeStyle::new(0.25, METRIC_LINES);
        let stroke_baseline = StrokeStyle::new(0.25, METRIC_BASELINE);

        let line_left = origin_x + sub_cell.ink_left;
        let line_right = origin_x + sub_cell.ink_right;

        // Ascender
        let asc_y = baseline_y - metrics.ascent;
        push_metric_guide(
            allocator,
            line_left,
            line_right,
            asc_y,
            baseline_y,
            metrics.slant,
            stroke_metric,
        );

        if let Some(ch) = metrics.cap_height {
            let ch_y = baseline_y - ch;
            push_metric_guide(
                allocator,
                line_left,
                line_right,
                ch_y,
                baseline_y,
                metrics.slant,
                stroke_metric,
            );
        }

        if let Some(xh) = metrics.x_height {
            let xh_y = baseline_y - xh;
            push_metric_guide(
                allocator,
                line_left,
                line_right,
                xh_y,
                baseline_y,
                metrics.slant,
                stroke_metric,
            );
        }

        // Baseline (slightly darker)
        push_metric_guide(
            allocator,
            line_left,
            line_right,
            baseline_y,
            baseline_y,
            metrics.slant,
            stroke_baseline,
        );

        // Descender
        let desc_y = baseline_y + metrics.descent;
        push_metric_guide(
            allocator,
            line_left,
            line_right,
            desc_y,
            baseline_y,
            metrics.slant,
            stroke_metric,
        );

        // Side bearings at glyph origin and origin+advance.
        let glyph_area_top = baseline_y - metrics.ascent - 2.0;
        let glyph_area_bottom = baseline_y + metrics.descent + 2.0;

        push_side_bearing_guide(
            allocator,
            origin_x,
            glyph_area_top,
            glyph_area_bottom,
            baseline_y,
            metrics.slant,
            stroke_metric,
        );

        let rsb_x = origin_x + sub_cell.advance;
        push_side_bearing_guide(
            allocator,
            rsb_x,
            glyph_area_top,
            glyph_area_bottom,
            baseline_y,
            metrics.slant,
            stroke_metric,
        );
    }

    allocator.push_command(DrawCommand::GlyphRun {
        font_id,
        size: font_size,
        glyphs: vec![PositionedGlyph {
            glyph_id,
            x: origin_x,
            y: baseline_y,
            y_offset: 0.0,
        }],
        text: codepoint.map(|c| c.to_string()).unwrap_or_default(),
        variations: variations.to_vec(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_approx_eq(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() < 0.01,
            "expected {actual} to be within 0.01 of {expected}"
        );
    }

    #[test]
    fn metric_slant_uses_caret_offset_and_height_above_baseline() {
        let slant = MetricSlant::new(9.462322, -45.0).unwrap();
        let baseline_y = 0.0;
        let top_y = -525.0;

        assert_approx_eq(slant.x_at_y(55.0, baseline_y, baseline_y), 10.0);
        assert_approx_eq(
            slant.x_at_y(55.0, baseline_y, top_y),
            55.0 + -45.0 + 9.462322_f32.to_radians().tan() * 525.0,
        );
    }

    #[test]
    fn explicit_slnt_value_wins_over_post_italic_angle() {
        let upright_slnt = vec![(SLNT_AXIS_TAG.to_string(), 0.0)];
        assert!(metric_slant_for(&upright_slnt, Some(9.0), -45.0, None).is_none());

        let tilted_slnt = vec![(SLNT_AXIS_TAG.to_string(), -5.0)];
        let slant = metric_slant_for(&tilted_slnt, Some(9.0), -45.0, None).unwrap();
        assert_approx_eq(slant.slope, 5.0_f32.to_radians().tan());
        assert_approx_eq(slant.x_at_y(55.0, 0.0, 0.0), 10.0);

        let mvar_slant = metric_slant_for(&tilted_slnt, Some(9.0), -45.0, Some(0.167)).unwrap();
        assert_approx_eq(mvar_slant.slope, 0.167);
    }
}
