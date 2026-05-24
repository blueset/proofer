//! Proof Model — versioned, serializable proof document schema.
//!
//! This is the data model for proof documents. It defines the structure
//! of a proof document as loaded from / saved to YAML or JSON.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// The top-level proof document.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ProofDocument {
    /// Schema version for forward compatibility.
    #[serde(default = "default_version")]
    pub version: String,
    /// Global page settings.
    #[serde(default)]
    pub page_settings: PageSettings,
    /// Font references used in this proof.
    pub fonts: Vec<FontReference>,
    /// Ordered list of proof sections.
    pub sections: Vec<Section>,
}

fn default_version() -> String {
    "1.0".to_string()
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

    /// Resolve all relative paths against a base directory.
    pub fn resolve_paths(&mut self, base: &std::path::Path) {
        for font in &mut self.fonts {
            if font.path.is_relative() {
                font.path = base.join(&font.path);
            }
        }
        for section in &mut self.sections {
            match &mut section.content {
                ContentSpec::FileRef { path } => {
                    if path.is_relative() {
                        *path = base.join(&*path);
                    }
                }
                _ => {}
            }
            if let LayoutType::ImagePdf { source } = &mut section.layout {
                if source.is_relative() {
                    *source = base.join(&*source);
                }
            }
        }
    }

    /// Validate the document and return any errors found.
    pub fn validate(&self) -> Vec<String> {
        let mut errors = Vec::new();
        let ps = &self.page_settings;
        if ps.width() <= 0.0 || ps.height() <= 0.0 {
            errors.push("page dimensions must be positive".into());
        }
        if ps.margin_top() < 0.0
            || ps.margin_bottom() < 0.0
            || ps.margin_left() < 0.0
            || ps.margin_right() < 0.0
        {
            errors.push("margins must be non-negative".into());
        }
        if self.fonts.is_empty() {
            errors.push("at least one font is required".into());
        }
        for (i, section) in self.sections.iter().enumerate() {
            for &idx in &section.font_indices {
                if idx >= self.fonts.len() {
                    errors.push(format!(
                        "section {}: font_indices[{}] out of range (have {} fonts)",
                        i,
                        idx,
                        self.fonts.len()
                    ));
                }
            }
            if section.font_indices.is_empty() {
                errors.push(format!("section {}: font_indices is empty", i));
            }
            if section.design_attrs.font_size <= 0.0 {
                errors.push(format!("section {}: font_size must be positive", i));
            }
            for tag in section.design_attrs.features.keys() {
                if tag.len() != 4 {
                    errors.push(format!(
                        "section {}: feature tag '{}' must be exactly 4 characters",
                        i, tag
                    ));
                }
            }
            for tag in section.design_attrs.variations.keys() {
                if tag.len() != 4 {
                    errors.push(format!(
                        "section {}: variation tag '{}' must be exactly 4 characters",
                        i, tag
                    ));
                }
            }
            if let LayoutType::GlyphGrid {
                subgrid_x,
                subgrid_y,
                ..
            } = &section.layout
            {
                let validate_axis = |axis: &SubgridAxis, side: &str, errs: &mut Vec<String>| {
                    if axis.axis.len() != 4 {
                        errs.push(format!(
                            "section {}: GlyphGrid subgrid_{} axis tag '{}' must be exactly 4 characters",
                            i, side, axis.axis
                        ));
                    }
                    if axis.values.is_empty() {
                        errs.push(format!(
                            "section {}: GlyphGrid subgrid_{} must have at least one value",
                            i, side
                        ));
                    }
                };
                if let Some(sx) = subgrid_x {
                    validate_axis(sx, "x", &mut errors);
                }
                if let Some(sy) = subgrid_y {
                    validate_axis(sy, "y", &mut errors);
                }
                if let (Some(sx), Some(sy)) = (subgrid_x, subgrid_y) {
                    if sx.axis == sy.axis {
                        errors.push(format!(
                            "section {}: GlyphGrid subgrid_x and subgrid_y must use different axes (both are '{}')",
                            i, sx.axis
                        ));
                    }
                }
            }
        }
        errors
    }
}

impl Default for ProofDocument {
    fn default() -> Self {
        Self::new()
    }
}

/// Page orientation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Orientation {
    Portrait,
    Landscape,
}

impl Default for Orientation {
    fn default() -> Self {
        Self::Landscape
    }
}

