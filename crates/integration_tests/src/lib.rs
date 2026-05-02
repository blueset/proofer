//! Integration tests for the font proofer pipeline.
//!
//! These tests exercise the full pipeline: load font → build proof doc →
//! resolve content → lay out → render PDF.

#[cfg(test)]
mod tests {
    use font_inspector::FontInspector;
    use font_registry::FontRegistry;
    use layout_ir::LayoutDocument;
    use proof_model::*;
    use std::path::PathBuf;

    fn test_font_path() -> PathBuf {
        // Use Arial from Windows fonts
        PathBuf::from(r"C:\Windows\Fonts\arial.ttf")
    }

    fn load_test_font(registry: &mut FontRegistry) -> layout_ir::FontId {
        registry
            .load_file(&test_font_path(), 0)
            .expect("failed to load test font")
    }

    #[test]
    fn test_font_registry_load() {
        let mut registry = FontRegistry::new();
        let font_id = load_test_font(&mut registry);

        assert!(registry.contains(font_id));
        let meta = registry.metadata(font_id).unwrap();
        assert!(!meta.family.is_empty());
        println!("Loaded font: {} {}", meta.family, meta.style);
    }

    #[test]
    fn test_font_inspector_glyphs() {
        let mut registry = FontRegistry::new();
        let font_id = load_test_font(&mut registry);
        let inspector = FontInspector::new(&registry);

        let glyphs = inspector.enumerate_glyphs(font_id).unwrap();
        assert!(!glyphs.is_empty(), "font should have glyphs");
        println!("Font has {} glyphs", glyphs.len());

        let metrics = inspector.get_metrics(font_id).unwrap();
        assert!(metrics.units_per_em > 0);
        println!(
            "UPM: {}, Ascender: {}, Descender: {}",
            metrics.units_per_em, metrics.ascender, metrics.descender
        );
    }

    #[test]
    fn test_font_inspector_features() {
        let mut registry = FontRegistry::new();
        let font_id = load_test_font(&mut registry);
        let inspector = FontInspector::new(&registry);

        let features = inspector.enumerate_features(font_id).unwrap();
        assert!(!features.is_empty(), "Arial should have OT features");
        println!("Font has {} OT features:", features.len());
        for f in &features {
            println!("  {} ({})", f.name, f.tag);
        }
    }

    #[test]
    fn test_proof_model_roundtrip() {
        let doc = ProofDocument {
            version: "1.0".to_string(),
            page_settings: PageSettings::letter(),
            fonts: vec![FontReference {
                path: test_font_path(),
                face_index: 0,
                checksum: None,
            }],
            sections: vec![
                Section {
                    name: Some("Waterfall".to_string()),
                    layout: LayoutType::Waterfall {
                        sizes: vec![8.0, 12.0, 18.0, 24.0, 36.0, 48.0, 72.0],
                    },
                    content: ContentSpec::Text {
                        text: "Hamburgefonstiv".to_string(),
                    },
                    design_attrs: DesignAttributes::default(),
                    font_indices: vec![0],
                    header_config: HeaderConfig::default(),
                },
                Section {
                    name: Some("Glyph Grid".to_string()),
                    layout: LayoutType::GlyphGrid {
                        mode: GlyphGridMode::Grid,
                        show_metrics: true,
                        show_names: true,
                        cell_padding: 4.0,
                    },
                    content: ContentSpec::AllGlyphs,
                    design_attrs: DesignAttributes {
                        font_size: 24.0,
                        ..Default::default()
                    },
                    font_indices: vec![0],
                    header_config: HeaderConfig::default(),
                },
            ],
        };

        let json = proof_model::to_json(&doc).unwrap();
        let parsed = proof_model::from_json(&json).unwrap();
        assert_eq!(parsed.sections.len(), 2);
    }

    #[test]
    fn test_content_resolver_all_glyphs() {
        let mut registry = FontRegistry::new();
        let font_id = load_test_font(&mut registry);
        let inspector = FontInspector::new(&registry);
        let resolver = content_resolver::ContentResolver::new(FontInspector::new(&registry));

        let content = resolver
            .resolve(&ContentSpec::AllGlyphs, font_id)
            .unwrap();
        match &content {
            content_resolver::ResolvedContent::Glyphs(glyphs) => {
                assert!(!glyphs.is_empty());
                println!("Resolved {} glyphs", glyphs.len());
            }
            _ => panic!("Expected Glyphs, got Text"),
        }
    }

