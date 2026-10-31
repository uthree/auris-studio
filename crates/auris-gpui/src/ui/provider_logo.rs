//! Embedded provider marks shared by settings and the model picker.

use std::borrow::Cow;

use gpui::{AssetSource, Hsla, IntoElement, SharedString, div, prelude::*, svg};

const MARKS: &[(&str, &[u8])] = &[
    (
        "sounds/keys.svg",
        include_bytes!("../../assets/sounds/keys.svg"),
    ),
    (
        "sounds/synth.svg",
        include_bytes!("../../assets/sounds/synth.svg"),
    ),
    (
        "sounds/strings.svg",
        include_bytes!("../../assets/sounds/strings.svg"),
    ),
    (
        "sounds/wind.svg",
        include_bytes!("../../assets/sounds/wind.svg"),
    ),
    (
        "sounds/drums.svg",
        include_bytes!("../../assets/sounds/drums.svg"),
    ),
    (
        "sounds/voice.svg",
        include_bytes!("../../assets/sounds/voice.svg"),
    ),
    (
        "sounds/wave.svg",
        include_bytes!("../../assets/sounds/wave.svg"),
    ),
    (
        "sounds/bell.svg",
        include_bytes!("../../assets/sounds/bell.svg"),
    ),
    (
        "sounds/mallet.svg",
        include_bytes!("../../assets/sounds/mallet.svg"),
    ),
    (
        "icons/menu.svg",
        include_bytes!("../../assets/icons/menu.svg"),
    ),
    (
        "icons/sound-sound.svg",
        include_bytes!("../../assets/icons/sound-sound.svg"),
    ),
    (
        "icons/sound-piano.svg",
        include_bytes!("../../assets/icons/sound-piano.svg"),
    ),
    (
        "icons/sound-organ.svg",
        include_bytes!("../../assets/icons/sound-organ.svg"),
    ),
    (
        "icons/sound-guitar.svg",
        include_bytes!("../../assets/icons/sound-guitar.svg"),
    ),
    (
        "icons/sound-bass.svg",
        include_bytes!("../../assets/icons/sound-bass.svg"),
    ),
    (
        "icons/sound-strings.svg",
        include_bytes!("../../assets/icons/sound-strings.svg"),
    ),
    (
        "icons/sound-brass.svg",
        include_bytes!("../../assets/icons/sound-brass.svg"),
    ),
    (
        "icons/sound-wind.svg",
        include_bytes!("../../assets/icons/sound-wind.svg"),
    ),
    (
        "icons/sound-drum.svg",
        include_bytes!("../../assets/icons/sound-drum.svg"),
    ),
    (
        "icons/sound-bell.svg",
        include_bytes!("../../assets/icons/sound-bell.svg"),
    ),
    (
        "icons/sound-voice.svg",
        include_bytes!("../../assets/icons/sound-voice.svg"),
    ),
    (
        "icons/sound-synth.svg",
        include_bytes!("../../assets/icons/sound-synth.svg"),
    ),
    (
        "icons/sound-pad.svg",
        include_bytes!("../../assets/icons/sound-pad.svg"),
    ),
    (
        "icons/sound-pluck.svg",
        include_bytes!("../../assets/icons/sound-pluck.svg"),
    ),
    (
        "icons/caption-minimize.svg",
        include_bytes!("../../assets/icons/caption-minimize.svg"),
    ),
    (
        "icons/caption-maximize.svg",
        include_bytes!("../../assets/icons/caption-maximize.svg"),
    ),
    (
        "icons/caption-restore.svg",
        include_bytes!("../../assets/icons/caption-restore.svg"),
    ),
    (
        "icons/caption-close.svg",
        include_bytes!("../../assets/icons/caption-close.svg"),
    ),
    (
        "providers/openai.svg",
        include_bytes!("../../assets/providers/openai.svg"),
    ),
    (
        "providers/claude.svg",
        include_bytes!("../../assets/providers/claude.svg"),
    ),
    (
        "providers/opencode.svg",
        include_bytes!("../../assets/providers/opencode.svg"),
    ),
    (
        "providers/antigravity.svg",
        include_bytes!("../../assets/providers/antigravity.svg"),
    ),
    (
        "providers/ollama.svg",
        include_bytes!("../../assets/providers/ollama.svg"),
    ),
];

/// Provider artwork included in the executable, with no runtime downloads.
pub(crate) struct ProviderAssets;

impl AssetSource for ProviderAssets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        Ok(MARKS
            .iter()
            .find(|(name, _)| *name == path)
            .map(|(_, bytes)| Cow::Borrowed(*bytes)))
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        Ok(MARKS
            .iter()
            .filter(|(name, _)| name.starts_with(path))
            .map(|(name, _)| (*name).into())
            .collect())
    }
}

/// Resolve the connection provider rather than guessing from a model's name.
pub(crate) fn provider_id<'a>(backend: &'a str, provider: &'a str) -> &'a str {
    match backend {
        "codex" => "openai",
        "claude" | "opencode" | "antigravity" => backend,
        _ if provider == "ollama" => "ollama",
        _ => "api",
    }
}

/// A monochrome provider mark that follows the current theme's foreground.
pub(crate) fn provider_logo(provider: &str, color: Hsla) -> impl IntoElement + use<> {
    let path = match provider {
        "openai" => Some("providers/openai.svg"),
        "claude" => Some("providers/claude.svg"),
        "opencode" => Some("providers/opencode.svg"),
        "antigravity" => Some("providers/antigravity.svg"),
        "ollama" => Some("providers/ollama.svg"),
        _ => None,
    };
    div()
        .size_5()
        .flex_shrink_0()
        .text_color(color)
        .when_some(path, |view, path| {
            view.child(svg().path(path).size_full().text_color(color))
        })
        .when(path.is_none(), |view| {
            view.flex()
                .items_center()
                .justify_center()
                .text_xs()
                .child("API")
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_sound_family_has_a_distinct_embedded_svg() {
        let paths: Vec<_> = auris_session::SoundIcon::ALL
            .iter()
            .copied()
            .map(
                |family| match crate::ui::icons::Icon::Sound(family).source() {
                    atelier::IconSource::Svg(path) => path,
                    other => {
                        panic!("sound family {family:?} must use its embedded SVG, got {other:?}")
                    }
                },
            )
            .collect();
        assert_eq!(
            paths.iter().collect::<std::collections::HashSet<_>>().len(),
            paths.len()
        );
        for path in paths {
            assert!(ProviderAssets.load(&path).unwrap().is_some(), "{path}");
        }
    }

    #[gpui::test]
    fn embedded_provider_and_extension_icons_render_without_external_files(
        cx: &mut gpui::TestAppContext,
    ) {
        let renderer = cx.update(|cx| cx.svg_renderer());
        for (path, _) in MARKS {
            let bytes = ProviderAssets.load(path).unwrap().unwrap();
            let image = gpui::Image::from_bytes(gpui::ImageFormat::Svg, bytes.into_owned())
                .to_image_data(renderer.clone())
                .unwrap();
            assert!(
                image
                    .as_bytes(0)
                    .unwrap()
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .any(|pixel| pixel[3] > 0),
                "{path} is empty"
            );
        }
    }

    #[test]
    fn an_api_dialect_does_not_claim_to_be_the_openai_provider() {
        assert_eq!(provider_id("rig", "openai"), "api");
        assert_eq!(provider_id("codex", "ollama"), "openai");
        assert_eq!(provider_id("claude", "openai"), "claude");
    }
}
