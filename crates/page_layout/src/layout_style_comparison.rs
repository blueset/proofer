//! Style Comparison layout — side-by-side style comparison.
//!
//! Displays the same text in multiple styles side by side,
//! either in columns or rows. Each style can differ in font,
//! size, tracking, features, variations, etc.

use content_resolver::ResolvedContent;
use font_registry::{FontId, FontRegistry};
use layout_ir::{Color, DrawCommand};
use proof_model::{ComparisonArrangement, ComparisonOverflow, DesignAttributes};
use text_flow::TextFlow;

use crate::{build_text_style, LayoutError, PageAllocator, ResolvedStyleVariant};

/// Lay out a style comparison section.
pub fn layout(
    allocator: &mut PageAllocator,
    content: &ResolvedContent,
    _base_design_attrs: &DesignAttributes,
    arrangement: &ComparisonArrangement,
    overflow: &ComparisonOverflow,
    spacing: f32,
    max_columns: Option<usize>,
    variants: &[ResolvedStyleVariant],
    registry: &FontRegistry,
) -> Result<(), LayoutError> {
    let text = match content {
        ResolvedContent::Text(t) => t.clone(),
        ResolvedContent::Glyphs(glyphs) => glyphs
            .iter()
            .filter_map(|g| g.codepoint)
            .collect::<String>(),
    };

    if text.is_empty() || variants.is_empty() {
        return Ok(());
    }

    match arrangement {
        ComparisonArrangement::Columns => {
            let chunk_size = max_columns
                .filter(|&n| n > 0)
                .unwrap_or(variants.len());

            for (chunk_idx, chunk) in variants.chunks(chunk_size).enumerate() {
                if chunk_idx > 0 {
                    allocator.new_page();
                }
                layout_columns(allocator, &text, overflow, spacing, chunk, registry)?;
            }
            Ok(())
        }
        ComparisonArrangement::Rows => {
            layout_rows(allocator, &text, overflow, spacing, variants, registry)
        }
    }
}

/// Column arrangement: each style gets a vertical column, side by side.
fn layout_columns(
    allocator: &mut PageAllocator,
    text: &str,
    overflow: &ComparisonOverflow,
    spacing: f32,
    variants: &[ResolvedStyleVariant],
    registry: &FontRegistry,
) -> Result<(), LayoutError> {
    let num_cols = variants.len();
    let gutter = spacing;
    let total_gutter = gutter * (num_cols - 1) as f32;
    let col_width = (allocator.body_width() - total_gutter) / num_cols as f32;

    // Create text flows for each style variant
    let mut flows: Vec<TextFlow> = Vec::with_capacity(num_cols);
    for variant in variants {
        let style = build_text_style(&variant.design_attrs, variant.font_id);
        flows.push(TextFlow::new(text, style, col_width, registry)?);
    }

    // Column header labels
    let header_height = 12.0;
    allocator.ensure_space(header_height + 20.0);
    let header_y = allocator.cursor_y();
    let body_left = allocator.body_left();

    for (i, variant) in variants.iter().enumerate() {
        let col_x = body_left + i as f32 * (col_width + gutter);
        let label = variant.label.clone().unwrap_or_else(|| {
            let meta = registry.metadata(variant.font_id);
            let base = meta.map(|m| format!("{} {}", m.family, m.style)).unwrap_or_default();
            let attrs = &variant.design_attrs;
            format!("{base} {:.0}pt", attrs.font_size)
        });

        allocator.push_command(DrawCommand::Label {
            x: allocator.aligned_label_x(&label, 7.0, variant.design_attrs.text_align, col_x, col_width),
            text: label,
            y: header_y + 9.0,
            size: 7.0,
            color: Color::gray(0.4),
        });
    }
    allocator.advance(header_height);

    match overflow {
        ComparisonOverflow::Truncate => {
            layout_columns_truncate(allocator, &mut flows, text, col_width, gutter, variants)?;
        }
        ComparisonOverflow::Flow => {
            layout_columns_flow(allocator, &mut flows, text, col_width, gutter, variants)?;
        }
    }

    Ok(())
}

fn layout_columns_truncate(
    allocator: &mut PageAllocator,
    flows: &mut [TextFlow],
    text: &str,
    col_width: f32,
    gutter: f32,
    variants: &[ResolvedStyleVariant],
) -> Result<(), LayoutError> {
    let body_left = allocator.body_left();
    let available = allocator.remaining_height();

    for (i, flow) in flows.iter_mut().enumerate() {
        let col_x = body_left + i as f32 * (col_width + gutter);
        let (lines, _) = flow.consume_into(available);
        if lines.is_empty() { continue; }

        let clip_y = allocator.cursor_y();
        let font_size = variants[i].design_attrs.font_size;
        let commands = TextFlow::lines_to_commands(&lines, col_x, clip_y, text, font_size, &variants[i].design_attrs.variations);

        allocator.push_command(DrawCommand::Clip {
            x: col_x, y: clip_y, w: col_width, h: available,
            children: commands,
        });
    }

    allocator.advance(available);
    Ok(())
}

