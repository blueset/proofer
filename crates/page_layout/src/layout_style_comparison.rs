//! Style Comparison layout — side-by-side style comparison.
//!
//! Displays the same text in multiple font styles side by side,
//! either in columns or rows. Supports truncate (clip overflow)
//! and synchronized flow (all columns break at the same point).

use content_resolver::ResolvedContent;
use font_registry::{FontId, FontRegistry};
use layout_ir::{Color, DrawCommand, StrokeStyle};
use proof_model::{ComparisonArrangement, ComparisonOverflow, DesignAttributes};
use text_flow::{self, TextFlow, TextStyle};

use crate::{build_text_style, LayoutError, PageAllocator};

/// Lay out a style comparison section.
///
/// `font_ids` contains the font IDs for each style to compare.
/// Each style gets its own column (or row).
pub fn layout(
    allocator: &mut PageAllocator,
    content: &ResolvedContent,
    design_attrs: &DesignAttributes,
    arrangement: &ComparisonArrangement,
    overflow: &ComparisonOverflow,
    font_ids: &[FontId],
    registry: &FontRegistry,
) -> Result<(), LayoutError> {
    let text = match content {
        ResolvedContent::Text(t) => t.clone(),
        ResolvedContent::Glyphs(glyphs) => glyphs
            .iter()
            .filter_map(|g| g.codepoint)
            .collect::<String>(),
    };

    if text.is_empty() || font_ids.is_empty() {
        return Ok(());
    }

    match arrangement {
        ComparisonArrangement::Columns => {
            layout_columns(allocator, &text, design_attrs, overflow, font_ids, registry)
        }
        ComparisonArrangement::Rows => {
            layout_rows(allocator, &text, design_attrs, overflow, font_ids, registry)
        }
    }
}

/// Column arrangement: each style gets a vertical column, side by side.
fn layout_columns(
    allocator: &mut PageAllocator,
    text: &str,
    design_attrs: &DesignAttributes,
    overflow: &ComparisonOverflow,
    font_ids: &[FontId],
    registry: &FontRegistry,
) -> Result<(), LayoutError> {
    let num_cols = font_ids.len();
    let gutter = 12.0; // points between columns
    let total_gutter = gutter * (num_cols - 1) as f32;
    let col_width = (allocator.body_width() - total_gutter) / num_cols as f32;

    // Create text flows for each style
    let mut flows: Vec<TextFlow> = Vec::with_capacity(num_cols);
    for &font_id in font_ids {
        let style = build_text_style(design_attrs, font_id);
        flows.push(TextFlow::new(text, style, col_width, registry)?);
    }

    // Draw column header labels (font names)
    let header_height = 12.0;
    allocator.ensure_space(header_height + 20.0);
    let header_y = allocator.cursor_y();
    let body_left = allocator.body_left();

    for (i, &font_id) in font_ids.iter().enumerate() {
        let col_x = body_left + i as f32 * (col_width + gutter);
        let font_name = registry
            .metadata(font_id)
            .map(|m| format!("{} {}", m.family, m.style))
            .unwrap_or_else(|_| format!("Style {}", i + 1));

        allocator.push_command(DrawCommand::Label {
            text: font_name,
            x: col_x,
            y: header_y + 9.0,
            size: 7.0,
            color: Color::gray(0.4),
        });
    }
    allocator.advance(header_height);

    match overflow {
        ComparisonOverflow::Truncate => {
            layout_columns_truncate(allocator, &mut flows, text, col_width, gutter, font_ids)?;
        }
        ComparisonOverflow::Flow => {
            layout_columns_synced(allocator, &mut flows, text, col_width, gutter, font_ids)?;
        }
    }

    Ok(())
}

/// Truncate mode: consume each flow independently, clip overflow.
fn layout_columns_truncate(
    allocator: &mut PageAllocator,
    flows: &mut [TextFlow],
    text: &str,
    col_width: f32,
    gutter: f32,
    font_ids: &[FontId],
) -> Result<(), LayoutError> {
    let body_left = allocator.body_left();
    let available = allocator.remaining_height();

    // Draw gutter lines
    for i in 1..flows.len() {
        let x = body_left + i as f32 * (col_width + gutter) - gutter / 2.0;
        allocator.push_command(DrawCommand::Line {
            x1: x,
            y1: allocator.cursor_y(),
            x2: x,
            y2: allocator.cursor_y() + available,
            stroke: StrokeStyle::hairline(Color::LIGHT_GRAY),
        });
    }

    // Consume each flow into its column, clipping at available height
    for (i, flow) in flows.iter_mut().enumerate() {
        let col_x = body_left + i as f32 * (col_width + gutter);
        let (lines, _) = flow.consume_into(available);

        if lines.is_empty() {
            continue;
        }

        // Clip to column bounds
        let clip_y = allocator.cursor_y();
        let commands = TextFlow::lines_to_commands(&lines, col_x, clip_y, text);

        allocator.push_command(DrawCommand::Clip {
            x: col_x,
            y: clip_y,
            w: col_width,
            h: available,
            children: commands,
        });
    }

    allocator.advance(available);
    Ok(())
}

