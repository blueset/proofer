//! Simple layout — text block(s) at configured size.

use content_resolver::ResolvedContent;
use font_registry::{FontId, FontRegistry};
use layout_ir::DrawCommand;
use proof_model::DesignAttributes;
use text_flow::{TextFlow, TextStyle};

use crate::{build_text_style, LayoutError, PageAllocator};

pub fn layout(
    allocator: &mut PageAllocator,
    content: &ResolvedContent,
    design_attrs: &DesignAttributes,
    font_id: FontId,
    registry: &FontRegistry,
) -> Result<(), LayoutError> {
    let text = match content {
        ResolvedContent::Text(t) => t.clone(),
        ResolvedContent::Glyphs(glyphs) => {
            // For simple layout, convert glyphs to a text string
            glyphs
                .iter()
                .filter_map(|g| g.codepoint)
                .collect::<String>()
        }
    };

    if text.is_empty() {
        return Ok(());
    }

    let style = build_text_style(design_attrs, font_id);
    let max_width = allocator.body_width();
    let mut flow = TextFlow::new(&text, style, max_width, registry)?;

    // Apply y_offset on the first page
    if design_attrs.y_offset > 0.0 {
        allocator.advance(design_attrs.y_offset);
    }

    while flow.has_remaining() {
        let available = allocator.remaining_height();
        if available <= 0.0 {
            allocator.new_page();
            if design_attrs.y_offset > 0.0 {
                allocator.advance(design_attrs.y_offset);
            }
            continue;
        }

        let (lines, _remaining) = flow.consume_into(available);
        if lines.is_empty() {
            allocator.new_page();
            if design_attrs.y_offset > 0.0 {
                allocator.advance(design_attrs.y_offset);
            }
            continue;
        }

        let commands = TextFlow::lines_to_commands(
            &lines,
            allocator.body_left(),
            allocator.cursor_y(),
            &text,
            design_attrs.font_size,
            &design_attrs.variations,
        );

        let total_height: f32 = lines.iter().map(|l| l.metrics.height()).sum();

        for cmd in commands {
            allocator.push_command(cmd);
        }
        allocator.advance(total_height);
    }

    Ok(())
}
