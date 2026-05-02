//! Interleave layout — multi-style page interleaving.
//!
//! Generates complete layouts per style, then interleaves the pages
//! according to the selected mode:
//! - Proofs: all pages of style A, then all pages of style B
//! - Sections: section 1 of A, section 1 of B, section 2 of A, ...
//! - Pages: page 1 of A, page 1 of B, page 2 of A, page 2 of B, ...

use content_resolver::ResolvedContent;
use font_registry::{FontId, FontRegistry};
use layout_ir::{LayoutDocument, Page};
use proof_model::{DesignAttributes, InterleaveMode};
use text_flow::TextFlow;

use crate::{build_text_style, LayoutError, PageAllocator};

/// Lay out interleaved styles.
///
/// This works by generating a complete sub-document for each style,
/// then interleaving the resulting pages.
pub fn layout(
    page_settings: &proof_model::PageSettings,
    content: &ResolvedContent,
    design_attrs: &DesignAttributes,
    mode: &InterleaveMode,
    font_ids: &[FontId],
    registry: &FontRegistry,
) -> Result<LayoutDocument, LayoutError> {
    let text = match content {
        ResolvedContent::Text(t) => t.clone(),
        ResolvedContent::Glyphs(glyphs) => glyphs
            .iter()
            .filter_map(|g| g.codepoint)
            .collect::<String>(),
    };

    if text.is_empty() || font_ids.is_empty() {
        return Ok(LayoutDocument::new());
    }

    // Generate a complete sub-layout for each style
    let mut style_docs: Vec<Vec<Page>> = Vec::with_capacity(font_ids.len());

    for &font_id in font_ids {
        let pages = layout_single_style(page_settings, &text, design_attrs, font_id, registry)?;
        style_docs.push(pages);
    }

    // Interleave pages according to mode
    let interleaved = match mode {
        InterleaveMode::Proofs => interleave_proofs(style_docs),
        InterleaveMode::Sections => interleave_sections(style_docs),
        InterleaveMode::Pages => interleave_pages(style_docs),
    };

    let mut doc = LayoutDocument::new();
    for page in interleaved {
        doc.add_page(page);
    }
    Ok(doc)
}

/// Generate a simple text layout for a single style, returning pages.
fn layout_single_style(
    page_settings: &proof_model::PageSettings,
    text: &str,
    design_attrs: &DesignAttributes,
    font_id: FontId,
    registry: &FontRegistry,
) -> Result<Vec<Page>, LayoutError> {
    let mut allocator = PageAllocator::new(page_settings.clone());
    let style = build_text_style(design_attrs, font_id);
    let max_width = allocator.body_width();
    let mut flow = TextFlow::new(text, style, max_width, registry)?;

    // Add a header label with font name
    let font_name = registry
        .metadata(font_id)
        .map(|m| format!("{} {}", m.family, m.style))
        .unwrap_or_else(|_| "Unknown".to_string());

    allocator.push_command(layout_ir::DrawCommand::Label {
        text: font_name,
        x: allocator.body_left(),
        y: allocator.cursor_y() + 9.0,
        size: 8.0,
        color: layout_ir::Color::gray(0.4),
    });
    allocator.advance(16.0);

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

        let total_height: f32 = lines.iter().map(|l| l.metrics.height()).sum();

        for cmd in commands {
            allocator.push_command(cmd);
        }
        allocator.advance(total_height);
    }

    Ok(allocator.into_document().pages)
}

/// Proofs mode: all pages of style A, then all pages of style B, etc.
fn interleave_proofs(style_docs: Vec<Vec<Page>>) -> Vec<Page> {
    let mut result = Vec::new();
    for pages in style_docs {
        result.extend(pages);
    }
    result
}

/// Sections mode: treat each style's pages as a single "section".
/// Interleave: section 1 of A, section 1 of B, section 2 of A, ...
/// Since we have a simple text layout, each style is one section,
/// so this is equivalent to Proofs mode for single-section content.
/// For multi-section documents, the caller would pass section-level data.
fn interleave_sections(style_docs: Vec<Vec<Page>>) -> Vec<Page> {
    // For now, same as proofs — the caller can split by section upstream
    interleave_proofs(style_docs)
}

/// Pages mode: page 1 of A, page 1 of B, page 2 of A, page 2 of B, ...
fn interleave_pages(style_docs: Vec<Vec<Page>>) -> Vec<Page> {
    if style_docs.is_empty() {
        return Vec::new();
    }

    let max_pages = style_docs.iter().map(|d| d.len()).max().unwrap_or(0);
    let mut result = Vec::with_capacity(max_pages * style_docs.len());

    for page_idx in 0..max_pages {
        for style_pages in &style_docs {
            if page_idx < style_pages.len() {
                result.push(style_pages[page_idx].clone());
            }
        }
    }

    result
}
