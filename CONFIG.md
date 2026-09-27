# Proof configuration guide

Proofer reads a YAML document (usually named `*.proof.yaml`) and writes a PDF. Run `cargo run --locked -p proofer-cli -- example` for a generated starter file or `cargo run --locked -p proofer-cli -- schema` for the machine-readable JSON Schema. See [README.md](README.md) for build instructions.

## Quick start

Save this as `sample.proof.yaml` in the repository root. The example font is included with the project.

```yaml
version: "1.0"
page_settings:
  preset: letter
  orientation: landscape
fonts:
  - path: assets/fonts/InterVariable.ttf
sections:
  - name: Sample text
    layout:
      type: Simple
    content:
      type: Text
      text: The quick brown fox jumps over the lazy dog.
    font_indices: [0]
    design_attrs:
      font_size: 24
      variations:
        wght: 600
  - name: Waterfall
    layout:
      type: Waterfall
      sizes: [12, 18, 24, 36, 48]
    content:
      type: Text
      text: Hamburgefonstiv
    font_indices: [0]
```

Generate the PDF with:

```sh
cargo run --locked -p proofer-cli -- generate sample.proof.yaml
```

The default output is `sample.proof.pdf`; use `--output proof.pdf` to change it. Relative font paths, top-level `FileRef` paths, and `ImagePdf.source` paths are resolved against the directory containing the YAML file, not the shell's working directory.

## Document structure

| Key | Required | Meaning |
| --- | --- | --- |
| `version` | No | Schema version; defaults to `"1.0"`. |
| `page_settings` | No | Page preset or custom size; defaults to landscape letter. |
| `fonts` | Yes | List of fonts; at least one is required. |
| `sections` | Yes | Ordered list of proof sections. Each section starts on a new page. |

### Page settings

For a preset:

```yaml
page_settings:
  preset: a4
  orientation: portrait
```

`preset` accepts `letter`, `a4`, `a3`, or `tabloid`; `orientation` accepts `portrait` or `landscape` and defaults to `landscape`. Preset margins are 36 points on each side. Preset dimensions in points are letter 612 x 792, A4 595.28 x 841.89, A3 841.89 x 1190.55, and tabloid 792 x 1224 (portrait; landscape swaps the edges).

For a custom page, supply positive `width` and `height` in points instead of `preset`:

```yaml
page_settings:
  width: 600
  height: 800
  margin_top: 36
  margin_bottom: 36
  margin_left: 36
  margin_right: 36
```

Custom margins default to 36 points each and must be nonnegative. The usable body is the page size minus its margins; leave enough room for text and headers.

### Fonts

```yaml
fonts:
  - path: assets/fonts/InterVariable.ttf
    face_index: 0
  - path: path/to/another-font.otf
```

`path` is required. `face_index` selects a face in a font collection (TTC/OTC) and defaults to `0`. `checksum` is an optional string accepted by the format but is not currently checked. Every font is loaded when generating a document. Section font indices are zero-based: `[0]` selects the first font.

### Sections

Each section requires `layout`, `content`, and a nonempty `font_indices` list of valid indices into `fonts`. `name` is optional and appears in the section header. `design_attrs` and `header_config` are optional; defaults are listed below. For multi-style layouts, `styles` optionally defines variants; otherwise one variant is created per `font_indices` entry using the base attributes.

```yaml
sections:
  - name: Body copy
    font_indices: [0]
    layout: { type: Simple }
    content:
      type: Text
      text: "A sample paragraph."
    design_attrs:
      font_size: 18
    header_config:
      show_datetime: false
```

## Layouts

Choose a layout with the case-sensitive `layout.type` value. The snippets below replace only the `layout` field of a section.

