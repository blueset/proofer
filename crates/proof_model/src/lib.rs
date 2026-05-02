//! Proof Model — versioned, serializable proof document schema.
//!
//! This is the data model for proof documents. It defines the structure
//! of a proof document as loaded from / saved to JSON.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// The top-level proof document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProofDocument {
    /// Schema version for forward compatibility.
    pub version: String,
    /// Global page settings.
    pub page_settings: PageSettings,
    /// Font references used in this proof.
    pub fonts: Vec<FontReference>,
    /// Ordered list of proof sections.
    pub sections: Vec<Section>,
}

impl ProofDocument {
    pub fn new() -> Self {
        Self {
            version: "1.0".to_string(),
            page_settings: PageSettings::default(),
            fonts: Vec::new(),
            sections: Vec::new(),
        }
    }
}

impl Default for ProofDocument {
    fn default() -> Self {
        Self::new()
    }
}

/// Page dimensions and margins.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageSettings {
    /// Page width in points (1 pt = 1/72 inch).
    pub width: f32,
    /// Page height in points.
    pub height: f32,
    /// Top margin in points.
    pub margin_top: f32,
    /// Bottom margin in points.
    pub margin_bottom: f32,
    /// Left margin in points.
    pub margin_left: f32,
    /// Right margin in points.
    pub margin_right: f32,
}

impl PageSettings {
    /// US Letter size with 0.5 inch margins.
    pub fn letter() -> Self {
        Self {
            width: 612.0,  // 8.5 inches
            height: 792.0, // 11 inches
            margin_top: 36.0,
            margin_bottom: 36.0,
            margin_left: 36.0,
            margin_right: 36.0,
        }
    }

    /// A4 size with 0.5 inch margins.
    pub fn a4() -> Self {
        Self {
            width: 595.28,
            height: 841.89,
            margin_top: 36.0,
            margin_bottom: 36.0,
            margin_left: 36.0,
            margin_right: 36.0,
        }
    }

    /// Usable width (page width minus left and right margins).
    pub fn body_width(&self) -> f32 {
        self.width - self.margin_left - self.margin_right
    }

    /// Usable height (page height minus top and bottom margins).
    pub fn body_height(&self) -> f32 {
        self.height - self.margin_top - self.margin_bottom
    }
}

impl Default for PageSettings {
    fn default() -> Self {
        Self::letter()
    }
}

/// A reference to a font file used in the proof.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FontReference {
    /// Path to the font file.
    pub path: PathBuf,
    /// Face index within a TTC/OTC collection.
    #[serde(default)]
    pub face_index: u32,
    /// Optional checksum for detecting font changes.
    pub checksum: Option<String>,
}

/// A section of the proof document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Section {
    /// Optional section name (shown in headers).
    pub name: Option<String>,
    /// Layout type for this section.
    pub layout: LayoutType,
    /// Content specification.
    pub content: ContentSpec,
    /// Design attributes (size, tracking, features, etc.).
    pub design_attrs: DesignAttributes,
    /// Font instances used in this section (indices into document font list).
    pub font_indices: Vec<usize>,
    /// Header configuration.
    #[serde(default)]
    pub header_config: HeaderConfig,
}

/// Layout type determines how content is arranged on pages.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum LayoutType {
    /// Simple text block(s) at configured sizes.
    Simple,

    /// Same text at cascading sizes.
    Waterfall {
        /// Font sizes to cascade through.
        sizes: Vec<f32>,
    },

    /// Multi-column text flow.
    Columns {
        /// Number of columns.
        count: usize,
        /// Gutter width between columns in points.
        gutter: f32,
        /// Show per-column headers.
        #[serde(default)]
        show_headers: bool,
    },

    /// Glyph grid display.
    GlyphGrid {
        /// Grid or compact mode.
        mode: GlyphGridMode,
        /// Show metric lines (ascender, descender, etc.).
        #[serde(default = "default_true")]
        show_metrics: bool,
        /// Show glyph names/codepoints below each cell.
        #[serde(default = "default_true")]
        show_names: bool,
        /// Cell padding in points.
        #[serde(default = "default_cell_padding")]
        cell_padding: f32,
    },

    /// Side-by-side style comparison.
    StyleComparison {
        /// Arrangement: columns or rows.
        arrangement: ComparisonArrangement,
        /// How to handle overflow.
        overflow: ComparisonOverflow,
    },

    /// Multi-style page interleaving.
    Interleave {
        mode: InterleaveMode,
    },

    /// Embedded reference image.
    ImagePdf {
        source: PathBuf,
    },
}

fn default_true() -> bool {
    true
}

