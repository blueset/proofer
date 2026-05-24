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
                let glyphs = self.inspector.enumerate_all_glyphs(font_id)?;
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

            ContentSpec::Pattern {
                glyphs,
                templates,
                between,
                wrap,
                placeholder,
                separator,
            } => {
                let text = expand_pattern(
                    glyphs,
                    templates,
                    between.as_ref(),
                    wrap,
                    placeholder,
                    separator,
                    font_id,
                    &self.inspector,
                )?;
                Ok(ResolvedContent::Text(text))
            }

            ContentSpec::Concat { parts, joiner } => {
                let mut texts = Vec::new();
                for part in parts {
                    match self.resolve(part, font_id)? {
                        ResolvedContent::Text(t) => texts.push(t),
                        ResolvedContent::Glyphs(glyphs) => {
                            // Convert glyphs to text for concatenation
                            let t: String = glyphs.iter().filter_map(|g| g.codepoint).collect();
                            texts.push(t);
                        }
                    }
                }
                Ok(ResolvedContent::Text(texts.join(joiner)))
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
                if !ranges
                    .iter()
                    .any(|(start, end)| cp_u32 >= *start && cp_u32 <= *end)
                {
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

/// Resolve a GlyphSet into a list of characters.
fn resolve_glyph_set(
    set: &proof_model::GlyphSet,
    font_id: FontId,
    inspector: &FontInspector<'_>,
) -> Result<Vec<char>, ResolverError> {
    use proof_model::{GlyphPreset, GlyphSet};
    match set {
        GlyphSet::Literal(s) => Ok(s.chars().collect()),
        GlyphSet::List(_) => {
            // List variant is for context strings, not individual chars
            // Return each string's first char as fallback
            Ok(vec![])
        }
        GlyphSet::Preset { preset } => {
            let glyphs = inspector.enumerate_glyphs(font_id)?;
            let chars: Vec<char> = glyphs
                .iter()
                .filter_map(|g| g.codepoint)
                .filter(|c| match preset {
                    GlyphPreset::Uppercase => c.is_uppercase(),
                    GlyphPreset::Lowercase => c.is_lowercase(),
                    GlyphPreset::Digits => c.is_ascii_digit(),
                    GlyphPreset::All => true,
                })
                .collect();
            Ok(chars)
        }
    }
}

/// Resolve a GlyphSet into a list of context strings (for `between` mode).
fn resolve_context_strings(
    set: &proof_model::GlyphSet,
    font_id: FontId,
    inspector: &FontInspector<'_>,
) -> Result<Vec<String>, ResolverError> {
    use proof_model::GlyphSet;
    match set {
        GlyphSet::Literal(s) => {
            // Each character becomes its own context string
            Ok(s.chars().map(|c| c.to_string()).collect())
        }
        GlyphSet::List(strings) => Ok(strings.clone()),
        GlyphSet::Preset { .. } => {
            let chars = resolve_glyph_set(set, font_id, inspector)?;
            Ok(chars.into_iter().map(|c| c.to_string()).collect())
        }
    }
}

/// Expand a Pattern content spec into a text string.
fn expand_pattern(
    glyphs: &proof_model::GlyphSet,
    templates: &[String],
    between: Option<&proof_model::GlyphSet>,
    wrap: &[(String, String)],
    placeholder: &str,
    separator: &proof_model::PatternSeparator,
    font_id: FontId,
    inspector: &FontInspector<'_>,
) -> Result<String, ResolverError> {
    let test_chars = resolve_glyph_set(glyphs, font_id, inspector)?;
    if test_chars.is_empty() {
        return Err(ResolverError::NoGlyphsMatched);
    }

    let sep = match separator {
        proof_model::PatternSeparator::Newline => "\n",
        proof_model::PatternSeparator::Space => " ",
        proof_model::PatternSeparator::None => "",
    };

    let mut result = String::new();

    if !templates.is_empty() {
        // Templates mode: for each test glyph, replace placeholder in each template
        for ch in &test_chars {
            let ch_str = ch.to_string();
            for (i, tmpl) in templates.iter().enumerate() {
                if i > 0 {
                    result.push_str(sep);
                }
                result.push_str(&tmpl.replace(placeholder, &ch_str));
            }
            result.push_str(sep);
        }
    } else if let Some(between_set) = between {
        // Between mode: for each context string, insert test glyphs between repetitions
        let contexts = resolve_context_strings(between_set, font_id, inspector)?;
        for ctx in &contexts {
            result.push_str(ctx);
            for ch in &test_chars {
                result.push(*ch);
                result.push_str(ctx);
            }
            result.push_str(sep);
        }
    } else if !wrap.is_empty() {
        // Wrap mode: for each test glyph, wrap with all before/after pairs
        for ch in &test_chars {
            for (before, after) in wrap {
                result.push_str(before);
                result.push(*ch);
                result.push_str(after);
            }
            result.push_str(sep);
        }
    }

    // Strip trailing separator
    if !sep.is_empty() {
        while result.ends_with(sep) {
            result.truncate(result.len() - sep.len());
        }
    }

    Ok(result)
}
