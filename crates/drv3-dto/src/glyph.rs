//! Glyph-geometry DTOs shared by the `spft` exchange schema and the
//! translation-patch schema.
//!
//! Glyph `position` / `size` / `kerning` are accepted only as named JSON
//! objects; the `object_only!` macro generates a map-only `Deserialize` that
//! rejects the legacy positional-array form (`[x, y]`). The types also derive
//! `Serialize` so `drv3-cli spft dump` can emit them.

use serde::Serialize;

/// Generate a map-only [`Deserialize`] for a fixed-shape struct: it accepts a
/// JSON object carrying the named fields and rejects every other shape —
/// notably a JSON array, so the legacy positional form (`[x, y]`) errors
/// instead of silently mapping onto the fields in declaration order.
macro_rules! object_only {
    ($t:ident, $expecting:literal, { $($f:ident : $ty:ty),+ $(,)? }) => {
        impl<'de> serde::Deserialize<'de> for $t {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                struct FieldVisitor;
                impl<'de> serde::de::Visitor<'de> for FieldVisitor {
                    type Value = $t;
                    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                        f.write_str($expecting)
                    }
                    fn visit_map<M: serde::de::MapAccess<'de>>(
                        self,
                        mut map: M,
                    ) -> Result<$t, M::Error> {
                        $(let mut $f: Option<$ty> = None;)+
                        while let Some(key) = map.next_key::<String>()? {
                            match key.as_str() {
                                $(stringify!($f) => {
                                    if $f.is_some() {
                                        return Err(serde::de::Error::duplicate_field(stringify!($f)));
                                    }
                                    $f = Some(map.next_value()?);
                                })+
                                other => {
                                    return Err(serde::de::Error::unknown_field(
                                        other,
                                        &[$(stringify!($f)),+],
                                    ));
                                }
                            }
                        }
                        Ok($t {
                            $($f: $f
                                .ok_or_else(|| serde::de::Error::missing_field(stringify!($f)))?,)+
                        })
                    }
                }
                deserializer.deserialize_map(FieldVisitor)
            }
        }
    };
}

/// Top-left atlas coordinate of a glyph, in pixels (12-bit each).
#[derive(Debug, Serialize)]
pub struct PositionJson {
    pub x: u16,
    pub y: u16,
}
object_only!(PositionJson, "a glyph position object with `x` and `y`", { x: u16, y: u16 });

/// Glyph bounding-box dimensions, in pixels.
#[derive(Debug, Serialize)]
pub struct SizeJson {
    pub width: u8,
    pub height: u8,
}
object_only!(SizeJson, "a glyph size object with `width` and `height`", { width: u8, height: u8 });

/// Per-glyph spacing deltas, in signed pixels: `left`/`right` are the
/// horizontal side bearings, `vertical` shifts the glyph up/down.
#[derive(Debug, Serialize)]
pub struct KerningJson {
    pub left: i8,
    pub right: i8,
    pub vertical: i8,
}
object_only!(
    KerningJson,
    "a glyph kerning object with `left`, `right`, and `vertical`",
    { left: i8, right: i8, vertical: i8 }
);