/// Page dimensions and margins. Can be specified as a preset or custom values.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum PageSettings {
    /// A named preset (e.g., "letter", "a4").
    Preset {
        preset: PagePreset,
        #[serde(default)]
        orientation: Orientation,
    },
    /// Custom dimensions in points.
    Custom {
        width: f32,
        height: f32,
        #[serde(default = "default_margin")]
        margin_top: f32,
        #[serde(default = "default_margin")]
        margin_bottom: f32,
        #[serde(default = "default_margin")]
        margin_left: f32,
        #[serde(default = "default_margin")]
        margin_right: f32,
    },
}

fn default_margin() -> f32 {
    36.0
}

/// Named page presets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum PagePreset {
    Letter,
    A4,
    A3,
    Tabloid,
}

impl PageSettings {
    pub fn letter() -> Self {
        PageSettings::Preset {
            preset: PagePreset::Letter,
            orientation: Orientation::default(),
        }
    }

    pub fn a4() -> Self {
        PageSettings::Preset {
            preset: PagePreset::A4,
            orientation: Orientation::default(),
        }
    }

    /// Resolve to concrete dimensions (orientation-aware).
    pub fn width(&self) -> f32 {
        match self {
            PageSettings::Preset {
                preset,
                orientation,
            } => match orientation {
                Orientation::Portrait => preset.short_edge(),
                Orientation::Landscape => preset.long_edge(),
            },
            PageSettings::Custom { width, .. } => *width,
        }
    }

    pub fn height(&self) -> f32 {
        match self {
            PageSettings::Preset {
                preset,
                orientation,
            } => match orientation {
                Orientation::Portrait => preset.long_edge(),
                Orientation::Landscape => preset.short_edge(),
            },
            PageSettings::Custom { height, .. } => *height,
        }
    }

    pub fn margin_top(&self) -> f32 {
        match self {
            PageSettings::Preset { .. } => 36.0,
            PageSettings::Custom { margin_top, .. } => *margin_top,
        }
    }

    pub fn margin_bottom(&self) -> f32 {
        match self {
            PageSettings::Preset { .. } => 36.0,
            PageSettings::Custom { margin_bottom, .. } => *margin_bottom,
        }
    }

    pub fn margin_left(&self) -> f32 {
        match self {
            PageSettings::Preset { .. } => 36.0,
            PageSettings::Custom { margin_left, .. } => *margin_left,
        }
    }

    pub fn margin_right(&self) -> f32 {
        match self {
            PageSettings::Preset { .. } => 36.0,
            PageSettings::Custom { margin_right, .. } => *margin_right,
        }
    }

    /// Usable width (page width minus left and right margins).
    pub fn body_width(&self) -> f32 {
        self.width() - self.margin_left() - self.margin_right()
    }

    /// Usable height (page height minus top and bottom margins).
    pub fn body_height(&self) -> f32 {
        self.height() - self.margin_top() - self.margin_bottom()
    }
}

impl PagePreset {
    /// The shorter dimension (portrait width / landscape height).
    pub fn short_edge(self) -> f32 {
        match self {
            Self::Letter => 612.0, // 8.5"
            Self::A4 => 595.28,
            Self::A3 => 841.89,
            Self::Tabloid => 792.0, // 11"
        }
    }

    /// The longer dimension (portrait height / landscape width).
    pub fn long_edge(self) -> f32 {
        match self {
            Self::Letter => 792.0, // 11"
            Self::A4 => 841.89,
            Self::A3 => 1190.55,
            Self::Tabloid => 1224.0, // 17"
        }
    }
}

impl Default for PageSettings {
    fn default() -> Self {
        Self::letter()
    }
}

/// A reference to a font file used in the proof.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FontReference {
    /// Path to the font file (relative to the .proof.yaml file).
    pub path: PathBuf,
    /// Face index within a TTC/OTC collection.
    #[serde(default)]
    pub face_index: u32,
    /// Optional checksum for detecting font changes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checksum: Option<String>,
}

/// A section of the proof document.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Section {
    /// Optional section name (shown in headers).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Layout type for this section.
    pub layout: LayoutType,
    /// Content specification.
    pub content: ContentSpec,
    /// Base design attributes (size, tracking, features, etc.).
    #[serde(default)]
    pub design_attrs: DesignAttributes,
    /// Font indices into the document's font list.
    pub font_indices: Vec<usize>,
    /// Header configuration.
    #[serde(default)]
    pub header_config: HeaderConfig,
    /// Style variants for StyleComparison/Interleave layouts.
    /// Each variant can override design_attrs and/or font_index.
    /// If empty, the layout uses font_indices with the base design_attrs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub styles: Vec<StyleVariant>,
}

