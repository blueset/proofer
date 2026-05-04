//! Layout IR — the display list intermediate representation.
//!
//! This is the single source of truth between layout and rendering.
//! Both the PDF renderer and screen preview consume this IR.

use serde::{Deserialize, Serialize};

/// A complete laid-out document ready for rendering.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayoutDocument {
    pub pages: Vec<Page>,
}

impl LayoutDocument {
    pub fn new() -> Self {
        Self { pages: Vec::new() }
    }

    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    pub fn add_page(&mut self, page: Page) {
        self.pages.push(page);
    }
}

impl Default for LayoutDocument {
    fn default() -> Self {
        Self::new()
    }
}

/// A single page in the output document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Page {
    pub width: f32,
    pub height: f32,
    pub commands: Vec<DrawCommand>,
}

impl Page {
    pub fn new(width: f32, height: f32) -> Self {
        Self {
            width,
            height,
            commands: Vec::new(),
        }
    }

    pub fn push(&mut self, cmd: DrawCommand) {
        self.commands.push(cmd);
    }
}

/// A font identifier used across all crates.
/// Opaque handle into the font registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FontId(pub u32);

/// A positioned glyph: (glyph_id, x, y) plus optional offsets.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct PositionedGlyph {
    pub glyph_id: u32,
    pub x: f32,
    pub y: f32,
    /// Per-glyph y offset from baseline (e.g., for mark attachment). Negative = above baseline.
    #[serde(default)]
    pub y_offset: f32,
}

/// Drawing commands that make up a page.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DrawCommand {
    /// A run of glyphs from a single font at a single size.
    GlyphRun {
        font_id: FontId,
        size: f32,
        glyphs: Vec<PositionedGlyph>,
        /// Original text for PDF ActualText (accessibility).
        text: String,
        /// Variation axis settings for variable fonts: (tag, value).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        variations: Vec<(String, f32)>,
    },

    /// A straight line.
    Line {
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
        stroke: StrokeStyle,
    },

    /// A rectangle, optionally filled and/or stroked.
    Rect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        fill: Option<Color>,
        stroke: Option<StrokeStyle>,
    },

    /// A text label rendered with a system font.
    Label {
        text: String,
        x: f32,
        y: f32,
        size: f32,
        color: Color,
    },

    /// Clip children to a rectangular region.
    Clip {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        children: Vec<DrawCommand>,
    },

    /// An embedded image.
    Image {
        data: ImageData,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
    },

    /// A group with an optional transform.
    Group {
        transform: Option<Transform>,
        children: Vec<DrawCommand>,
    },
}

/// Stroke style for lines and rectangles.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct StrokeStyle {
    pub width: f32,
    pub color: Color,
}

impl StrokeStyle {
    pub fn new(width: f32, color: Color) -> Self {
        Self { width, color }
    }

    pub fn hairline(color: Color) -> Self {
        Self {
            width: 0.25,
            color,
        }
    }
}

/// RGBA color.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const BLACK: Self = Self {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    };
    pub const WHITE: Self = Self {
        r: 1.0,
        g: 1.0,
        b: 1.0,
        a: 1.0,
    };
    pub const RED: Self = Self {
        r: 1.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    };
    pub const LIGHT_GRAY: Self = Self {
        r: 0.85,
        g: 0.85,
        b: 0.85,
        a: 1.0,
    };
    pub const BLUE: Self = Self {
        r: 0.2,
        g: 0.4,
        b: 0.8,
        a: 1.0,
    };

    pub fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    pub fn gray(v: f32) -> Self {
        Self {
            r: v,
            g: v,
            b: v,
            a: 1.0,
        }
    }
}

/// Image data for embedded images.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ImageData {
    Png(Vec<u8>),
    Jpeg(Vec<u8>),
}

/// 2D affine transform.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Transform {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
    pub e: f32,
    pub f: f32,
}

impl Transform {
    pub fn identity() -> Self {
        Self {
            a: 1.0,
            b: 0.0,
            c: 0.0,
            d: 1.0,
            e: 0.0,
            f: 0.0,
        }
    }

    pub fn translate(x: f32, y: f32) -> Self {
        Self {
            a: 1.0,
            b: 0.0,
            c: 0.0,
            d: 1.0,
            e: x,
            f: y,
        }
    }

    pub fn scale(sx: f32, sy: f32) -> Self {
        Self {
            a: sx,
            b: 0.0,
            c: 0.0,
            d: sy,
            e: 0.0,
            f: 0.0,
        }
    }
}
