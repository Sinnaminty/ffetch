//! `--json`: everything ffetch found as one JSON object, with structured
//! module values and the palette, so other tools can take their colours from
//! the same image.

use serde::{Serialize, Serializer, ser::SerializeMap};

use crate::{
    info::{Info, ModuleDef, Value},
    palette::{Palette, Roles},
};

/// Bumped on any breaking change to the output's shape.
const SCHEMA: u32 = 1;

#[derive(Serialize)]
struct Document<'a> {
    schema: u32,
    user: &'a str,
    host: &'a str,
    palette: Colors,
    modules: Modules<'a>,
}

/// Colours as "#rrggbb".
#[derive(Serialize)]
struct Colors {
    accent: String,
    secondary: String,
    muted: String,
    /// The image's palette, most common first.
    colors: Vec<String>,
}

/// The modules that had something to report, by id, in the order asked for.
struct Modules<'a>(&'a [(&'static ModuleDef, Value)]);

impl Serialize for Modules<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut map = s.serialize_map(Some(self.0.len()))?;
        for (def, value) in self.0 {
            map.serialize_entry(def.id, value)?;
        }
        map.end()
    }
}

/// The JSON document for `info`, pretty-printed and ending in a newline. The
/// role colours are `roles`, as ffetch paints them (after the theme's
/// adjustments and overrides); `colors` are the palette's, as in the image.
pub fn render(info: &Info, palette: &Palette, roles: Roles) -> String {
    let document = Document {
        schema: SCHEMA,
        user: &info.user,
        host: &info.host,
        palette: Colors {
            accent: roles.accent.hex(),
            secondary: roles.secondary.hex(),
            muted: roles.muted.hex(),
            colors: palette.colors.iter().map(|c| c.hex()).collect(),
        },
        modules: Modules(&info.modules),
    };
    // Only maps with non-string keys or failing `Serialize` impls can fail,
    // and there are neither.
    let mut out = serde_json::to_string_pretty(&document).expect("the output serializes");
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        info::{
            self, Report,
            value::{Kernel, Uptime},
        },
        term::Rgb,
    };

    fn sample(ids: &[&str]) -> Info {
        let modules = ids
            .iter()
            .map(|id| {
                let value = match *id {
                    "kernel" => Value::Kernel(Kernel {
                        release: "6.1".into(),
                    }),
                    _ => Value::Uptime(Uptime { seconds: 60 }),
                };
                (info::find(id).unwrap(), value)
            })
            .collect();
        Info {
            user: "me".into(),
            host: "box".into(),
            modules,
        }
    }

    fn palette() -> (Palette, Roles) {
        let palette = Palette {
            colors: vec![Rgb(0xf2, 0xd7, 0xc5), Rgb(1, 2, 3)],
            accent: Rgb(0xc3, 0x3e, 0x58),
            secondary: Rgb(0xf2, 0xd7, 0xc5),
            muted: Rgb(0xc9, 0xa6, 0x9d),
        };
        let roles = Roles {
            accent: Rgb(0xC3, 0x3E, 0x58),
            secondary: Rgb(255, 255, 255),
            muted: Rgb(0, 0, 0),
        };
        (palette, roles)
    }

    #[test]
    fn document_shape() {
        let (palette, roles) = palette();
        let text = render(&sample(&["kernel", "uptime"]), &palette, roles);
        assert!(text.ends_with("}\n"));
        let json: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "schema": 1,
                "user": "me",
                "host": "box",
                "palette": {
                    "accent": "#c33e58",
                    "secondary": "#ffffff",
                    "muted": "#000000",
                    "colors": ["#f2d7c5", "#010203"],
                },
                "modules": {
                    "kernel": {"release": "6.1"},
                    "uptime": {"seconds": 60},
                },
            })
        );
        assert!(!text.contains('\x1b'));
    }

    #[test]
    fn modules_keep_the_order_asked_for() {
        let (palette, roles) = palette();
        for ids in [["kernel", "uptime"], ["uptime", "kernel"]] {
            let text = render(&sample(&ids), &palette, roles);
            let at = |id: &str| text.find(&format!("\"{id}\": {{")).unwrap();
            assert!(at(ids[0]) < at(ids[1]), "{text}");
        }
        let empty = render(&sample(&[]), &palette, roles);
        assert!(empty.contains("\"modules\": {}"), "{empty}");
        // Display text never leaks in.
        let one = sample(&["uptime"]);
        assert!(!render(&one, &palette, roles).contains(&one.modules[0].1.display()));
    }
}