/// A style variant for comparison/interleave layouts.
/// Overrides the section's base design_attrs and/or font.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct StyleVariant {
    /// Optional label for this style (shown in headers).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Font index override (into document font list).
    /// If not set, uses the first entry from the section's font_indices.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub font_index: Option<usize>,
    /// Design attribute overrides. Only specified fields override the base.
    #[serde(default)]
    pub design_attrs: DesignAttributeOverrides,
}

/// Partial design attribute overrides for style variants.
/// Any field set to Some overrides the section's base value.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct DesignAttributeOverrides {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub font_size: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_height: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tracking: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kerning: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub features: Option<BTreeMap<String, u32>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variations: Option<BTreeMap<String, f32>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_align: Option<TextAlign>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_limit: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub y_offset: Option<f32>,
}

impl DesignAttributeOverrides {
    /// Apply overrides to a base DesignAttributes, returning a new copy.
    pub fn apply_to(&self, base: &DesignAttributes) -> DesignAttributes {
        DesignAttributes {
            font_size: self.font_size.unwrap_or(base.font_size),
            line_height: self.line_height.or(base.line_height),
            tracking: self.tracking.unwrap_or(base.tracking),
            kerning: self.kerning.unwrap_or(base.kerning),
            features: self
                .features
                .clone()
                .unwrap_or_else(|| base.features.clone()),
            language: self.language.clone().or_else(|| base.language.clone()),
            variations: self
                .variations
                .clone()
                .unwrap_or_else(|| base.variations.clone()),
            text_align: self.text_align.unwrap_or(base.text_align),
            line_limit: self.line_limit.or(base.line_limit),
            y_offset: self.y_offset.unwrap_or(base.y_offset),
        }
    }
}

/// Layout type determines how content is arranged on pages.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type")]
pub enum LayoutType {
    /// Simple text block(s) at configured sizes.
    Simple,

    /// Same text at cascading sizes.
    Waterfall {
        /// Font sizes to cascade through.
        sizes: Vec<f32>,
        /// Spacing between size blocks in points.
        #[serde(default = "default_waterfall_spacing")]
        spacing: f32,
        /// Optional label text appended after size labels on each page.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
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
        /// Label text repeated at the top of each column.
        #[serde(skip_serializing_if = "Option::is_none")]
        column_label: Option<String>,
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
        /// Optional variation-axis subgrid along the X (column) direction.
        /// When set, each glyph cell becomes a row of sub-cells, one per value.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subgrid_x: Option<SubgridAxis>,
        /// Optional variation-axis subgrid along the Y (row) direction.
        /// When set, each glyph cell becomes a column of sub-cells, one per value.
        /// Combined with `subgrid_x`, the cell becomes a 2D matrix of variations.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subgrid_y: Option<SubgridAxis>,
    },

    /// Side-by-side style comparison.
    StyleComparison {
        arrangement: ComparisonArrangement,
        overflow: ComparisonOverflow,
        /// Spacing between rows/columns in points.
        #[serde(default = "default_comparison_spacing")]
        spacing: f32,
        /// Maximum columns per row (Column arrangement only).
        /// If more variants exist, they wrap to additional rows on new pages.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_columns: Option<usize>,
    },

    /// Multi-style page interleaving.
    Interleave { mode: InterleaveMode },

    /// Embedded reference image.
    ImagePdf { source: PathBuf },
}

fn default_true() -> bool {
    true
}

fn default_cell_padding() -> f32 {
    4.0
}

fn default_waterfall_spacing() -> f32 {
    8.0
}

fn default_comparison_spacing() -> f32 {
    16.0
}

/// Glyph grid display mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum GlyphGridMode {
    Grid,
    Compact,
}

/// A single variation axis used to drive a glyph-grid subgrid.
///
/// When attached to `LayoutType::GlyphGrid` via `subgrid_x` / `subgrid_y`,
/// each glyph cell is rendered as a matrix of instances by combining the
/// row/column values with `design_attrs.variations` (subgrid values silently
/// override matching tags).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SubgridAxis {
    /// Variation axis tag (4 ASCII characters, e.g. "wght" or "wdth").
    pub axis: String,
    /// Axis values, in display order. Must contain at least one value.
    pub values: Vec<f32>,
}

