//! On-disk JSON schema (`drv3-translate/v1`) and its conversion into the
//! library's plain Rust model. The library crate is `serde`-free, so this
//! module owns every `Deserialize` impl.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use drv3_translate::{
    AtlasSpec, FileFormat, FontFileGroup, FontGlyphPatch, StxEntryPatch, StxFileGroup,
    TranslationFileGroup, TranslationSet,
};
use serde::Deserialize;

pub const EXPECTED_SCHEMA: &str = "drv3-translate/v1";

/// Top-level JSON document.
#[derive(Debug, Deserialize)]
pub struct TranslationDocJson {
    pub schema: String,
    #[serde(default)]
    pub source_language: String,
    #[serde(default)]
    pub target_language: String,
    // Accepted-and-ignored: kept in the schema for translator audit trails
    // but the patcher has no use for either.
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    pub files: Vec<FileGroupJson>,
}

/// File-group variant. Serde's internally-tagged enum dispatch on the
/// `format` field — every variant gets the format string flattened
/// alongside its own fields in the same object.
#[derive(Debug, Deserialize)]
#[serde(tag = "format", rename_all = "lowercase")]
pub enum FileGroupJson {
    Stx(StxFileGroupJson),
    Font(FontFileGroupJson),
}

#[derive(Debug, Deserialize)]
pub struct StxFileGroupJson {
    pub cpk: String,
    pub cpk_path: String,
    pub spc_member: String,
    pub entries: Vec<EntryJson>,
}

#[derive(Debug, Deserialize)]
pub struct EntryJson {
    pub table: u32,
    pub index: u32,
    pub source: String,
    pub target: String,
    // `context` is intentionally accepted-and-ignored: it carries opaque
    // translator metadata that the patch engine has no use for. Deserialize
    // it via the catch-all so unknown context shapes don't break parsing.
    #[serde(default)]
    pub context: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct FontFileGroupJson {
    pub cpk: String,
    pub cpk_path: String,
    pub spc_member: String,
    #[serde(default)]
    pub font_name: Option<String>,
    /// Target atlas geometry. When present and taller than the game's
    /// shipped `$TXR`, the engine grows the BC4 atlas before blitting.
    #[serde(default)]
    pub atlas: Option<AtlasJson>,
    pub glyphs: Vec<FontGlyphJson>,
}

#[derive(Debug, Deserialize)]
pub struct AtlasJson {
    pub width: u16,
    pub height: u16,
    /// Pixel format of the *existing* atlas being patched: `"BC4"` (the
    /// shipped format) or `"ARGB8888"` (an already-patched atlas). Other
    /// values are rejected at conversion time. Patched atlases are always
    /// re-emitted uncompressed as ARGB8888 regardless of this value.
    pub format: String,
}

use crate::glyph::{KerningJson, PositionJson, SizeJson};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FontGlyphJson {
    pub codepoint: u32,
    /// Optional human-readable mirror of `codepoint`. When both are
    /// present the loader validates they agree.
    #[serde(default)]
    pub char: Option<String>,
    /// Path to the per-glyph bitmap image, relative to the JSON file's
    /// directory. The image's alpha channel is the glyph mask.
    #[serde(default)]
    pub image_path: Option<String>,
    #[serde(default)]
    pub position: Option<PositionJson>,
    #[serde(default)]
    pub size: Option<SizeJson>,
    #[serde(default)]
    pub kerning: Option<KerningJson>,
}

/// Load one JSON file and return its parsed contents alongside the
/// directory the file lives in (used to resolve PNG sidecar paths
/// relative to the JSON).
///
/// # Errors
///
/// Returns an error if the file can't be read, isn't valid JSON, or the
/// `schema` field doesn't match [`EXPECTED_SCHEMA`].
pub fn load_doc(path: &Path) -> Result<(TranslationDocJson, PathBuf)> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let doc: TranslationDocJson =
        serde_json::from_slice(&bytes).with_context(|| format!("parsing {}", path.display()))?;
    if doc.schema != EXPECTED_SCHEMA {
        bail!(
            "unsupported schema {:?} in {} (expected {})",
            doc.schema,
            path.display(),
            EXPECTED_SCHEMA
        );
    }
    let base = path
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    Ok((doc, base))
}

