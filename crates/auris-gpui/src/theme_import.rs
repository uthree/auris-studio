//! Adapts VS Code workbench palettes to Auris's surface and document colour roles.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use gpui::Hsla;
use serde::Deserialize;

use crate::appearance::CustomScheme;
use crate::theme::scheme_or_default;

const MAX_BYTES: u64 = 4 * 1024 * 1024;
const MAX_DEPTH: usize = 16;

#[derive(Default, Deserialize)]
struct EditorTheme {
    name: Option<String>,
    include: Option<String>,
    #[serde(default)]
    colors: BTreeMap<String, Option<String>>,
}

fn read_theme(path: &Path, depth: usize, remaining: &mut u64) -> Result<EditorTheme, String> {
    if depth >= MAX_DEPTH {
        return Err("Theme includes are circular or nested too deeply.".into());
    }
    let mut text = String::new();
    File::open(path)
        .map_err(|error| format!("{}: {error}", path.display()))?
        .take(*remaining + 1)
        .read_to_string(&mut text)
        .map_err(|error| error.to_string())?;
    if text.len() as u64 > *remaining {
        return Err("Theme files exceed the 4 MiB import limit.".into());
    }
    *remaining -= text.len() as u64;
    let value = jsonc_parser::parse_to_serde_value::<Option<serde_json::Value>>(
        text.trim_start_matches('\u{feff}'),
        &Default::default(),
    )
    .map_err(|error| error.to_string())?
    .ok_or("The theme file is empty.")?;
    let mut theme: EditorTheme =
        serde_json::from_value(value).map_err(|error| error.to_string())?;
    if let Some(include) = &theme.include {
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        let inherited = read_theme(&parent.join(include), depth + 1, remaining)?;
        let overrides = std::mem::replace(&mut theme.colors, inherited.colors);
        theme.colors.extend(overrides);
    }
    Ok(theme)
}

/// Imports a JSON/JSONC theme and its local includes without executing extension code.
pub(crate) fn import_vscode(path: &Path) -> Result<CustomScheme, String> {
    let mut remaining = MAX_BYTES;
    let source = read_theme(path, 0, &mut remaining)?;
    let color = |key: &str| source.colors.get(key)?.as_deref().and_then(parse_color);
    let background = color("editor.background")
        .filter(|color| color.a == 1.0)
        .ok_or("The theme must define an opaque editor.background colour.")?;
    let base = scheme_or_default(if background.l < 0.5 {
        "one-dark"
    } else {
        "one-light"
    });
    let name = source
        .name
        .as_deref()
        .filter(|name| !name.trim().is_empty() && !name.starts_with('%'))
        .or_else(|| path.file_stem().and_then(|name| name.to_str()))
        .unwrap_or("Imported theme");
    let mut imported = CustomScheme::from_scheme(
        "imported".into(),
        name.trim().chars().take(80).collect(),
        base,
    );
    // Auris derives its surface ladder and readable text rather than copying editor geometry.
    imported.hue = background.h;
    imported.chroma = background.s;
    imported.base = if background.l < 0.5 {
        background.l.clamp(0.04, 0.20)
    } else {
        background.l.clamp(0.90, 0.98)
    };
    let opaque = |color| pack(background.blend(color));
    if let Some(accent) = [
        "focusBorder",
        "button.background",
        "activityBarBadge.background",
        "editorCursor.foreground",
    ]
    .into_iter()
    .filter_map(color)
    .find(|color| color.a > 0.0)
    {
        imported.accent = opaque(accent);
    }
    for (slot, key) in [
        "terminal.ansiBlue",
        "terminal.ansiGreen",
        "terminal.ansiRed",
        "terminal.ansiYellow",
        "terminal.ansiMagenta",
        "terminal.ansiCyan",
        "terminal.ansiBrightYellow",
        "terminal.ansiBrightMagenta",
    ]
    .into_iter()
    .enumerate()
    {
        if let Some(color) = color(key) {
            imported.track_palette[slot] = Some(opaque(color));
        }
    }
    for (slot, key) in [
        "terminal.ansiGreen",
        "editorWarning.foreground",
        "editorError.foreground",
        "terminal.ansiCyan",
    ]
    .into_iter()
    .enumerate()
    {
        if let Some(color) = color(key) {
            imported.signal_palette[slot] = Some(opaque(color));
        }
    }
    imported.velocity_palette = [imported.track_palette[0], imported.track_palette[2]];
    imported.velocity_stops.clear();
    imported.validate().map_err(str::to_owned)?;
    Ok(imported)
}

fn pack(color: Hsla) -> u32 {
    let color: gpui::Rgba = color.into();
    let channel = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u32;
    (channel(color.r) << 16) | (channel(color.g) << 8) | channel(color.b)
}

fn parse_color(text: &str) -> Option<Hsla> {
    let hex = text.strip_prefix('#')?;
    if !matches!(hex.len(), 3 | 4 | 6 | 8) || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let expanded;
    let hex = if hex.len() <= 4 {
        expanded = hex.chars().flat_map(|ch| [ch, ch]).collect::<String>();
        expanded.as_str()
    } else {
        hex
    };
    let rgb = u32::from_str_radix(&hex[..6], 16).ok()?;
    let mut color = Hsla::from(gpui::rgb(rgb));
    if hex.len() == 8 {
        color.a = u8::from_str_radix(&hex[6..], 16).ok()? as f32 / 255.0;
    }
    Some(color)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jsonc_includes_merge_child_colours_and_keep_the_child_name() {
        let directory = tempfile::tempdir().unwrap();
        let parent = directory.path().join("base.json");
        std::fs::write(&parent, r##"{"name":"Base", "colors":{"editor.background":"#161821", "terminal.ansiBlue":"#84a0c6", "focusBorder":"#123"}}"##).unwrap();
        let child = directory.path().join("theme.jsonc");
        std::fs::write(
            &child,
            r##"{
            // Theme name contains a slash, so comments must be parsed as JSONC.
            "name":"Ice / Snow", "include":"./base.json",
            "colors":{"focusBorder":"#abc", "terminal.ansiRed":"#e27878ff",},
        }"##,
        )
        .unwrap();
        let imported = import_vscode(&child).unwrap();
        assert_eq!(imported.name, "Ice / Snow");
        assert_eq!(imported.accent, 0xaabbcc);
        assert_eq!(imported.track_palette[0], Some(0x84a0c6));
        assert_eq!(imported.track_palette[2], Some(0xe27878));
        assert!((imported.base - 0.107843).abs() < 0.00001);
        imported.validate().unwrap();
    }

    #[test]
    fn cyclic_missing_and_unrelated_files_do_not_import() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("theme.json");
        for text in [
            r#"{"include":"theme.json"}"#,
            r#"{"include":"missing.json"}"#,
            r#"{"settings":{}}"#,
            "broken",
        ] {
            std::fs::write(&path, text).unwrap();
            assert!(import_vscode(&path).is_err(), "{text}");
        }
    }

    #[test]
    fn accepts_editor_hex_formats_and_rejects_invalid_colours() {
        for (text, expected) in [
            ("#abc", 0xaabbcc),
            ("#abc8", 0xaabbcc),
            ("#abcdef", 0xabcdef),
            ("#abcdef80", 0xabcdef),
        ] {
            assert_eq!(pack(parse_color(text).unwrap()), expected);
        }
        assert_eq!(parse_color("#0000").unwrap().a, 0.0);
        for text in ["#ab", "#abcdefghi", "red", "#zzzzzz", "#ああ"] {
            assert!(parse_color(text).is_none());
        }
    }
}
