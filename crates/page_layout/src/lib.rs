//! Page Layout Engine — orchestrates layout types, producing Layout IR.
//!
//! This is the core layout engine that transforms proof sections into
//! positioned draw commands organized into pages.

pub mod layout_columns;
pub mod layout_glyph_grid;
pub mod layout_interleave;
pub mod layout_simple;
pub mod layout_style_comparison;
pub mod layout_waterfall;

use font_inspector::FontInspector;
use font_registry::{FontId, FontRegistry};
use layout_ir::{Color, DrawCommand, LayoutDocument, Page, StrokeStyle};
use proof_model::{HeaderConfig, PageSettings, Section};
use text_flow::{TextFlow, TextStyle};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum LayoutError {
    #[error("text flow error: {0}")]
    TextFlow(#[from] text_flow::TextFlowError),
    #[error("content resolver error: {0}")]
    ContentResolver(#[from] content_resolver::ResolverError),
    #[error("font error: {0}")]
    Font(#[from] font_registry::FontError),
    #[error("inspector error: {0}")]
    Inspector(#[from] font_inspector::InspectorError),
    #[error("no fonts specified for section")]
    NoFonts,
    #[error("font index {0} out of range")]
    FontIndexOutOfRange(usize),
}

/// Manages page allocation during layout.
pub struct PageAllocator {
    page_settings: PageSettings,
    pages: Vec<Page>,
    cursor_y: f32,
}

impl PageAllocator {
    pub fn new(page_settings: PageSettings) -> Self {
        let mut allocator = Self {
            page_settings,
            pages: Vec::new(),
            cursor_y: 0.0,
        };
        allocator.new_page();
        allocator
    }

    /// Start a new page and reset cursor.
    pub fn new_page(&mut self) {
        let page = Page::new(self.page_settings.width(), self.page_settings.height());
        self.pages.push(page);
        self.cursor_y = self.page_settings.margin_top();
    }

    /// Get remaining height on current page.
    pub fn remaining_height(&self) -> f32 {
        self.page_settings.height() - self.page_settings.margin_bottom() - self.cursor_y
    }

    /// Advance the cursor by a given amount.
    pub fn advance(&mut self, amount: f32) {
        self.cursor_y += amount;
    }

    /// Get current Y position.
    pub fn cursor_y(&self) -> f32 {
        self.cursor_y
    }

    /// Get page body left X.
    pub fn body_left(&self) -> f32 {
        self.page_settings.margin_left()
    }

    /// Get page body width.
    pub fn body_width(&self) -> f32 {
        self.page_settings.body_width()
    }

    /// Get page body height.
    pub fn body_height(&self) -> f32 {
        self.page_settings.body_height()
    }

    /// Push a command to the current page.
    pub fn push_command(&mut self, cmd: DrawCommand) {
        if let Some(page) = self.pages.last_mut() {
            page.push(cmd);
        }
    }

    /// Ensure at least `height` is available, starting a new page if needed.
    pub fn ensure_space(&mut self, height: f32) {
        if self.remaining_height() < height {
            self.new_page();
        }
    }

    /// Consume into a LayoutDocument.
    pub fn into_document(self) -> LayoutDocument {
        let mut doc = LayoutDocument::new();
        for page in self.pages {
            doc.add_page(page);
        }
        doc
    }

    /// Push a pre-built page (e.g., from interleave layout).
    pub fn push_page(&mut self, page: Page) {
        self.pages.push(page);
        self.cursor_y = self.page_settings.margin_top();
    }

    /// Get the page settings.
    pub fn page_settings(&self) -> &PageSettings {
        &self.page_settings
    }

    /// Get current page index.
    pub fn current_page_index(&self) -> usize {
        self.pages.len().saturating_sub(1)
    }
}

/// Render a section header.
pub fn render_header(
    allocator: &mut PageAllocator,
    section: &Section,
    font_name: &str,
    page_number: usize,
) {
    let config = &section.header_config;
    if !config.show_header {
        return;
    }

    let header_size: f32 = 8.0;
    let header_y = allocator.cursor_y();
    let left = allocator.body_left();
    let width = allocator.body_width();

    // Section name on the left
    if let Some(name) = &section.name {
        allocator.push_command(DrawCommand::Label {
            text: name.clone(),
            x: left,
            y: header_y + header_size,
            size: header_size,
            color: Color::gray(0.4),
        });
    }

    // Font name in the center
    if config.show_font_name {
        allocator.push_command(DrawCommand::Label {
            text: font_name.to_string(),
            x: left + width / 2.0 - 50.0, // approximate centering
            y: header_y + header_size,
            size: header_size,
            color: Color::gray(0.4),
        });
    }

    // Page number on the right
    if config.show_page_numbers {
        allocator.push_command(DrawCommand::Label {
            text: format!("{}", page_number),
            x: left + width - 20.0,
            y: header_y + header_size,
            size: header_size,
            color: Color::gray(0.4),
        });
    }

    // Header separator line
    let line_y = header_y + header_size + 4.0;
    allocator.push_command(DrawCommand::Line {
        x1: left,
        y1: line_y,
        x2: left + width,
        y2: line_y,
        stroke: StrokeStyle::hairline(Color::LIGHT_GRAY),
    });

    allocator.advance(header_size + 12.0); // header height + spacing
}

/// Build a TextStyle from section design attributes and a font ID.
pub fn build_text_style(
    design_attrs: &proof_model::DesignAttributes,
    font_id: FontId,
) -> TextStyle {
    TextStyle {
        font_id,
        font_size: design_attrs.font_size,
        line_height: design_attrs.line_height,
        tracking: design_attrs.tracking,
        kerning: design_attrs.kerning,
        features: design_attrs.features.clone(),
        language: design_attrs.language.clone(),
        variations: design_attrs.variations.clone(),
    }
}

/// Lay out a complete proof document.
pub fn layout_document(
    doc: &proof_model::ProofDocument,
    font_ids: &[FontId],
    registry: &FontRegistry,
) -> Result<LayoutDocument, LayoutError> {
    let inspector = FontInspector::new(registry);
    let resolver = content_resolver::ContentResolver::new(FontInspector::new(registry));
    let mut allocator = PageAllocator::new(doc.page_settings.clone());

    for (section_idx, section) in doc.sections.iter().enumerate() {
        if section.font_indices.is_empty() {
            return Err(LayoutError::NoFonts);
        }

        let primary_font_idx = section.font_indices[0];
        if primary_font_idx >= font_ids.len() {
            return Err(LayoutError::FontIndexOutOfRange(primary_font_idx));
        }
        let primary_font_id = font_ids[primary_font_idx];

        // Resolve content
        let content = resolver.resolve(&section.content, primary_font_id)?;

        // Get font name for headers
        let font_name = registry
            .metadata(primary_font_id)
            .map(|m| format!("{} {}", m.family, m.style))
            .unwrap_or_else(|_| "Unknown".to_string());

        // Render header on first page
        let page_num = allocator.current_page_index() + 1;
        render_header(
            &mut allocator,
            section,
            &font_name,
            page_num,
        );

        // Dispatch to the appropriate layout
        match &section.layout {
            proof_model::LayoutType::Simple => {
                layout_simple::layout(
                    &mut allocator,
                    &content,
                    &section.design_attrs,
                    primary_font_id,
                    registry,
                )?;
            }
            proof_model::LayoutType::Waterfall { sizes } => {
                layout_waterfall::layout(
                    &mut allocator,
                    &content,
                    &section.design_attrs,
                    sizes,
                    primary_font_id,
                    registry,
                )?;
            }
            proof_model::LayoutType::Columns {
                count,
                gutter,
                show_headers,
            } => {
                layout_columns::layout(
                    &mut allocator,
                    &content,
                    &section.design_attrs,
                    *count,
                    *gutter,
                    primary_font_id,
                    registry,
                )?;
            }
            proof_model::LayoutType::GlyphGrid {
                mode,
                show_metrics,
                show_names,
                cell_padding,
            } => {
                layout_glyph_grid::layout(
                    &mut allocator,
                    &content,
                    &section.design_attrs,
                    mode,
                    *show_metrics,
                    *show_names,
                    *cell_padding,
                    primary_font_id,
                    registry,
                )?;
            }
            proof_model::LayoutType::StyleComparison {
                arrangement,
                overflow,
            } => {
                // Collect all font IDs for this section
                let style_font_ids: Vec<FontId> = section
                    .font_indices
                    .iter()
                    .filter_map(|&idx| font_ids.get(idx).copied())
                    .collect();

                layout_style_comparison::layout(
                    &mut allocator,
                    &content,
                    &section.design_attrs,
                    arrangement,
                    overflow,
                    &style_font_ids,
                    registry,
                )?;
            }
            proof_model::LayoutType::Interleave { mode } => {
                let style_font_ids: Vec<FontId> = section
                    .font_indices
                    .iter()
                    .filter_map(|&idx| font_ids.get(idx).copied())
                    .collect();

                let interleaved_doc = layout_interleave::layout(
                    &doc.page_settings,
                    &content,
                    &section.design_attrs,
                    mode,
                    &style_font_ids,
                    registry,
                )?;

                // Merge interleaved pages into the main document
                for page in interleaved_doc.pages {
                    allocator.push_page(page);
                }
            }
            proof_model::LayoutType::ImagePdf { .. } => {
                // Image/PDF layout — not yet implemented
            }
        }
    }

    Ok(allocator.into_document())
}
