//! Font Inspector — read-only font introspection queries.
//!
//! All queries go through the [`FontRegistry`] to access font data.

use font_registry::{FontId, FontRegistry};
use skrifa::prelude::*;
use skrifa::{MetadataProvider, Tag};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum InspectorError {
    #[error("font error: {0}")]
    Font(#[from] font_registry::FontError),
    #[error("table not found: {0}")]
    TableNotFound(String),
}

/// Information about a single glyph.
#[derive(Debug, Clone)]
pub struct GlyphInfo {
    pub glyph_id: u32,
    pub codepoint: Option<char>,
    pub name: Option<String>,
}

/// Global font metrics.
#[derive(Debug, Clone, Copy)]
pub struct GlobalMetrics {
    pub units_per_em: u16,
    pub ascender: f32,
    pub descender: f32,
    pub line_gap: f32,
    pub cap_height: Option<f32>,
    pub x_height: Option<f32>,
}

/// Per-glyph metrics.
#[derive(Debug, Clone, Copy)]
pub struct GlyphMetrics {
    pub advance_width: f32,
    pub lsb: f32,
    pub bbox: Option<GlyphBBox>,
}

/// Glyph bounding box.
#[derive(Debug, Clone, Copy)]
pub struct GlyphBBox {
    pub x_min: f32,
    pub y_min: f32,
    pub x_max: f32,
    pub y_max: f32,
}

/// Information about an OpenType feature.
#[derive(Debug, Clone)]
pub struct FeatureInfo {
    pub tag: Tag,
    pub name: String,
}

/// Information about a variation axis.
#[derive(Debug, Clone)]
pub struct AxisInfo {
    pub tag: Tag,
    pub name: String,
    pub min: f32,
    pub default: f32,
    pub max: f32,
}

/// Information about a named instance.
#[derive(Debug, Clone)]
pub struct NamedInstanceInfo {
    pub name: String,
    pub coords: Vec<(Tag, f32)>,
}

/// Read-only font inspector. All queries go through the registry.
pub struct FontInspector<'a> {
    registry: &'a FontRegistry,
}

impl<'a> FontInspector<'a> {
    pub fn new(registry: &'a FontRegistry) -> Self {
        Self { registry }
    }

    /// Enumerate all glyphs in the font via the charmap.
    pub fn enumerate_glyphs(&self, font_id: FontId) -> Result<Vec<GlyphInfo>, InspectorError> {
        use skrifa::raw::TableProvider;

        let font = self.registry.font_ref(font_id)?;
        let charmap = font.charmap();
        let post = font.post().ok();
        let mut glyphs = Vec::new();

        for (codepoint, glyph_id) in charmap.mappings() {
            let gid = glyph_id.to_u32();
            let codepoint = char::from_u32(codepoint);
            glyphs.push(GlyphInfo {
                glyph_id: gid,
                codepoint,
                name: resolve_glyph_name(post.as_ref(), gid, codepoint),
            });
        }

        Ok(glyphs)
    }

    /// Enumerate ALL glyphs in the font (0..glyph_count), including
    /// unencoded glyphs like alternates, components, and .notdef.
    /// Populates codepoint via reverse cmap and name from post table or codepoint.
    pub fn enumerate_all_glyphs(&self, font_id: FontId) -> Result<Vec<GlyphInfo>, InspectorError> {
        use skrifa::raw::TableProvider;
        use std::collections::HashMap;

        let font = self.registry.font_ref(font_id)?;
        let glyph_metrics = font.glyph_metrics(Size::unscaled(), LocationRef::default());
        let glyph_count = glyph_metrics.glyph_count();

        // Build reverse cmap: glyph_id → first codepoint
        let charmap = font.charmap();
        let mut reverse_cmap: HashMap<u32, char> = HashMap::new();
        for (codepoint, glyph_id) in charmap.mappings() {
            let gid = glyph_id.to_u32();
            reverse_cmap
                .entry(gid)
                .or_insert_with(|| char::from_u32(codepoint).unwrap_or('\u{FFFD}'));
        }

        // Try to read post table for glyph names
        let post = font.post().ok();

        let mut glyphs = Vec::with_capacity(glyph_count as usize);
        for gid in 0..glyph_count {
            let codepoint = reverse_cmap.get(&gid).copied();
            glyphs.push(GlyphInfo {
                glyph_id: gid,
                codepoint,
                name: resolve_glyph_name(post.as_ref(), gid, codepoint),
            });
        }

        Ok(glyphs)
    }

    /// Get global font metrics.
    pub fn get_metrics(&self, font_id: FontId) -> Result<GlobalMetrics, InspectorError> {
        let font = self.registry.font_ref(font_id)?;
        let metrics = font.metrics(Size::unscaled(), LocationRef::default());

        Ok(GlobalMetrics {
            units_per_em: metrics.units_per_em,
            ascender: metrics.ascent,
            descender: metrics.descent,
            line_gap: metrics.leading,
            cap_height: metrics.cap_height,
            x_height: metrics.x_height,
        })
    }

    /// Get per-glyph metrics.
    pub fn get_glyph_metrics(
        &self,
        font_id: FontId,
        glyph_id: u32,
    ) -> Result<GlyphMetrics, InspectorError> {
        let font = self.registry.font_ref(font_id)?;
        let glyph_metrics = font.glyph_metrics(Size::unscaled(), LocationRef::default());
        let gid = skrifa::GlyphId::new(glyph_id);

        let advance_width = glyph_metrics.advance_width(gid).unwrap_or(0.0);
        let lsb = glyph_metrics.left_side_bearing(gid).unwrap_or(0.0);
        let bbox = glyph_metrics.bounds(gid).map(|b| GlyphBBox {
            x_min: b.x_min,
            y_min: b.y_min,
            x_max: b.x_max,
            y_max: b.y_max,
        });

        Ok(GlyphMetrics {
            advance_width,
            lsb,
            bbox,
        })
    }