/// Convert one or more loaded JSON documents into the library's value type.
///
/// `docs` is a list of `(parsed_doc, base_directory)` pairs as returned
/// by [`load_doc`]. The base directory is used to resolve relative PNG
/// paths in font groups.
///
/// # Errors
///
/// Returns an error if any file group declares an unknown `format`, any
/// STX slot is referenced twice across all input docs, any font glyph
/// codepoint is referenced twice within one font group, any
/// `char`/`codepoint` pair disagrees, or any glyph's PNG can't be read.
pub fn merge_docs(docs: Vec<(TranslationDocJson, PathBuf)>) -> Result<TranslationSet> {
    let mut set = TranslationSet::default();
    let mut stx_seen: std::collections::HashSet<(String, String, String, u32, u32)> =
        std::collections::HashSet::new();

    for (doc, base) in docs {
        if set.source_language.is_empty() {
            set.source_language = doc.source_language;
        }
        if set.target_language.is_empty() {
            set.target_language = doc.target_language;
        }
        for fg in doc.files {
            match fg {
                FileGroupJson::Stx(stx) => convert_stx_group(stx, &mut stx_seen, &mut set)?,
                FileGroupJson::Font(font) => convert_font_group(font, &base, &mut set)?,
            }
        }
    }

    Ok(set)
}

fn convert_stx_group(
    fg: StxFileGroupJson,
    seen: &mut std::collections::HashSet<(String, String, String, u32, u32)>,
    set: &mut TranslationSet,
) -> Result<()> {
    let mut entries = Vec::with_capacity(fg.entries.len());
    for e in fg.entries {
        let key = (
            fg.cpk.clone(),
            fg.cpk_path.clone(),
            fg.spc_member.clone(),
            e.table,
            e.index,
        );
        if !seen.insert(key.clone()) {
            return Err(anyhow!(
                "duplicate entry for {}::{}::{} table {} index {}",
                key.0,
                key.1,
                key.2,
                key.3,
                key.4
            ));
        }
        entries.push(StxEntryPatch {
            table: e.table,
            index: e.index,
            source: e.source,
            target: e.target,
        });
    }
    set.files.push(TranslationFileGroup {
        cpk: fg.cpk,
        cpk_path: fg.cpk_path,
        spc_member: fg.spc_member,
        format: FileFormat::Stx(StxFileGroup { entries }),
    });
    Ok(())
}

fn convert_font_group(fg: FontFileGroupJson, base: &Path, set: &mut TranslationSet) -> Result<()> {
    // Atlas geometry. `format` names the existing atlas we read from — the
    // shipped BC4 or, when re-applying, ARGB8888. Patched atlases are always
    // re-emitted uncompressed (ARGB8888) so the anti-aliased edges survive.
    // Reject other source formats up front with a clear message.
    let atlas = match fg.atlas {
        Some(a) => {
            let supported =
                a.format.eq_ignore_ascii_case("BC4") || a.format.eq_ignore_ascii_case("ARGB8888");
            if !supported {
                return Err(anyhow!(
                    "unsupported atlas format {:?} in font group {}::{}::{} \
                     (only BC4 and ARGB8888 are supported)",
                    a.format,
                    fg.cpk,
                    fg.cpk_path,
                    fg.spc_member,
                ));
            }
            Some(AtlasSpec {
                width: a.width,
                height: a.height,
            })
        }
        None => None,
    };

    let mut codepoints: std::collections::HashSet<u32> = std::collections::HashSet::new();
    let mut glyphs = Vec::with_capacity(fg.glyphs.len());
    for g in fg.glyphs {
        if !codepoints.insert(g.codepoint) {
            return Err(anyhow!(
                "duplicate glyph codepoint U+{:04X} in font group {}::{}::{}",
                g.codepoint,
                fg.cpk,
                fg.cpk_path,
                fg.spc_member,
            ));
        }
        if let Some(c) = &g.char {
            let mut chars = c.chars();
            let (first, second) = (chars.next(), chars.next());
            match (first, second) {
                (Some(ch), None) if u32::from(ch) == g.codepoint => {}
                _ => {
                    return Err(anyhow!(
                        "glyph codepoint U+{:04X} disagrees with `char` field {:?} \
                         in font group {}::{}",
                        g.codepoint,
                        c,
                        fg.cpk_path,
                        fg.spc_member,
                    ));
                }
            }
        }
        let size = g.size.map(|s| (s.width, s.height));
        // Decode the glyph image to alpha8 if present. The library expects
        // a single-channel buffer of length `size.0 * size.1`.
        let glyph_alpha8 = if let Some(rel) = &g.image_path {
            let path = base.join(rel);
            let alpha = load_glyph_png_as_alpha8(&path, size, g.codepoint)?;
            Some(alpha)
        } else {
            None
        };
        glyphs.push(FontGlyphPatch {
            codepoint: g.codepoint,
            glyph_alpha8,
            position: g.position.map(|p| (p.x, p.y)),
            size,
            kerning: g.kerning.map(|k| (k.left, k.right, k.vertical)),
        });
    }
    set.files.push(TranslationFileGroup {
        cpk: fg.cpk,
        cpk_path: fg.cpk_path,
        spc_member: fg.spc_member,
        format: FileFormat::Font(FontFileGroup {
            font_name: fg.font_name,
            atlas,
            glyphs,
        }),
    });
    Ok(())
}