/// How styles are arranged in a comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum ComparisonArrangement {
    Columns,
    Rows,
}

/// How overflow is handled in a style comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum ComparisonOverflow {
    Truncate,
    Flow,
}

/// Interleave mode for multi-style proofs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum InterleaveMode {
    Proofs,
    Sections,
    Pages,
}

/// Content specification — what text or glyphs to display.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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
        #[serde(default)]
        scripts: Vec<String>,
        #[serde(default)]
        blocks: Vec<String>,
        #[serde(default)]
        categories: Vec<String>,
        #[serde(default)]
        ranges: Vec<(u32, u32)>,
    },
    /// Repeating pattern — generates text by substituting glyphs
    /// into templates, between context strings, or wrapped in pairs.
    Pattern {
        /// Characters to iterate over. Each produces one output cycle.
        glyphs: GlyphSet,
        /// Template strings where the placeholder is replaced with each glyph.
        /// Mutually exclusive with `between` and `wrap`.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        templates: Vec<String>,
        /// Context strings — test glyphs are inserted between repetitions
        /// of each context string. One output line per context string.
        /// Mutually exclusive with `templates` and `wrap`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        between: Option<GlyphSet>,
        /// Before/after wrap pairs — each test glyph is wrapped by all pairs.
        /// Mutually exclusive with `templates` and `between`.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        wrap: Vec<(String, String)>,
        /// Placeholder string in templates (default: "?").
        #[serde(default = "default_placeholder")]
        placeholder: String,
        /// Separator between output lines (default: newline).
        #[serde(default)]
        separator: PatternSeparator,
    },
    /// Concatenate multiple content specs into one.
    Concat {
        /// Content parts to concatenate in order.
        parts: Vec<ContentSpec>,
        /// Joiner string between parts (default: "\n").
        #[serde(default = "default_concat_joiner")]
        joiner: String,
    },
    /// Custom user-provided content.
    Custom { text: String },
}

fn default_placeholder() -> String {
    "?".to_string()
}

fn default_concat_joiner() -> String {
    "\n".to_string()
}

/// A set of glyphs — either a literal string or a preset from the font.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum GlyphSet {
    /// Literal characters.
    Literal(String),
    /// Characters split by spaces (each element is a string context).
    List(Vec<String>),
    /// A preset from the font's character map.
    Preset { preset: GlyphPreset },
}

/// Preset glyph sets derived from the font's character map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum GlyphPreset {
    Uppercase,
    Lowercase,
    Digits,
    All,
}

/// Separator between pattern output lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "lowercase")]
pub enum PatternSeparator {
    #[default]
    Newline,
    Space,
    None,
}

/// Text alignment for a text block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "lowercase")]
pub enum TextAlign {
    #[default]
    Left,
    Center,
    Right,
    Justified,
}

/// Design attributes controlling typography.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DesignAttributes {
    /// Font size in points.
    #[serde(default = "default_font_size")]
    pub font_size: f32,
    /// Line height override (multiplier of font size).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_height: Option<f32>,
    /// Additional tracking in em units.
    #[serde(default)]
    pub tracking: f32,
    /// Enable kerning.
    #[serde(default = "default_true")]
    pub kerning: bool,
    /// OpenType feature settings: tag → value (e.g., smcp: 1).
    #[serde(default)]
    pub features: BTreeMap<String, u32>,
    /// Language tag for OpenType shaping.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Variation axis settings: tag → value (e.g., wght: 700).
    #[serde(default)]
    pub variations: BTreeMap<String, f32>,
    /// Text alignment: left, center, right, or justified.
    #[serde(default)]
    pub text_align: TextAlign,
    /// Maximum number of lines to show. None = no limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_limit: Option<usize>,
    /// Extra top padding (in points) between header/top edge and text on each page.
    #[serde(default)]
    pub y_offset: f32,
}

fn default_font_size() -> f32 {
    12.0
}

impl Default for DesignAttributes {
    fn default() -> Self {
        Self {
            font_size: 12.0,
            line_height: None,
            tracking: 0.0,
            kerning: true,
            features: BTreeMap::new(),
            language: None,
            variations: BTreeMap::new(),
            text_align: TextAlign::Left,
            line_limit: None,
            y_offset: 0.0,
        }
    }
}

