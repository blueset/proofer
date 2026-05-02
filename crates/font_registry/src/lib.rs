//! Font Registry — single source of truth for all font data.
//!
//! Every other module references fonts by [`FontId`]. The registry owns
//! the raw font bytes and provides access to skrifa `FontRef` instances.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use skrifa::prelude::*;
use skrifa::raw::FileRef;
use skrifa::{FontRef, MetadataProvider};
use thiserror::Error;

/// Re-export FontId from layout_ir so all crates use the same type.
pub use layout_ir::FontId;

#[derive(Error, Debug)]
pub enum FontError {
    #[error("failed to read font file: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to parse font data: {0}")]
    Parse(#[from] skrifa::raw::ReadError),
    #[error("font not found: {0:?}")]
    NotFound(FontId),
    #[error("invalid face index {index} for font with {count} faces")]
    InvalidFaceIndex { index: u32, count: u32 },
}

/// Metadata extracted from a font for display in proof headers.
#[derive(Debug, Clone)]
pub struct FontMetadata {
    pub family: String,
    pub style: String,
    pub version: Option<String>,
    pub designer: Option<String>,
    pub is_variable: bool,
    pub face_count: u32,
    pub source_path: Option<PathBuf>,
}

/// Information about a font axis (for variable fonts).
#[derive(Debug, Clone)]
pub struct AxisInfo {
    pub tag: [u8; 4],
    pub name: String,
    pub min: f32,
    pub default: f32,
    pub max: f32,
    pub is_hidden: bool,
}

/// A named instance of a variable font.
#[derive(Debug, Clone)]
pub struct NamedInstance {
    pub name: String,
    pub coords: Vec<(String, f32)>,
}

/// An entry in the registry for one loaded font face.
struct FontEntry {
    data: Arc<Vec<u8>>,
    face_index: u32,
    metadata: FontMetadata,
}

/// The font registry. Owns all loaded font data.
pub struct FontRegistry {
    fonts: HashMap<FontId, FontEntry>,
    next_id: u32,
}

impl FontRegistry {
    pub fn new() -> Self {
        Self {
            fonts: HashMap::new(),
            next_id: 0,
        }
    }

    /// Load a font from a file path. Returns the FontId for the first face,
    /// or a specific face if `face_index` is provided.
    pub fn load_file(&mut self, path: &Path, face_index: u32) -> Result<FontId, FontError> {
        let data = std::fs::read(path)?;
        self.load_bytes(Arc::new(data), face_index, Some(path.to_path_buf()))
    }

    /// Load a font from raw bytes.
    pub fn load_bytes(
        &mut self,
        data: Arc<Vec<u8>>,
        face_index: u32,
        source_path: Option<PathBuf>,
    ) -> Result<FontId, FontError> {
        // Validate the font data
        let face_count = match FileRef::new(&data)? {
            FileRef::Font(_) => {
                if face_index != 0 {
                    return Err(FontError::InvalidFaceIndex {
                        index: face_index,
                        count: 1,
                    });
                }
                1
            }
            FileRef::Collection(collection) => {
                let count = collection.len();
                if face_index >= count {
                    return Err(FontError::InvalidFaceIndex {
                        index: face_index,
                        count,
                    });
                }
                count
            }
        };

        let font_ref = FontRef::from_index(&data, face_index)?;
        let metadata = extract_metadata(&font_ref, face_count, source_path);

        let id = FontId(self.next_id);
        self.next_id += 1;

        self.fonts.insert(
            id,
            FontEntry {
                data,
                face_index,
                metadata,
            },
        );

        Ok(id)
    }

    /// Load all faces from a font file (for TTC/OTC collections).
    pub fn load_all_faces(&mut self, path: &Path) -> Result<Vec<FontId>, FontError> {
        let data = Arc::new(std::fs::read(path)?);
        let count = match FileRef::new(&data)? {
            FileRef::Font(_) => 1,
            FileRef::Collection(c) => c.len(),
        };
        let mut ids = Vec::with_capacity(count as usize);
        for i in 0..count {
            ids.push(self.load_bytes(Arc::clone(&data), i, Some(path.to_path_buf()))?);
        }
        Ok(ids)
    }

    /// Get a skrifa FontRef for the given font ID.
    pub fn font_ref(&self, id: FontId) -> Result<FontRef<'_>, FontError> {
        let entry = self.fonts.get(&id).ok_or(FontError::NotFound(id))?;
        Ok(FontRef::from_index(&entry.data, entry.face_index)?)
    }

    /// Get the raw font bytes and face index (for passing to krilla).
    pub fn font_data(&self, id: FontId) -> Result<(Arc<Vec<u8>>, u32), FontError> {
        let entry = self.fonts.get(&id).ok_or(FontError::NotFound(id))?;
        Ok((Arc::clone(&entry.data), entry.face_index))
    }

    /// Get font metadata.
    pub fn metadata(&self, id: FontId) -> Result<&FontMetadata, FontError> {
        let entry = self.fonts.get(&id).ok_or(FontError::NotFound(id))?;
        Ok(&entry.metadata)
    }

    /// Check if a font ID is valid.
    pub fn contains(&self, id: FontId) -> bool {
        self.fonts.contains_key(&id)
    }

    /// Get all loaded font IDs.
    pub fn font_ids(&self) -> Vec<FontId> {
        self.fonts.keys().copied().collect()
    }
}

impl Default for FontRegistry {
    fn default() -> Self {
        Self::new()
    }
}

fn extract_metadata(
    font: &FontRef<'_>,
    face_count: u32,
    source_path: Option<PathBuf>,
) -> FontMetadata {
    let name_table = font.localized_strings(skrifa::string::StringId::FAMILY_NAME);
    let family = name_table
        .into_iter()
        .find_map(|s| Some(s.chars().collect::<String>()))
        .unwrap_or_else(|| "Unknown".to_string());

    let style_table =
        font.localized_strings(skrifa::string::StringId::SUBFAMILY_NAME);
    let style = style_table
        .into_iter()
        .find_map(|s| Some(s.chars().collect::<String>()))
        .unwrap_or_else(|| "Regular".to_string());

    let version_table =
        font.localized_strings(skrifa::string::StringId::VERSION_STRING);
    let version = version_table
        .into_iter()
        .find_map(|s| Some(s.chars().collect::<String>()));

    let designer_table =
        font.localized_strings(skrifa::string::StringId::DESIGNER);
    let designer = designer_table
        .into_iter()
        .find_map(|s| Some(s.chars().collect::<String>()));

    let is_variable = !font.axes().is_empty();

    FontMetadata {
        family,
        style,
        version,
        designer,
        is_variable,
        face_count,
        source_path,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_creation() {
        let registry = FontRegistry::new();
        assert!(registry.font_ids().is_empty());
    }
}