    #[test]
    fn test_text_flow_basic() {
        let mut registry = FontRegistry::new();
        let font_id = load_test_font(&mut registry);

        let style = text_flow::TextStyle {
            font_id,
            font_size: 12.0,
            line_height: None,
            tracking: 0.0,
            kerning: true,
            features: vec![],
            language: None,
            variations: vec![],
        };

        let mut flow =
            text_flow::TextFlow::new("The quick brown fox jumps over the lazy dog.", style, 200.0, &registry)
                .unwrap();

        assert!(flow.has_remaining());
        assert!(flow.line_count() > 0);
        println!("Text shaped into {} lines", flow.line_count());

        let (lines, remaining) = flow.consume_into(100.0);
        assert!(!lines.is_empty());
        println!(
            "Consumed {} lines, remaining height: {:.1}",
            lines.len(),
            remaining
        );
    }

    #[test]
    fn test_simple_layout_to_ir() {
        let mut registry = FontRegistry::new();
        let font_id = load_test_font(&mut registry);

        let doc = ProofDocument {
            version: "1.0".to_string(),
            page_settings: PageSettings::letter(),
            fonts: vec![FontReference {
                path: test_font_path(),
                face_index: 0,
                checksum: None,
            }],
            sections: vec![Section {
                name: Some("Simple Text".to_string()),
                layout: LayoutType::Simple,
                content: ContentSpec::Text {
                    text: "The quick brown fox jumps over the lazy dog. ".repeat(20),
                },
                design_attrs: DesignAttributes {
                    font_size: 12.0,
                    ..Default::default()
                },
                font_indices: vec![0],
                header_config: HeaderConfig::default(),
            }],
        };

        let layout_doc = page_layout::layout_document(&doc, &[font_id], &registry).unwrap();
        assert!(layout_doc.page_count() >= 1);
        println!("Simple layout: {} pages", layout_doc.page_count());

        // Check that pages have commands
        for (i, page) in layout_doc.pages.iter().enumerate() {
            assert!(
                !page.commands.is_empty(),
                "Page {} should have commands",
                i
            );
        }
    }

    #[test]
    fn test_waterfall_layout() {
        let mut registry = FontRegistry::new();
        let font_id = load_test_font(&mut registry);

        let doc = ProofDocument {
            version: "1.0".to_string(),
            page_settings: PageSettings::letter(),
            fonts: vec![FontReference {
                path: test_font_path(),
                face_index: 0,
                checksum: None,
            }],
            sections: vec![Section {
                name: Some("Waterfall".to_string()),
                layout: LayoutType::Waterfall {
                    sizes: vec![8.0, 10.0, 12.0, 16.0, 24.0, 36.0, 48.0, 72.0],
                },
                content: ContentSpec::Text {
                    text: "Hamburgefonstiv".to_string(),
                },
                design_attrs: DesignAttributes::default(),
                font_indices: vec![0],
                header_config: HeaderConfig::default(),
            }],
        };

        let layout_doc = page_layout::layout_document(&doc, &[font_id], &registry).unwrap();
        assert!(layout_doc.page_count() >= 1);
        println!("Waterfall layout: {} pages", layout_doc.page_count());
    }

    #[test]
    fn test_columns_layout() {
        let mut registry = FontRegistry::new();
        let font_id = load_test_font(&mut registry);

        let doc = ProofDocument {
            version: "1.0".to_string(),
            page_settings: PageSettings::letter(),
            fonts: vec![FontReference {
                path: test_font_path(),
                face_index: 0,
                checksum: None,
            }],
            sections: vec![Section {
                name: Some("Three Columns".to_string()),
                layout: LayoutType::Columns {
                    count: 3,
                    gutter: 12.0,
                    show_headers: false,
                },
                content: ContentSpec::Text {
                    text: "Lorem ipsum dolor sit amet, consectetur adipiscing elit. ".repeat(50),
                },
                design_attrs: DesignAttributes {
                    font_size: 10.0,
                    ..Default::default()
                },
                font_indices: vec![0],
                header_config: HeaderConfig::default(),
            }],
        };

        let layout_doc = page_layout::layout_document(&doc, &[font_id], &registry).unwrap();
        assert!(layout_doc.page_count() >= 1);
        println!("Columns layout: {} pages", layout_doc.page_count());
    }

    #[test]
    fn test_glyph_grid_layout() {
        let mut registry = FontRegistry::new();
        let font_id = load_test_font(&mut registry);

        let doc = ProofDocument {
            version: "1.0".to_string(),
            page_settings: PageSettings::letter(),
            fonts: vec![FontReference {
                path: test_font_path(),
                face_index: 0,
                checksum: None,
            }],
            sections: vec![Section {
                name: Some("Glyph Grid".to_string()),
                layout: LayoutType::GlyphGrid {
                    mode: GlyphGridMode::Grid,
                    show_metrics: true,
                    show_names: true,
                    cell_padding: 4.0,
                },
                content: ContentSpec::AllGlyphs,
                design_attrs: DesignAttributes {
                    font_size: 24.0,
                    ..Default::default()
                },
                font_indices: vec![0],
                header_config: HeaderConfig::default(),
            }],
        };

        let layout_doc = page_layout::layout_document(&doc, &[font_id], &registry).unwrap();
        assert!(layout_doc.page_count() >= 1);
        println!("Glyph grid layout: {} pages", layout_doc.page_count());
    }