    /// Enumerate variation axes.
    pub fn enumerate_axes(&self, font_id: FontId) -> Result<Vec<AxisInfo>, InspectorError> {
        let font = self.registry.font_ref(font_id)?;
        let axes_collection = font.axes();
        let mut axes = Vec::new();

        for axis in axes_collection.iter() {
            let tag = axis.tag();
            let name = font
                .localized_strings(axis.name_id())
                .into_iter()
                .find_map(|s| Some(s.chars().collect::<String>()))
                .unwrap_or_else(|| format!("{}", tag));

            axes.push(AxisInfo {
                tag,
                name,
                min: axis.min_value(),
                default: axis.default_value(),
                max: axis.max_value(),
            });
        }

        Ok(axes)
    }

    /// Enumerate named instances of a variable font.
    pub fn enumerate_named_instances(
        &self,
        font_id: FontId,
    ) -> Result<Vec<NamedInstanceInfo>, InspectorError> {
        let font = self.registry.font_ref(font_id)?;
        let instances_collection = font.named_instances();
        let axes_collection = font.axes();
        let mut instances = Vec::new();

        for instance in instances_collection.iter() {
            let name = font
                .localized_strings(instance.subfamily_name_id())
                .into_iter()
                .find_map(|s| Some(s.chars().collect::<String>()))
                .unwrap_or_else(|| "Unnamed".to_string());

            let coords: Vec<(Tag, f32)> = axes_collection
                .iter()
                .zip(instance.user_coords())
                .map(|(axis, coord)| (axis.tag(), coord))
                .collect();

            instances.push(NamedInstanceInfo { name, coords });
        }

        Ok(instances)
    }

    /// Check if a specific codepoint is supported by the font.
    pub fn has_codepoint(&self, font_id: FontId, ch: char) -> Result<bool, InspectorError> {
        let font = self.registry.font_ref(font_id)?;
        let charmap = font.charmap();
        Ok(charmap.map(ch).is_some())
    }

    /// Get the set of supported codepoints.
    pub fn supported_codepoints(&self, font_id: FontId) -> Result<Vec<char>, InspectorError> {
        let font = self.registry.font_ref(font_id)?;
        let charmap = font.charmap();
        let mut codepoints = Vec::new();
        for (cp, _gid) in charmap.mappings() {
            if let Some(ch) = char::from_u32(cp) {
                codepoints.push(ch);
            }
        }
        Ok(codepoints)
    }

    /// Enumerate OpenType features (GSUB + GPOS).
    pub fn enumerate_features(&self, font_id: FontId) -> Result<Vec<FeatureInfo>, InspectorError> {
        use skrifa::raw::TableProvider;
        let font = self.registry.font_ref(font_id)?;
        let mut features = Vec::new();
        let mut seen_tags = std::collections::HashSet::new();

        // GSUB features
        if let Ok(gsub) = font.gsub() {
            if let Ok(fl) = gsub.feature_list() {
                for rec in fl.feature_records() {
                    let tag = rec.feature_tag();
                    if seen_tags.insert(tag) {
                        features.push(FeatureInfo {
                            tag,
                            name: feature_tag_name(tag),
                        });
                    }
                }
            }
        }

        // GPOS features
        if let Ok(gpos) = font.gpos() {
            if let Ok(fl) = gpos.feature_list() {
                for rec in fl.feature_records() {
                    let tag = rec.feature_tag();
                    if seen_tags.insert(tag) {
                        features.push(FeatureInfo {
                            tag,
                            name: feature_tag_name(tag),
                        });
                    }
                }
            }
        }

        Ok(features)
    }
}

/// Resolve a glyph name from (in priority order) the post table,
/// a codepoint-derived label, or `None` if neither is available.
fn resolve_glyph_name(
    post: Option<&skrifa::raw::tables::post::Post<'_>>,
    gid: u32,
    codepoint: Option<char>,
) -> Option<String> {
    post.and_then(|p| {
        p.glyph_name(skrifa::GlyphId16::new(gid as u16))
            .map(|s| s.to_string())
    })
    .or_else(|| {
        codepoint.map(|c| {
            if c.is_ascii_graphic() {
                c.to_string()
            } else {
                format!("U+{:04X}", c as u32)
            }
        })
    })
}

/// Map a feature tag to a human-readable name.
fn feature_tag_name(tag: Tag) -> String {
    match &tag.to_be_bytes() {
        b"kern" => "Kerning".to_string(),
        b"liga" => "Standard Ligatures".to_string(),
        b"dlig" => "Discretionary Ligatures".to_string(),
        b"hlig" => "Historical Ligatures".to_string(),
        b"calt" => "Contextual Alternates".to_string(),
        b"salt" => "Stylistic Alternates".to_string(),
        b"smcp" => "Small Capitals".to_string(),
        b"c2sc" => "Caps to Small Caps".to_string(),
        b"onum" => "Oldstyle Figures".to_string(),
        b"lnum" => "Lining Figures".to_string(),
        b"tnum" => "Tabular Figures".to_string(),
        b"pnum" => "Proportional Figures".to_string(),
        b"frac" => "Fractions".to_string(),
        b"ordn" => "Ordinals".to_string(),
        b"sups" => "Superscript".to_string(),
        b"subs" => "Subscript".to_string(),
        b"swsh" => "Swash".to_string(),
        b"ss01" => "Stylistic Set 1".to_string(),
        b"ss02" => "Stylistic Set 2".to_string(),
        b"ss03" => "Stylistic Set 3".to_string(),
        _ => format!("{tag}"),
    }
}