/// Decode a glyph PNG into a single-channel alpha8 buffer.
///
/// Alpha channel only — provide RGBA with a transparent background. The DR V3
/// atlas convention is "background = 0, ink = 255", which a transparent-
/// background, opaque-ink glyph maps to directly: the alpha channel passes
/// through unchanged.
fn load_glyph_png_as_alpha8(
    path: &Path,
    declared_size: Option<(u8, u8)>,
    codepoint: u32,
) -> Result<Vec<u8>> {
    let img =
        image::open(path).with_context(|| format!("decoding glyph PNG {}", path.display()))?;
    let (w, h) = (img.width(), img.height());
    if let Some((dw, dh)) = declared_size
        && (u32::from(dw) != w || u32::from(dh) != h)
    {
        return Err(anyhow!(
            "glyph U+{:04X} PNG {} is {}×{} but size field declares {}×{}",
            codepoint,
            path.display(),
            w,
            h,
            dw,
            dh,
        ));
    }
    let rgba = img.to_rgba8();
    // Use the alpha channel directly. For typical font glyph exports
    // this matches the DR V3 atlas convention (transparent → 0,
    // opaque ink → 255).
    let alpha: Vec<u8> = rgba.pixels().map(|p| p.0[3]).collect();
    Ok(alpha)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> Result<TranslationSet> {
        let doc: TranslationDocJson = serde_json::from_str(json)?;
        merge_docs(vec![(doc, PathBuf::from("."))])
    }

    #[test]
    fn font_glyph_object_shape_maps_to_model_tuples() {
        // Metadata-only glyph (no `image_path`) so the test needs no file IO.
        let json = r#"{
          "schema": "drv3-translate/v1",
          "files": [{
            "format": "font",
            "cpk": "a.cpk",
            "cpk_path": "x/y.spc",
            "spc_member": "f.stx",
            "atlas": { "width": 2048, "height": 200, "format": "BC4" },
            "glyphs": [{
              "codepoint": 196,
              "char": "Ä",
              "position": { "x": 10, "y": 20 },
              "size": { "width": 30, "height": 40 },
              "kerning": { "left": 1, "right": -2, "vertical": 5 }
            }]
          }]
        }"#;
        let set = parse(json).expect("parses new object shape");
        let FileFormat::Font(fg) = &set.files[0].format else {
            panic!("expected a font group");
        };
        assert_eq!(
            fg.atlas,
            Some(AtlasSpec {
                width: 2048,
                height: 200
            })
        );
        let g = &fg.glyphs[0];
        assert_eq!(g.codepoint, 196);
        assert_eq!(g.position, Some((10, 20)));
        assert_eq!(g.size, Some((30, 40)));
        assert_eq!(g.kerning, Some((1, -2, 5)));
        assert!(g.glyph_alpha8.is_none());
    }

    #[test]
    fn legacy_png_field_is_rejected() {
        // `deny_unknown_fields` makes the old `png` key a hard error rather
        // than silently dropping the image reference.
        let json = r#"{
          "schema": "drv3-translate/v1",
          "files": [{
            "format": "font", "cpk": "a", "cpk_path": "b", "spc_member": "c",
            "glyphs": [{
              "codepoint": 65,
              "png": "x.png",
              "position": { "x": 0, "y": 0 },
              "size": { "width": 1, "height": 1 }
            }]
          }]
        }"#;
        assert!(serde_json::from_str::<TranslationDocJson>(json).is_err());
    }

    #[test]
    fn array_geometry_is_rejected() {
        // Glyph geometry must be a named object. The legacy positional-array
        // form (`[x, y]`) is no longer accepted — the map-only deserializer
        // rejects a JSON array as an invalid type.
        let json = r#"{
          "schema": "drv3-translate/v1",
          "files": [{
            "format": "font", "cpk": "a", "cpk_path": "b", "spc_member": "c",
            "glyphs": [{
              "codepoint": 65,
              "position": [10, 20],
              "size": { "width": 1, "height": 1 }
            }]
          }]
        }"#;
        assert!(serde_json::from_str::<TranslationDocJson>(json).is_err());
    }
}