| Type | Fields | Behavior |
| --- | --- | --- |
| `Simple` | None | Flows text at `design_attrs.font_size` across pages. |
| `Waterfall` | `sizes` (required list of point sizes), `spacing` (default `8` points), `label` (optional) | Repeats the same text at each size, with size labels and optional appended label. |
| `Columns` | `count` (required column count), `gutter` (required points), `show_headers` (default `false`), `column_label` (optional) | Flows text left to right through columns and onto new pages; `column_label` is repeated above filled columns. |
| `GlyphGrid` | `mode` (required: `Grid` or `Compact`), `show_metrics` (default `true`), `show_names` (default `true`), `cell_padding` (default `4` points), `subgrid_x` / `subgrid_y` (optional) | Displays glyph cells with optional metric guides and names/codepoints. `Compact` packs cells more tightly. |
| `StyleComparison` | `arrangement` (required: `Columns` or `Rows`), `overflow` (required: `Truncate` or `Flow`), `spacing` (default `16` points), `max_columns` (optional, for `Columns`) | Shows the same text in several styles side by side or stacked. `Truncate` clips text to available space; `Flow` continues on later pages. |
| `Interleave` | `mode` (required: `Proofs`, `Sections`, or `Pages`) | Generates a text proof per style, then orders the pages by style or by page number. Currently `Sections` behaves like `Proofs` for a section. |
| `ImagePdf` | `source` (required path) | Accepted by the schema but **not rendered** yet; do not use it for production proofs. |

Examples:

```yaml
layout:
  type: Columns
  count: 2
  gutter: 18
  column_label: Draft
```

```yaml
layout:
  type: GlyphGrid
  mode: Grid
  show_metrics: true
  show_names: true
  subgrid_x:
    axis: wght
    values: [300, 600, 900]
  subgrid_y:
    axis: wdth
    values: [75, 100]
```

Subgrids vary each glyph along the X (columns) and/or Y (rows) axis. Each `axis` tag must contain exactly four characters, and `values` must not be empty. The X and Y tags must differ. Subgrid coordinates override matching `design_attrs.variations` for each cell; unspecified axes retain the base values.

```yaml
layout:
  type: StyleComparison
  arrangement: Columns
  overflow: Flow
  spacing: 16
  max_columns: 2
```

For `Columns`, `max_columns` splits additional variants onto new pages. For `Rows`, each style occupies a row. `Interleave` modes `Proofs` and `Sections` place all pages of one style before the next; `Pages` alternates page 1 of each style, then page 2, and so on.

**Implementation notes:** `Columns.show_headers` is accepted but currently ignored; use `column_label` for visible labels. `ImagePdf` currently produces no image/PDF content. Avoid relying on either as a working rendering feature.

## Content

Set the case-sensitive `content.type` to one of the following:

| Type | Fields | Result |
| --- | --- | --- |
| `Text` | `text` (required string) | Literal text, including YAML multiline strings. |
| `Custom` | `text` (required string) | Literal user-provided text; handled the same way as `Text`. |
| `FileRef` | `path` (required) | Text read from a UTF-8 file. |
| `AllGlyphs` | None | Every glyph in the primary font, including unencoded glyphs, for a glyph grid. |
| `GlyphFilter` | `scripts`, `blocks`, `categories`, `ranges` (all optional lists) | Encoded glyphs matching the specified filters. |
| `Pattern` | `glyphs` (required), plus `templates`, `between`, or `wrap`; optional `placeholder`, `separator` | Generated text for repeated glyph tests. |
| `Concat` | `parts` (required list of content specs), `joiner` (default newline) | Concatenates the resolved parts as text. |

Content is resolved once against the section's first font index. With `GlyphGrid`, text content selects glyphs by the characters present in that text; order follows the font's character map, not the text. With text layouts, glyph-list content is converted to text using only glyphs that have Unicode codepoints, so `AllGlyphs` is most useful with `GlyphGrid`.

### File references and filters

```yaml
content:
  type: FileRef
  path: samples/paragraph.txt
```

```yaml
content:
  type: GlyphFilter
  scripts: [Latin]
  categories: [Lu, Ll]
  ranges: [[65, 90], [97, 122]]
```

Within a filter, entries in the same list are alternatives; nonempty filter lists are combined with AND. Ranges are inclusive Unicode scalar-value pairs expressed as integers. `scripts` matches Unicode script names case-insensitively. `categories` currently supports the implementation's simplified labels `Lu`, `Ll`, `Nd`, `L`, `Zs`, and `So` (case-sensitive), not the full Unicode general-category taxonomy. `blocks` is accepted but currently ignored. If no glyphs match, generation fails.

### Patterns

