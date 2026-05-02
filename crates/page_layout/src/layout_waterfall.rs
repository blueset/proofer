//! Waterfall layout — same text at cascading sizes.

use content_resolver::ResolvedContent;
use font_registry::{FontId, FontRegistry};
use layout_ir::{Color, DrawCommand};
use proof_model::DesignAttributes;
use text_flow::{TextFlow, TextStyle};

use crate::{LayoutError, PageAllocator};

pub fn layout(
    allocator: &mut PageAllocator,
    content: &ResolvedContent,
    design_attrs: &DesignAttributes,
    sizes: &[f32],
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

    if text.is_empty() {
        return Ok(());
    }

    let spacing = 8.0; // points between size blocks

    for &size in sizes {
        let style = TextStyle {
            font_id,
            font_size: size,
            line_height: design_attrs.line_height,
            tracking: design_attrs.tracking,
            kerning: design_attrs.kerning,
            features: design_attrs.features.clone(),
            language: design_attrs.language.clone(),
            variations: design_attrs.variations.clone(),
        };

        let max_width = allocator.body_width();
        let mut flow = TextFlow::new(&text, style, max_width, registry)?;

        // Optional: show size label
        let label_height = 10.0;
        allocator.ensure_space(label_height + size * 1.5);

        allocator.push_command(DrawCommand::Label {
            text: format!("{:.0}pt", size),
            x: allocator.body_left(),
            y: allocator.cursor_y() + 8.0,
            size: 7.0,
            color: Color::gray(0.5),
        });
        allocator.advance(label_height);

        // Consume lines for this size
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
                &text,
                size,
                &design_attrs.variations,
            );

            let total_height: f32 = lines.iter().map(|l| l.metrics.height()).sum();

            for cmd in commands {
                allocator.push_command(cmd);
            }
            allocator.advance(total_height);
        }

        allocator.advance(spacing);
    }

    Ok(())
}