/// Header configuration for a section.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct HeaderConfig {
    #[serde(default = "default_true")]
    pub show_header: bool,
    #[serde(default = "default_true")]
    pub show_font_name: bool,
    #[serde(default = "default_true")]
    pub show_font_version: bool,
    #[serde(default = "default_true")]
    pub show_datetime: bool,
    #[serde(default = "default_true")]
    pub show_page_numbers: bool,
}

impl Default for HeaderConfig {
    fn default() -> Self {
        Self {
            show_header: true,
            show_font_name: true,
            show_font_version: true,
            show_datetime: true,
            show_page_numbers: true,
        }
    }
}

// ── Serialization helpers ───────────────────────────────────────────

pub fn to_json(doc: &ProofDocument) -> Result<String, serde_json::Error> {
    serde_json::to_string_pretty(doc)
}

pub fn from_json(json: &str) -> Result<ProofDocument, serde_json::Error> {
    serde_json::from_str(json)
}

pub fn to_yaml(doc: &ProofDocument) -> Result<String, serde_yaml::Error> {
    serde_yaml::to_string(doc)
}

pub fn from_yaml(yaml: &str) -> Result<ProofDocument, serde_yaml::Error> {
    serde_yaml::from_str(yaml)
}

/// Generate the JSON Schema for ProofDocument.
pub fn json_schema() -> schemars::schema::RootSchema {
    schemars::schema_for!(ProofDocument)
}

/// Generate the JSON Schema as a pretty-printed JSON string.
pub fn json_schema_string() -> String {
    serde_json::to_string_pretty(&json_schema()).unwrap()
}

/// Generate an example proof document.
pub fn example_document() -> ProofDocument {
    ProofDocument {
        version: "1.0".to_string(),
        page_settings: PageSettings::letter(),
        fonts: vec![FontReference {
            path: PathBuf::from("./MyFont-Regular.otf"),
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
                styles: vec![],
            },
            Section {
                name: Some("Waterfall".to_string()),
                layout: LayoutType::Waterfall {
                    sizes: vec![8.0, 10.0, 12.0, 16.0, 24.0, 36.0, 48.0, 72.0],
                    spacing: 8.0,
                    label: None,
                },
                content: ContentSpec::Text {
                    text: "Hamburgefonstiv".to_string(),
                },
                design_attrs: DesignAttributes::default(),
                font_indices: vec![0],
                header_config: HeaderConfig::default(),
                styles: vec![],
            },
            Section {
                name: Some("Glyph Grid".to_string()),
                layout: LayoutType::GlyphGrid {
                    mode: GlyphGridMode::Grid,
                    show_metrics: true,
                    show_names: true,
                    cell_padding: 4.0,
                    subgrid_x: None,
                    subgrid_y: None,
                },
                content: ContentSpec::AllGlyphs,
                design_attrs: DesignAttributes {
                    font_size: 32.0,
                    features: BTreeMap::from([("smcp".to_string(), 1)]),
                    ..Default::default()
                },
                font_indices: vec![0],
                header_config: HeaderConfig::default(),
                styles: vec![],
            },
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_json_roundtrip() {
        let doc = example_document();
        let json = to_json(&doc).unwrap();
        let parsed = from_json(&json).unwrap();
        assert_eq!(parsed.version, "1.0");
        assert_eq!(parsed.sections.len(), 3);
    }

    #[test]
    fn test_yaml_roundtrip() {
        let doc = example_document();
        let yaml = to_yaml(&doc).unwrap();
        let parsed = from_yaml(&yaml).unwrap();
        assert_eq!(parsed.version, "1.0");
        assert_eq!(parsed.sections.len(), 3);
    }

    #[test]
    fn test_schema_generation() {
        let schema = json_schema_string();
        assert!(schema.contains("ProofDocument"));
        assert!(schema.contains("LayoutType"));
        assert!(schema.contains("ContentSpec"));
    }

    #[test]
    fn header_config_defaults_show_font_version() {
        assert!(HeaderConfig::default().show_font_version);

        let config: HeaderConfig = serde_json::from_str(
            r#"{
                "show_header": true,
                "show_font_name": true,
                "show_datetime": false,
                "show_page_numbers": false
            }"#,
        )
        .unwrap();

        assert!(config.show_font_version);
    }
}
