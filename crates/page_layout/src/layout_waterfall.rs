//! Waterfall layout — same text at cascading sizes.

use content_resolver::ResolvedContent;
use font_registry::{FontId, FontRegistry};
use layout_ir::{Color, DrawCommand};
use proof_model::DesignAttributes;
use text_flow::{TextFlow, TextStyle};

use crate::{LayoutError, PageAllocator};

/// A pre-shaped waterfall item ready for placement.
struct WaterfallItem {
    size: f32,
    flow: TextFlow,
    total_height: f32,
}

pub fn layout(
    allocator: &mut PageAllocator,
    content: &ResolvedContent,
    design_attrs: &DesignAttributes,
    sizes: &[f32],
    spacing: f32,
    label: Option<&str>,
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

    if text.is_empty() || sizes.is_empty() {
        return Ok(());
    }

    // Pass 1: pre-shape all sizes and compute total heights
    let mut items: Vec<WaterfallItem> = Vec::with_capacity(sizes.len());
    let max_width = allocator.body_width();

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
            text_align: design_attrs.text_align,
            line_limit: design_attrs.line_limit,
        };

        let flow = TextFlow::new(&text, style, max_width, registry)?;
        let total_height: f32 = (0..flow.line_count())
            .filter_map(|_| None::<f32>) // placeholder — compute from lines
            .sum();
        // Actually compute height by peeking all lines
        // We need to clone the flow to peek without consuming
        items.push(WaterfallItem {
            size,
            flow,
            total_height: 0.0, // will compute below
        });
    }

    // Compute total heights by examining line metrics
    for item in &mut items {
        let mut h: f32 = 0.0;
        // Peek all lines — consume into a huge height then reset
        let (lines, _) = item.flow.consume_into(f32::MAX);
        h = lines.iter().map(|l| l.metrics.height()).sum();
        item.total_height = h;
        item.flow.reset();
    }

    // Pass 2: paginate and render
    let label_size: f32 = 7.0;
    let label_height: f32 = label_size + 4.0; // label text + padding

    // Track page state
    let mut page_sizes: Vec<f32> = Vec::new();
    let mut label_y: f32 = allocator.cursor_y();
    let mut first_on_page = true;

    // Reserve space for the first page's label
    allocator.advance(label_height + spacing);

    for item in &mut items {
        let item_height = item.total_height;

        if !first_on_page {
            // Check if item fits entirely on current page
            let remaining = allocator.remaining_height();
            if item_height + spacing > remaining {
                // Emit label for current page before moving on
                emit_size_label(
                    allocator,
                    &page_sizes,
                    label,
                    label_y,
                    label_size,
                    design_attrs,
                    max_width,
                );
                page_sizes.clear();

                // Start new page
                allocator.new_page();
                label_y = allocator.cursor_y();
                allocator.advance(label_height + spacing);
                first_on_page = true;
            }
        }

        page_sizes.push(item.size);

        // Render the item's text
        while item.flow.has_remaining() {
            let available = allocator.remaining_height();
            if available <= 0.0 {
                // Emit label for current page if we haven't yet
                if !page_sizes.is_empty() {
                    emit_size_label(
                        allocator,
                        &page_sizes,
                        label,
                        label_y,
                        label_size,
                        design_attrs,
                        max_width,
                    );
                    // Don't clear page_sizes — keep current item's size for next page label
                }

                allocator.new_page();
                label_y = allocator.cursor_y();
                // On overflow pages, the label shows just the overflowing size
                page_sizes = vec![item.size];
                allocator.advance(label_height + spacing);
                continue;
            }

            let (lines, _) = item.flow.consume_into(available);
            if lines.is_empty() {
                allocator.new_page();
                label_y = allocator.cursor_y();
                page_sizes = vec![item.size];
                allocator.advance(label_height + spacing);
                continue;
            }

            let commands = TextFlow::lines_to_commands(
                &lines,
                allocator.body_left(),
                allocator.cursor_y(),
                &text,
                item.size,
                &design_attrs.variations,
            );

            let total_height: f32 = lines.iter().map(|l| l.metrics.height()).sum();
            for cmd in commands {
                allocator.push_command(cmd);
            }
            allocator.advance(total_height);
        }

        // Add spacing after item (unless more items might push to new page)
        allocator.advance(spacing);
        first_on_page = false;
    }

    // Emit label for the final page
    if !page_sizes.is_empty() {
        emit_size_label(
            allocator,
            &page_sizes,
            label,
            label_y,
            label_size,
            design_attrs,
            max_width,
        );
    }

    Ok(())
}

/// Emit a consolidated size label at the given y position.
fn emit_size_label(
    allocator: &mut PageAllocator,
    sizes: &[f32],
    user_label: Option<&str>,
    label_y: f32,
    label_size: f32,
    design_attrs: &DesignAttributes,
    max_width: f32,
) {
    if sizes.is_empty() {
        return;
    }

    // Build label text: "8pt  10pt  12pt  ...  72pt"
    let size_parts: Vec<String> = sizes.iter().map(|s| format!("{:.0}pt", s)).collect();
    // Use 1em worth of spaces — approximate with space characters
    let em_spacer = "\u{2003}"; // em space Unicode character
    let mut label_text = size_parts.join(em_spacer);

    // Append user label with 2em gap
    if let Some(ul) = user_label {
        label_text.push_str(em_spacer);
        label_text.push_str(em_spacer);
        label_text.push_str(ul);
    }

    let x = allocator.aligned_label_x(
        &label_text,
        label_size,
        design_attrs.text_align,
        allocator.body_left(),
        max_width,
    );

    allocator.push_command(DrawCommand::Label {
        text: label_text,
        x,
        y: label_y + label_size,
        size: label_size,
        color: Color::gray(0.5),
    });
}