fn default_cell_padding() -> f32 {
    4.0
}

/// Glyph grid display mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GlyphGridMode {
    /// Uniform cell sizes based on max dimensions.
    Grid,
    /// Variable-width cells bin-packed into rows.
    Compact,
}

/// How styles are arranged in a comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ComparisonArrangement {
    Columns,
    Rows,
}

/// How overflow is handled in a style comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ComparisonOverflow {
    /// Clip content that overflows the column/row.
    Truncate,
    /// Synchronize flow so all columns break at the same point.
    Flow,
}

/// Interleave mode for multi-style proofs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InterleaveMode {
    /// All pages of style A, then all pages of style B.
    Proofs,
    /// Section 1 of A, section 1 of B, section 2 of A, ...
    Sections,
    /// Page 1 of A, page 1 of B, page 2 of A, page 2 of B.
    Pages,
}

/// Content specification — what text or glyphs to display.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentSpec {
    /// Literal text content.
    Text { text: String },
    /// Reference to an external text file.
    FileRef { path: PathBuf },
    /// All glyphs in the font.
    AllGlyphs,
    /// Filtered subset of glyphs.
    GlyphFilter {
        /// Unicode script filter (e.g., "Latin", "Cyrillic").
        scripts: Vec<String>,
        /// Unicode block filter.
        blocks: Vec<String>,
        /// Unicode category filter (e.g., "Lu" for uppercase letters).
        categories: Vec<String>,
        /// Explicit codepoint ranges.
        ranges: Vec<(u32, u32)>,
    },
    /// Spacing string patterns.
    SpacingStrings {
        /// Pattern template (e.g., "HnHoHpH").
        pattern: String,
    },
    /// Custom user-provided content.
    Custom { text: String },
}

/// Design attributes controlling typography.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesignAttributes {
    /// Font size in points.
    pub font_size: f32,
    /// Line height override (multiplier of font size).
    pub line_height: Option<f32>,
    /// Additional tracking in em units.
    #[serde(default)]
    pub tracking: f32,
    /// Enable kerning.
    #[serde(default = "default_true")]
    pub kerning: bool,
    /// OpenType feature settings (tag, value).
    #[serde(default)]
    pub features: Vec<(String, u32)>,
    /// Language tag for OpenType shaping.
    pub language: Option<String>,
    /// Variation axis settings (tag, value) for variable fonts.
    #[serde(default)]
    pub variations: Vec<(String, f32)>,
}

impl Default for DesignAttributes {
    fn default() -> Self {
        Self {
            font_size: 12.0,
            line_height: None,
            tracking: 0.0,
            kerning: true,
            features: Vec::new(),
            language: None,
            variations: Vec::new(),
        }
    }
}

/// Header configuration for a section.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeaderConfig {
    /// Show section headers.
    #[serde(default = "default_true")]
    pub show_header: bool,
    /// Show font name in header.
    #[serde(default = "default_true")]
    pub show_font_name: bool,
    /// Show font size in header.
    #[serde(default)]
    pub show_font_size: bool,
    /// Show page numbers.
    #[serde(default = "default_true")]
    pub show_page_numbers: bool,
}

impl Default for HeaderConfig {
    fn default() -> Self {
        Self {
            show_header: true,
            show_font_name: true,
            show_font_size: false,
            show_page_numbers: true,
        }
    }
}

/// Serialize a proof document to JSON.
pub fn to_json(doc: &ProofDocument) -> Result<String, serde_json::Error> {
    serde_json::to_string_pretty(doc)
}

/// Deserialize a proof document from JSON.
pub fn from_json(json: &str) -> Result<ProofDocument, serde_json::Error> {
    serde_json::from_str(json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_roundtrip() {
        let doc = ProofDocument {
            version: "1.0".to_string(),
            page_settings: PageSettings::letter(),
            fonts: vec![FontReference {
                path: PathBuf::from("test.otf"),
                face_index: 0,
                checksum: None,
            }],
            sections: vec![Section {
                name: Some("Waterfall".to_string()),
                layout: LayoutType::Waterfall {
                    sizes: vec![8.0, 10.0, 12.0, 16.0, 24.0, 36.0, 48.0, 72.0],
                },
                content: ContentSpec::Text {
                    text: "The quick brown fox jumps over the lazy dog".to_string(),
                },
                design_attrs: DesignAttributes::default(),
                font_indices: vec![0],
                header_config: HeaderConfig::default(),
            }],
        };

        let json = to_json(&doc).unwrap();
        let parsed = from_json(&json).unwrap();
        assert_eq!(parsed.version, "1.0");
        assert_eq!(parsed.sections.len(), 1);
    }
}
