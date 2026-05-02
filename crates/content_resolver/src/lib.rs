//! Content Resolver — expands abstract content specs into concrete text/glyph lists.
//!
//! Transforms [`ContentSpec`](proof_model::ContentSpec) into actual text strings
//! or glyph ID lists that the layout engine can process.

use font_inspector::{FontInspector, GlyphInfo, InspectorError};
use font_registry::FontId;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ResolverError {
    #[error("inspector error: {0}")]
    Inspector(#[from] InspectorError),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("no glyphs matched the filter")]
    NoGlyphsMatched,
}

/// The result of content resolution.
#[derive(Debug, Clone)]
pub enum ResolvedContent {
    /// A text string to be shaped and rendered.
    Text(String),
    /// A list of individual glyphs (for glyph grids).
    Glyphs(Vec<GlyphInfo>),
}

/// Resolve content for a given font.
pub struct ContentResolver<'a> {
    inspector: FontInspector<'a>,
}

impl<'a> ContentResolver<'a> {
    pub fn new(inspector: FontInspector<'a>) -> Self {
        Self { inspector }
    }

    /// Resolve a content spec to concrete content.
    pub fn resolve(
        &self,
        spec: &proof_model::ContentSpec,
        font_id: FontId,
    ) -> Result<ResolvedContent, ResolverError> {
        use proof_model::ContentSpec;

        match spec {
            ContentSpec::Text { text } => Ok(ResolvedContent::Text(text.clone())),

            ContentSpec::FileRef { path } => {
                let text = std::fs::read_to_string(path)?;
                Ok(ResolvedContent::Text(text))
            }

            ContentSpec::AllGlyphs => {
                let glyphs = self.inspector.enumerate_glyphs(font_id)?;
                if glyphs.is_empty() {
                    return Err(ResolverError::NoGlyphsMatched);
                }
                Ok(ResolvedContent::Glyphs(glyphs))
            }

            ContentSpec::GlyphFilter {
                scripts,
                blocks: _,
                categories,
                ranges,
            } => {
                let all_glyphs = self.inspector.enumerate_glyphs(font_id)?;
                let filtered = filter_glyphs(&all_glyphs, scripts, categories, ranges);
                if filtered.is_empty() {
                    return Err(ResolverError::NoGlyphsMatched);
                }
                Ok(ResolvedContent::Glyphs(filtered))
            }

            ContentSpec::SpacingStrings { pattern } => {
                let text = generate_spacing_strings(pattern, font_id, &self.inspector)?;
                Ok(ResolvedContent::Text(text))
            }

            ContentSpec::Custom { text } => Ok(ResolvedContent::Text(text.clone())),
        }
    }

    /// Remove characters from text that are missing in the font.
    pub fn remove_missing_glyphs(
        &self,
        text: &str,
        font_id: FontId,
    ) -> Result<String, ResolverError> {
        let mut result = String::with_capacity(text.len());
        for ch in text.chars() {
            if ch.is_whitespace() || self.inspector.has_codepoint(font_id, ch)? {
                result.push(ch);
            }
        }
        Ok(result)
    }
}

/// Filter glyphs by Unicode script, category, and codepoint ranges.
fn filter_glyphs(
    glyphs: &[GlyphInfo],
    scripts: &[String],
    categories: &[String],
    ranges: &[(u32, u32)],
) -> Vec<GlyphInfo> {
    glyphs
        .iter()
        .filter(|g| {
            let Some(cp) = g.codepoint else {
                return false;
            };

            // Script filter
            if !scripts.is_empty() {
                use unicode_script::UnicodeScript;
                let script = cp.script();
                let script_name = format!("{script:?}");
                if !scripts.iter().any(|s| s.eq_ignore_ascii_case(&script_name)) {
                    return false;
                }
            }

            // Category filter (general category)
            if !categories.is_empty() {
                let cat = unicode_general_category(cp);
                if !categories.iter().any(|c| c == &cat) {
                    return false;
                }
            }

            // Range filter
            if !ranges.is_empty() {
                let cp_u32 = cp as u32;
                if !ranges.iter().any(|(start, end)| cp_u32 >= *start && cp_u32 <= *end) {
                    return false;
                }
            }

            true
        })
        .cloned()
        .collect()
}

/// Simple Unicode general category detection.
fn unicode_general_category(ch: char) -> String {
    if ch.is_uppercase() {
        "Lu".to_string()
    } else if ch.is_lowercase() {
        "Ll".to_string()
    } else if ch.is_numeric() {
        "Nd".to_string()
    } else if ch.is_alphabetic() {
        "L".to_string()
    } else if ch.is_whitespace() {
        "Zs".to_string()
    } else {
        "So".to_string()
    }
}

/// Generate spacing strings from a pattern template.
fn generate_spacing_strings(
    pattern: &str,
    font_id: FontId,
    inspector: &FontInspector<'_>,
) -> Result<String, ResolverError> {
    // Pattern like "HnH" means: for each lowercase letter X, produce "HXH\n"
    // We look for a placeholder char (lowercase in pattern means "substitute here")
    let glyphs = inspector.enumerate_glyphs(font_id)?;
    let lowercase_glyphs: Vec<_> = glyphs
        .iter()
        .filter(|g| g.codepoint.is_some_and(|c| c.is_lowercase()))
        .collect();

    let mut result = String::new();
    for glyph in &lowercase_glyphs {
        if let Some(cp) = glyph.codepoint {
            let line: String = pattern
                .chars()
                .map(|c| {
                    if c.is_lowercase() {
                        cp
                    } else {
                        c
                    }
                })
                .collect();
            result.push_str(&line);
            result.push('\n');
        }
    }

    Ok(result)
}