/// Independent flow mode: each column flows independently across pages.
/// Columns that finish early leave empty space on subsequent pages.
/// When ALL columns are done, the section is complete.
fn layout_columns_flow(
    allocator: &mut PageAllocator,
    flows: &mut [TextFlow],
    text: &str,
    col_width: f32,
    gutter: f32,
    variants: &[ResolvedStyleVariant],
) -> Result<(), LayoutError> {
    let body_left = allocator.body_left();

    while flows.iter().any(|f| f.has_remaining()) {
        let available = allocator.remaining_height();
        if available <= 0.0 {
            allocator.new_page();
            continue;
        }

        // Each column consumes independently into the available height
        let mut max_height: f32 = 0.0;
        for (i, flow) in flows.iter_mut().enumerate() {
            if !flow.has_remaining() {
                continue;
            }

            let col_x = body_left + i as f32 * (col_width + gutter);
            let col_y = allocator.cursor_y();
            let (lines, _) = flow.consume_into(available);

            if lines.is_empty() {
                continue;
            }

            let height: f32 = lines.iter().map(|l| l.metrics.height()).sum();
            max_height = max_height.max(height);

            let font_size = variants[i].design_attrs.font_size;
            let commands = TextFlow::lines_to_commands(
                &lines, col_x, col_y, text, font_size,
                &variants[i].design_attrs.variations,
            );
            for cmd in commands {
                allocator.push_command(cmd);
            }
        }

        if max_height <= 0.0 {
            break;
        }

        allocator.advance(max_height);

        // If any column still has remaining text, go to next page
        // so all columns get a fresh page together
        if flows.iter().any(|f| f.has_remaining()) {
            allocator.new_page();
        }
    }

    Ok(())
}

/// Row arrangement: each style gets a horizontal row, stacked vertically.
fn layout_rows(
    allocator: &mut PageAllocator,
    text: &str,
    overflow: &ComparisonOverflow,
    spacing: f32,
    variants: &[ResolvedStyleVariant],
    registry: &FontRegistry,
) -> Result<(), LayoutError> {
    let body_width = allocator.body_width();

    for (i, variant) in variants.iter().enumerate() {
        let label = variant.label.clone().unwrap_or_else(|| {
            let meta = registry.metadata(variant.font_id);
            let base = meta.map(|m| format!("{} {}", m.family, m.style)).unwrap_or_default();
            format!("{base} {:.0}pt", variant.design_attrs.font_size)
        });

        allocator.ensure_space(20.0);
        allocator.push_command(DrawCommand::Label {
            x: allocator.aligned_label_x(&label, 7.0, variant.design_attrs.text_align, allocator.body_left(), body_width),
            text: label,
            y: allocator.cursor_y() + 9.0,
            size: 7.0,
            color: Color::gray(0.4),
        });
        allocator.advance(12.0);

        let style = build_text_style(&variant.design_attrs, variant.font_id);
        let font_size = variant.design_attrs.font_size;
        let mut flow = TextFlow::new(text, style, body_width, registry)?;

        match overflow {
            ComparisonOverflow::Truncate => {
                let available = allocator.remaining_height().min(200.0);
                let (lines, _) = flow.consume_into(available);
                if !lines.is_empty() {
                    let commands = TextFlow::lines_to_commands(
                        &lines, allocator.body_left(), allocator.cursor_y(), text, font_size,
                        &variant.design_attrs.variations,
                    );
                    let height: f32 = lines.iter().map(|l| l.metrics.height()).sum();
                    allocator.push_command(DrawCommand::Clip {
                        x: allocator.body_left(), y: allocator.cursor_y(),
                        w: body_width, h: available, children: commands,
                    });
                    allocator.advance(height.min(available));
                }
            }
            ComparisonOverflow::Flow => {
                while flow.has_remaining() {
                    let available = allocator.remaining_height();
                    if available <= 0.0 { allocator.new_page(); continue; }
                    let (lines, _) = flow.consume_into(available);
                    if lines.is_empty() { allocator.new_page(); continue; }
                    let commands = TextFlow::lines_to_commands(
                        &lines, allocator.body_left(), allocator.cursor_y(), text, font_size,
                        &variant.design_attrs.variations,
                    );
                    let height: f32 = lines.iter().map(|l| l.metrics.height()).sum();
                    for cmd in commands { allocator.push_command(cmd); }
                    allocator.advance(height);
                }
            }
        }

        allocator.advance(spacing);
    }

    Ok(())
}