    #[test]
    fn test_style_comparison_layout() {
        let mut registry = FontRegistry::new();
        let font_id = load_test_font(&mut registry);
        // Use same font twice (in real use these would be different styles)
        let font_id2 = registry.load_file(&test_font_path(), 0).unwrap();

        let doc = ProofDocument {
            version: "1.0".to_string(),
            page_settings: PageSettings::letter(),
            fonts: vec![
                FontReference {
                    path: test_font_path(),
                    face_index: 0,
                    checksum: None,
                },
                FontReference {
                    path: test_font_path(),
                    face_index: 0,
                    checksum: None,
                },
            ],
            sections: vec![Section {
                name: Some("Style Comparison".to_string()),
                layout: LayoutType::StyleComparison {
                    arrangement: ComparisonArrangement::Columns,
                    overflow: ComparisonOverflow::Flow,
                },
                content: ContentSpec::Text {
                    text: "The quick brown fox jumps over the lazy dog. ".repeat(10),
                },
                design_attrs: DesignAttributes {
                    font_size: 12.0,
                    ..Default::default()
                },
                font_indices: vec![0, 1],
                header_config: HeaderConfig::default(),
            }],
        };

        let layout_doc =
            page_layout::layout_document(&doc, &[font_id, font_id2], &registry).unwrap();
        assert!(layout_doc.page_count() >= 1);
        println!(
            "Style comparison layout: {} pages",
            layout_doc.page_count()
        );
    }

    #[test]
    fn test_interleave_layout() {
        let mut registry = FontRegistry::new();
        let font_id = load_test_font(&mut registry);
        let font_id2 = registry.load_file(&test_font_path(), 0).unwrap();

        let doc = ProofDocument {
            version: "1.0".to_string(),
            page_settings: PageSettings::letter(),
            fonts: vec![
                FontReference {
                    path: test_font_path(),
                    face_index: 0,
                    checksum: None,
                },
                FontReference {
                    path: test_font_path(),
                    face_index: 0,
                    checksum: None,
                },
            ],
            sections: vec![Section {
                name: Some("Interleave".to_string()),
                layout: LayoutType::Interleave {
                    mode: InterleaveMode::Pages,
                },
                content: ContentSpec::Text {
                    text: "The quick brown fox jumps over the lazy dog. ".repeat(30),
                },
                design_attrs: DesignAttributes {
                    font_size: 14.0,
                    ..Default::default()
                },
                font_indices: vec![0, 1],
                header_config: HeaderConfig::default(),
            }],
        };

        let layout_doc =
            page_layout::layout_document(&doc, &[font_id, font_id2], &registry).unwrap();
        assert!(layout_doc.page_count() >= 2);
        println!("Interleave layout: {} pages", layout_doc.page_count());
    }

    #[test]
    fn test_end_to_end_pdf_generation() {
        let mut registry = FontRegistry::new();
        let font_id = load_test_font(&mut registry);

        let doc = ProofDocument {
            version: "1.0".to_string(),
            page_settings: PageSettings::letter(),
            fonts: vec![FontReference {
                path: test_font_path(),
                face_index: 0,
                checksum: None,
            }],
            sections: vec![
                Section {
                    name: Some("Sample Text".to_string()),
                    layout: LayoutType::Simple,
                    content: ContentSpec::Text {
                        text: "The quick brown fox jumps over the lazy dog.".to_string(),
                    },
                    design_attrs: DesignAttributes {
                        font_size: 24.0,
                        ..Default::default()
                    },
                    font_indices: vec![0],
                    header_config: HeaderConfig::default(),
                },
                Section {
                    name: Some("Waterfall".to_string()),
                    layout: LayoutType::Waterfall {
                        sizes: vec![8.0, 12.0, 18.0, 24.0, 36.0, 48.0],
                    },
                    content: ContentSpec::Text {
                        text: "Hamburgefonstiv".to_string(),
                    },
                    design_attrs: DesignAttributes::default(),
                    font_indices: vec![0],
                    header_config: HeaderConfig::default(),
                },
            ],
        };

        // Layout
        let layout_doc = page_layout::layout_document(&doc, &[font_id], &registry).unwrap();
        assert!(layout_doc.page_count() >= 1);

        // Render to PDF
        let pdf_bytes = pdf_renderer::render_to_pdf(&layout_doc, &registry).unwrap();
        assert!(!pdf_bytes.is_empty());
        assert!(pdf_bytes.len() > 100); // PDF should have meaningful content
        // Check PDF header
        assert_eq!(&pdf_bytes[0..5], b"%PDF-");
        println!(
            "Generated PDF: {} bytes, {} pages",
            pdf_bytes.len(),
            layout_doc.page_count()
        );
    }
}
