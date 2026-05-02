//! Column layout — multi-column text flow with per-column headers.

use content_resolver::ResolvedContent;
use font_registry::{FontId, FontRegistry};
use layout_ir::{Color, DrawCommand, StrokeStyle};
use proof_model::DesignAttributes;
use text_flow::{TextFlow, TextStyle};

use crate::{build_text_style, LayoutError, PageAllocator};

pub fn layout(
    allocator: &mut PageAllocator,
    content: &ResolvedContent,
    design_attrs: &DesignAttributes,
    num_columns: usize,
    gutter: f32,
    font_id: FontId,
    registry: &FontRegistry,
) -> Result<(), LayoutError> {
    let text = match content {
        ResolvedContent::Text(t) => t.clone(),
        ResolvedContent::Glyphs(glyphs) => glyphs
            .iter()
            .filter_map(|g| g.codepoint)
            .collect::<String>(),
    };

    if text.is_empty() || num_columns == 0 {
        return Ok(());
    }

    let total_gutter = gutter * (num_columns - 1) as f32;
    let col_width = (allocator.body_width() - total_gutter) / num_columns as f32;

    let style = build_text_style(design_attrs, font_id);
    let mut flow = TextFlow::new(&text, style, col_width, registry)?;

    while flow.has_remaining() {
        // Draw column separators
        let body_left = allocator.body_left();
        let page_top = allocator.cursor_y();
        let available_height = allocator.remaining_height();

        if available_height <= 0.0 {
            allocator.new_page();
            continue;
        }

        // Draw gutter lines
        for col in 1..num_columns {
            let x = body_left + col as f32 * (col_width + gutter) - gutter / 2.0;
            allocator.push_command(DrawCommand::Line {
                x1: x,
                y1: page_top,
                x2: x,
                y2: page_top + available_height,
                stroke: StrokeStyle::hairline(Color::LIGHT_GRAY),
            });
        }

        // Fill columns left to right
        let mut any_consumed = false;
        for col in 0..num_columns {
            if !flow.has_remaining() {
                break;
            }

            let col_x = body_left + col as f32 * (col_width + gutter);
            let (lines, _) = flow.consume_into(available_height);
            if lines.is_empty() {
                continue;
            }

            any_consumed = true;
            let commands =
                TextFlow::lines_to_commands(&lines, col_x, page_top, &text, design_attrs.font_size);

            for cmd in commands {
                allocator.push_command(cmd);
            }
        }

        if !any_consumed {
            break;
        }

        // Move to next page for remaining text
        if flow.has_remaining() {
            allocator.new_page();
        } else {
            allocator.advance(available_height);
        }
    }

    Ok(())
}
