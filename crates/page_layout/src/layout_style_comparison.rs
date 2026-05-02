//! Style Comparison layout — side-by-side style comparison.
//!
//! Displays the same text in multiple styles side by side,
//! either in columns or rows. Each style can differ in font,
//! size, tracking, features, variations, etc.

use content_resolver::ResolvedContent;
use font_registry::{FontId, FontRegistry};
use layout_ir::{Color, DrawCommand, StrokeStyle};
use proof_model::{ComparisonArrangement, ComparisonOverflow, DesignAttributes};
use text_flow::{self, TextFlow};

use crate::{build_text_style, LayoutError, PageAllocator, ResolvedStyleVariant};

/// Lay out a style comparison section.
pub fn layout(
    allocator: &mut PageAllocator,
    content: &ResolvedContent,
    base_design_attrs: &DesignAttributes,
    arrangement: &ComparisonArrangement,
    overflow: &ComparisonOverflow,
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
            layout_columns(allocator, &text, overflow, variants, registry)
        }
        ComparisonArrangement::Rows => {
            layout_rows(allocator, &text, overflow, variants, registry)
        }
    }
}

/// Column arrangement: each style gets a vertical column, side by side.
fn layout_columns(
    allocator: &mut PageAllocator,
    text: &str,
    overflow: &ComparisonOverflow,
    variants: &[ResolvedStyleVariant],
    registry: &FontRegistry,
) -> Result<(), LayoutError> {
    let num_cols = variants.len();
    let gutter = 12.0;
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
            text: label,
            x: col_x,
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
            layout_columns_synced(allocator, &mut flows, text, col_width, gutter, variants)?;
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

    for i in 1..flows.len() {
        let x = body_left + i as f32 * (col_width + gutter) - gutter / 2.0;
        allocator.push_command(DrawCommand::Line {
            x1: x, y1: allocator.cursor_y(),
            x2: x, y2: allocator.cursor_y() + available,
            stroke: StrokeStyle::hairline(Color::LIGHT_GRAY),
        });
    }

    for (i, flow) in flows.iter_mut().enumerate() {
        let col_x = body_left + i as f32 * (col_width + gutter);
        let (lines, _) = flow.consume_into(available);
        if lines.is_empty() { continue; }

        let clip_y = allocator.cursor_y();
        let font_size = variants[i].design_attrs.font_size;
        let commands = TextFlow::lines_to_commands(&lines, col_x, clip_y, text, font_size);

        allocator.push_command(DrawCommand::Clip {
            x: col_x, y: clip_y, w: col_width, h: available,
            children: commands,
        });
    }

    allocator.advance(available);
    Ok(())
}

fn layout_columns_synced(
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
        if available <= 0.0 { allocator.new_page(); continue; }

        for i in 1..flows.len() {
            let x = body_left + i as f32 * (col_width + gutter) - gutter / 2.0;
            allocator.push_command(DrawCommand::Line {
                x1: x, y1: allocator.cursor_y(),
                x2: x, y2: allocator.cursor_y() + available,
                stroke: StrokeStyle::hairline(Color::LIGHT_GRAY),
            });
        }

        let results = text_flow::sync_consume(flows, available);

        let mut max_height: f32 = 0.0;
        for (i, lines) in results.iter().enumerate() {
            let col_x = body_left + i as f32 * (col_width + gutter);
            let col_y = allocator.cursor_y();
            let height: f32 = lines.iter().map(|l| l.metrics.height()).sum();
            max_height = max_height.max(height);

            if !lines.is_empty() {
                let font_size = variants[i].design_attrs.font_size;
                let commands = TextFlow::lines_to_commands(lines, col_x, col_y, text, font_size);
                for cmd in commands { allocator.push_command(cmd); }
            }
        }

        if max_height <= 0.0 { break; }
        allocator.advance(max_height);
    }

    Ok(())
}

/// Row arrangement: each style gets a horizontal row, stacked vertically.
fn layout_rows(
    allocator: &mut PageAllocator,
    text: &str,
    overflow: &ComparisonOverflow,
    variants: &[ResolvedStyleVariant],
    registry: &FontRegistry,
) -> Result<(), LayoutError> {
    let body_width = allocator.body_width();
    let row_spacing = 16.0;

    for (i, variant) in variants.iter().enumerate() {
        let label = variant.label.clone().unwrap_or_else(|| {
            let meta = registry.metadata(variant.font_id);
            let base = meta.map(|m| format!("{} {}", m.family, m.style)).unwrap_or_default();
            format!("{base} {:.0}pt", variant.design_attrs.font_size)
        });

        allocator.ensure_space(20.0);
        allocator.push_command(DrawCommand::Label {
            text: label,
            x: allocator.body_left(),
            y: allocator.cursor_y() + 9.0,
            size: 7.0,
            color: Color::gray(0.4),
        });
        allocator.advance(12.0);

        allocator.push_command(DrawCommand::Line {
            x1: allocator.body_left(), y1: allocator.cursor_y(),
            x2: allocator.body_left() + body_width, y2: allocator.cursor_y(),
            stroke: StrokeStyle::hairline(Color::LIGHT_GRAY),
        });
        allocator.advance(4.0);

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
                    );
                    let height: f32 = lines.iter().map(|l| l.metrics.height()).sum();
                    for cmd in commands { allocator.push_command(cmd); }
                    allocator.advance(height);
                }
            }
        }

        allocator.advance(row_spacing);
    }

    Ok(())
}
