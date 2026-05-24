//! Column layout — multi-column text flow with optional column labels.

use content_resolver::ResolvedContent;
use font_registry::{FontId, FontRegistry};
use layout_ir::{Color, DrawCommand};
use proof_model::DesignAttributes;
use text_flow::TextFlow;

use crate::{LayoutError, PageAllocator, build_text_style};

pub fn layout(
    allocator: &mut PageAllocator,
    content: &ResolvedContent,
    design_attrs: &DesignAttributes,
    num_columns: usize,
    gutter: f32,
    column_label: Option<&str>,
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
    let label_height: f32 = if column_label.is_some() { 14.0 } else { 0.0 };

    let style = build_text_style(design_attrs, font_id);
    let mut flow = TextFlow::new(&text, style, col_width, registry)?;

    while flow.has_remaining() {
        let body_left = allocator.body_left();
        let mut page_top = allocator.cursor_y();
        let mut available_height = allocator.remaining_height();

        if available_height <= label_height {
            allocator.new_page();
            continue;
        }

        // Reserve space for labels (rendered after we know which columns have text)
        let content_top = if column_label.is_some() {
            page_top + label_height
        } else {
            page_top
        };
        let content_height = available_height
            - if column_label.is_some() {
                label_height
            } else {
                0.0
            };

        // Fill columns left to right, collecting which columns got text
        let mut any_consumed = false;
        let mut filled_columns: Vec<bool> = vec![false; num_columns];
        for col in 0..num_columns {
            if !flow.has_remaining() {
                break;
            }

            let col_x = body_left + col as f32 * (col_width + gutter);
            let (lines, _) = flow.consume_into(content_height);
            if lines.is_empty() {
                continue;
            }

            filled_columns[col] = true;
            any_consumed = true;
            let commands = TextFlow::lines_to_commands(
                &lines,
                col_x,
                content_top,
                &text,
                design_attrs.font_size,
                &design_attrs.variations,
            );

            for cmd in commands {
                allocator.push_command(cmd);
            }
        }

        // Render labels only for columns that received text
        if let Some(label) = column_label {
            for col in 0..num_columns {
                if filled_columns[col] {
                    let col_x = body_left + col as f32 * (col_width + gutter);
                    allocator.push_command(DrawCommand::Label {
                        x: allocator.aligned_label_x(
                            label,
                            7.0,
                            design_attrs.text_align,
                            col_x,
                            col_width,
                        ),
                        text: label.to_string(),
                        y: page_top + 9.0,
                        size: 7.0,
                        color: Color::gray(0.4),
                    });
                }
            }
        }

        if !any_consumed {
            break;
        }

        if flow.has_remaining() {
            allocator.new_page();
        } else {
            allocator.advance(available_height);
        }
    }

    Ok(())
}
