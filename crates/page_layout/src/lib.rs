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
    /// Current section header info for repeating on new pages.
    current_header: Option<HeaderContext>,
    /// Label font ID for measuring header text.
    label_font_id: Option<FontId>,
    /// Cached label font data for measurement (Arc-shared with registry).
    label_font_data: Option<std::sync::Arc<Vec<u8>>>,
}

/// Context for repeating section headers on new pages.
struct HeaderContext {
    section_name: Option<String>,
    font_name: String,
    header_config: proof_model::HeaderConfig,
}

impl PageAllocator {
    pub fn new(page_settings: PageSettings, registry: &FontRegistry) -> Self {
        let label_font_id = registry.label_font_id();
        let label_font_data = label_font_id
            .and_then(|id| registry.font_data(id).ok())
            .map(|(data, _)| data);
        let mut allocator = Self {
            page_settings,
            pages: Vec::new(),
            cursor_y: 0.0,
            current_header: None,
            label_font_id,
            label_font_data,
        };
        allocator.new_page();
        allocator
    }

    /// Start a new page and reset cursor. Renders the current section
    /// header automatically if one is set.
    pub fn new_page(&mut self) {
        let page = Page::new(self.page_settings.width(), self.page_settings.height());
        self.pages.push(page);
        self.cursor_y = self.page_settings.margin_top();

        // Render header on the new page if we have section context
        if let Some(ctx) = &self.current_header {
            if ctx.header_config.show_header {
                let page_num = self.pages.len();
                self.render_header_inner(
                    ctx.section_name.clone(),
                    ctx.font_name.clone(),
                    ctx.header_config.clone(),
                    page_num,
                );
            }
        }
    }

    /// Set the current section header context. Call this at the start
    /// of each section so that new pages get the right header.
    pub fn set_section_header(
        &mut self,
        section_name: Option<String>,
        font_name: String,
        header_config: proof_model::HeaderConfig,
    ) {
        self.current_header = Some(HeaderContext {
            section_name,
            font_name,
            header_config,
        });
    }

    /// Clear the section header context.
    pub fn clear_section_header(&mut self) {
        self.current_header = None;
    }

    /// Measure label text width using the bundled label font.
    fn measure_label(&self, text: &str, size: f32) -> f32 {
        use skrifa::MetadataProvider;
        if let Some(data) = &self.label_font_data {
            if let Ok(font) = skrifa::FontRef::new(data) {
                let s = skrifa::prelude::Size::new(size);
                let gm = font.glyph_metrics(s, skrifa::prelude::LocationRef::default());
                let cm = font.charmap();
                return text.chars().map(|ch| {
                    cm.map(ch).and_then(|gid| gm.advance_width(gid)).unwrap_or(size * 0.3)
                }).sum();
            }
        }
        // Fallback: approximate
        text.len() as f32 * size * 0.5
    }