`glyphs` may be a literal string such as `"abc"` or a preset such as `{ preset: lowercase }`. Presets are `uppercase`, `lowercase`, `digits`, and `all`, selected from encoded characters in the primary font; `digits` selects ASCII digits. A list of strings is supported for `between` contexts, not for the test `glyphs` (a list in `glyphs` yields no matches).

Choose one generation method:

```yaml
content:
  type: Pattern
  glyphs: "ab"
  templates: ["H?H", "o?o"]
  placeholder: "?"
  separator: newline
```

The result contains one substituted template per test character and template. `placeholder` defaults to `?`; `separator` accepts `newline` (default), `space`, or `none`.

```yaml
content:
  type: Pattern
  glyphs: "ab"
  between: ["HH", "oo"]
```

For each context, `between` places every test glyph between repetitions of that context (for example, `HHaHHbHH`). A literal string or preset in `between` treats each character as its own context.

```yaml
content:
  type: Pattern
  glyphs: "ab"
  wrap: [["(", ")"], ["[", "]"]]
```

`wrap` applies every before/after pair to each test glyph, without adding a separator between pairs. Supply exactly one of `templates`, `between`, or `wrap`; the renderer uses the first nonempty method in that order, and with none it generates empty text.

### Concatenation

```yaml
content:
  type: Concat
  joiner: "\n\n"
  parts:
    - type: Text
      text: First paragraph.
    - type: Pattern
      glyphs: "ab"
      templates: ["?a", "?b"]
```

`parts` can contain other content specs, including nested `Concat` values. A glyph-list part is converted to its encoded characters when concatenated. **Path caveat:** relative `FileRef.path` values nested inside `Concat` are not currently rebased to the YAML file's directory; use absolute paths for those nested references.

## Typography and style variants

Use `design_attrs` on a section to set its base typography:

```yaml
design_attrs:
  font_size: 24
  line_height: 1.3
  tracking: 0.02
  kerning: true
  features:
    liga: 1
    smcp: 0
  language: en
  variations:
    wght: 700
  text_align: left
  line_limit: 20
  y_offset: 12
```

| Key | Default | Meaning |
| --- | --- | --- |
| `font_size` | `12` | Point size; must be positive. |
| `line_height` | Not set | Relative line-height multiplier for text (text shaping uses `1.2` when unset). |
| `tracking` | `0` | Additional letter spacing in em units. |
| `kerning` | `true` | Enables kerning for text. |
| `features` | `{}` | OpenType feature tag to unsigned integer value. |
| `language` | Not set | Optional language tag passed to text shaping. |
| `variations` | `{}` | Variable-font axis tag to numeric coordinate. |
| `text_align` | `left` | `left`, `center`, `right`, or `justified`. |
| `line_limit` | Not set | Maximum number of shaped text lines. |
| `y_offset` | `0` | Extra top offset in points for `Simple` layout pages only; positive values move text down. |

Feature and variation tags must each have exactly four characters. Use tags and coordinates supported by the chosen font. Glyph-grid `font_size` controls glyph cell sizing; waterfall `sizes` supplies the point sizes instead of the base `font_size`.

For `StyleComparison` and `Interleave`, define `styles` to override the base attributes or choose another font:

```yaml
font_indices: [0]
design_attrs:
  font_size: 18
styles:
  - label: Regular
    font_index: 0
  - label: Bold
    font_index: 0
    design_attrs:
      variations: { wght: 700 }
      font_size: 24
```

`label` and `font_index` are optional. A missing `font_index` uses the first section font. Each `design_attrs` key listed above can be overridden; unspecified values inherit the base. `features` and `variations` maps, if specified in a variant, replace the entire base map rather than merging. Without `styles`, one variant is made for each entry in `font_indices`. Other layouts use only the first font and base attributes. Choose valid font indices in styles too; the current validator does not check them separately.

## Headers

```yaml
header_config:
  show_header: true
  show_font_name: true
  show_font_version: true
  show_datetime: false
  show_page_numbers: true
```

All header switches default to `true`. The section name, font name (with version when available), local date/time, page number, and dividing line are placed in the header. `show_font_name`, `show_font_version`, `show_datetime`, and `show_page_numbers` control their respective elements. Currently `show_header: false` suppresses headers on subsequent pages, but the first page of a section still receives one; avoid relying on this option to produce entirely headerless output.