/// Synchronized flow mode: all columns break at the same content point.
fn layout_columns_synced(
    allocator: &mut PageAllocator,
    flows: &mut [TextFlow],
    text: &str,
    col_width: f32,
    gutter: f32,
    font_ids: &[FontId],
) -> Result<(), LayoutError> {
    let body_left = allocator.body_left();

    while flows.iter().any(|f| f.has_remaining()) {
        let available = allocator.remaining_height();
        if available <= 0.0 {
            allocator.new_page();
            continue;
        }

        // Draw gutter lines
        for i in 1..flows.len() {
            let x = body_left + i as f32 * (col_width + gutter) - gutter / 2.0;
            allocator.push_command(DrawCommand::Line {
                x1: x,
                y1: allocator.cursor_y(),
                x2: x,
                y2: allocator.cursor_y() + available,
                stroke: StrokeStyle::hairline(Color::LIGHT_GRAY),
            });
        }

        // Use sync_consume to consume from all flows simultaneously
        let results = text_flow::sync_consume(flows, available);

        let mut max_height: f32 = 0.0;
        for (i, lines) in results.iter().enumerate() {
            let col_x = body_left + i as f32 * (col_width + gutter);
            let col_y = allocator.cursor_y();

            let height: f32 = lines.iter().map(|l| l.metrics.height()).sum();
            max_height = max_height.max(height);

            if !lines.is_empty() {
                let commands = TextFlow::lines_to_commands(lines, col_x, col_y, text);
                for cmd in commands {
                    allocator.push_command(cmd);
                }
            }
        }

        if max_height <= 0.0 {
            break;
        }

        allocator.advance(max_height);
    }

    Ok(())
}

/// Row arrangement: each style gets a horizontal row, stacked vertically.
fn layout_rows(
    allocator: &mut PageAllocator,
    text: &str,
    design_attrs: &DesignAttributes,
    overflow: &ComparisonOverflow,
    font_ids: &[FontId],
    registry: &FontRegistry,
) -> Result<(), LayoutError> {
    let body_width = allocator.body_width();
    let row_spacing = 16.0;

    for (i, &font_id) in font_ids.iter().enumerate() {
        // Row label
        let font_name = registry
            .metadata(font_id)
            .map(|m| format!("{} {}", m.family, m.style))
            .unwrap_or_else(|_| format!("Style {}", i + 1));

        allocator.ensure_space(20.0);
        allocator.push_command(DrawCommand::Label {
            text: font_name,
            x: allocator.body_left(),
            y: allocator.cursor_y() + 9.0,
            size: 7.0,
            color: Color::gray(0.4),
        });
        allocator.advance(12.0);

        // Separator line
        allocator.push_command(DrawCommand::Line {
            x1: allocator.body_left(),
            y1: allocator.cursor_y(),
            x2: allocator.body_left() + body_width,
            y2: allocator.cursor_y(),
            stroke: StrokeStyle::hairline(Color::LIGHT_GRAY),
        });
        allocator.advance(4.0);

        // Text flow for this style
        let style = build_text_style(design_attrs, font_id);
        let mut flow = TextFlow::new(text, style, body_width, registry)?;

        match overflow {
            ComparisonOverflow::Truncate => {
                // Consume only what fits on the current page
                let available = allocator.remaining_height().min(200.0); // cap row height
                let (lines, _) = flow.consume_into(available);
                if !lines.is_empty() {
                    let commands = TextFlow::lines_to_commands(
                        &lines,
                        allocator.body_left(),
                        allocator.cursor_y(),
                        text,
                    );
                    let height: f32 = lines.iter().map(|l| l.metrics.height()).sum();

                    allocator.push_command(DrawCommand::Clip {
                        x: allocator.body_left(),
                        y: allocator.cursor_y(),
                        w: body_width,
                        h: available,
                        children: commands,
                    });
                    allocator.advance(height.min(available));
                }
            }
            ComparisonOverflow::Flow => {
                // Flow all text, potentially across pages
                while flow.has_remaining() {
                    let available = allocator.remaining_height();
                    if available <= 0.0 {
                        allocator.new_page();
                        continue;
                    }

                    let (lines, _) = flow.consume_into(available);
                    if lines.is_empty() {
                        allocator.new_page();
                        continue;
                    }

                    let commands = TextFlow::lines_to_commands(
                        &lines,
                        allocator.body_left(),
                        allocator.cursor_y(),
                        text,
                    );
                    let height: f32 = lines.iter().map(|l| l.metrics.height()).sum();

                    for cmd in commands {
                        allocator.push_command(cmd);
                    }
                    allocator.advance(height);
                }
            }
        }

        allocator.advance(row_spacing);
    }

    Ok(())
}