    fn render_header_inner(
        &mut self,
        section_name: Option<String>,
        font_name: String,
        config: proof_model::HeaderConfig,
        page_number: usize,
    ) {
        let header_size: f32 = 8.0;
        let header_y = self.cursor_y;
        let left = self.body_left();
        let width = self.body_width();

        // Section name — left aligned
        if let Some(name) = &section_name {
            self.push_command(DrawCommand::Label {
                text: name.clone(),
                x: left,
                y: header_y + header_size,
                size: header_size,
                color: Color::gray(0.4),
            });
        }

        // Font name — centered
        if config.show_font_name {
            let text_width = self.measure_label(&font_name, header_size);
            self.push_command(DrawCommand::Label {
                text: font_name,
                x: left + (width - text_width) / 2.0,
                y: header_y + header_size,
                size: header_size,
                color: Color::gray(0.4),
            });
        }

        // Page number — right aligned
        if config.show_page_numbers {
            let page_str = format!("{page_number}");
            let text_width = self.measure_label(&page_str, header_size);
            self.push_command(DrawCommand::Label {
                text: page_str,
                x: left + width - text_width,
                y: header_y + header_size,
                size: header_size,
                color: Color::gray(0.4),
            });
        }

        let line_y = header_y + header_size + 4.0;
        self.push_command(DrawCommand::Line {
            x1: left,
            y1: line_y,
            x2: left + width,
            y2: line_y,
            stroke: StrokeStyle::hairline(Color::LIGHT_GRAY),
        });

        self.cursor_y += header_size + 12.0;
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

/// A resolved style variant with concrete font ID and design attributes.
pub struct ResolvedStyleVariant {
    pub font_id: FontId,
    pub design_attrs: proof_model::DesignAttributes,
    pub label: Option<String>,
}

/// Resolve style variants for a section.
/// If the section has explicit `styles`, use those.
/// Otherwise, create one variant per font_index with the base design_attrs.
fn resolve_style_variants(
    section: &proof_model::Section,
    font_ids: &[FontId],
) -> Vec<ResolvedStyleVariant> {
    let default_font_id = section
        .font_indices
        .first()
        .and_then(|&idx| font_ids.get(idx).copied())
        .unwrap_or(FontId(0));

    if !section.styles.is_empty() {
        section
            .styles
            .iter()
            .map(|variant| {
                let font_id = variant
                    .font_index
                    .and_then(|idx| font_ids.get(idx).copied())
                    .unwrap_or(default_font_id);
                let design_attrs = variant.design_attrs.apply_to(&section.design_attrs);
                ResolvedStyleVariant {
                    font_id,
                    design_attrs,
                    label: variant.label.clone(),
                }
            })
            .collect()
    } else {
        // Legacy: one variant per font_index
        section
            .font_indices
            .iter()
            .filter_map(|&idx| {
                font_ids.get(idx).map(|&fid| ResolvedStyleVariant {
                    font_id: fid,
                    design_attrs: section.design_attrs.clone(),
                    label: None,
                })
            })
            .collect()
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
    let mut allocator = PageAllocator::new(doc.page_settings.clone(), registry);

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
            .map(|m| format!("{}", m.family))
            .unwrap_or_else(|_| "Unknown".to_string());

        // Begin each section on a new page (except the very first section
        // which already starts on page 1).
        // Set header context BEFORE new_page so the correct header renders.
        allocator.set_section_header(
            section.name.clone(),
            font_name.clone(),
            section.header_config.clone(),
        );

        if section_idx > 0 {
            allocator.new_page();
        } else {
            // First section: manually render header on page 1
            // (new_page was already called in PageAllocator::new, before
            // any header context was set)
            allocator.render_header_inner(
                section.name.clone(),
                font_name,
                section.header_config.clone(),
                allocator.pages.len(),
            );
        }

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
            proof_model::LayoutType::Waterfall { sizes, spacing } => {
                layout_waterfall::layout(
                    &mut allocator,
                    &content,
                    &section.design_attrs,
                    sizes,
                    *spacing,
                    primary_font_id,
                    registry,
                )?;
            }
            proof_model::LayoutType::Columns {
                count,
                gutter,
                show_headers: _,
                column_label,
            } => {
                layout_columns::layout(
                    &mut allocator,
                    &content,
                    &section.design_attrs,
                    *count,
                    *gutter,
                    column_label.as_deref(),
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
                spacing,
            } => {
                // Build resolved style variants
                let style_variants = resolve_style_variants(section, font_ids);

                layout_style_comparison::layout(
                    &mut allocator,
                    &content,
                    &section.design_attrs,
                    arrangement,
                    overflow,
                    *spacing,
                    &style_variants,
                    registry,
                )?;
            }
            proof_model::LayoutType::Interleave { mode } => {
                let style_variants = resolve_style_variants(section, font_ids);

                let interleaved_doc = layout_interleave::layout(
                    &doc.page_settings,
                    &content,
                    &section.design_attrs,
                    mode,
                    &style_variants,
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
