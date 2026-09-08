//! The window's colour schemes and font, and where those choices are kept.
//!
//! Its own preferences file rather than a field on [`Settings`](auris_session::Settings), where
//! the interface language lives. The language is kept down there because the command line
//! frontend answers in it too, and being told twice is how the two come to disagree. A colour
//! scheme has no such second reader: nothing at or below `auris-session` has a window to paint.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::theme::{
    DEFAULT_SCHEME, GradientStop, Scheme, Theme, contrast_ratio, scheme, scheme_or_default,
    ui_font_for,
};

/// A user-created palette, stored as the same parameters used by the built-in schemes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CustomScheme {
    /// Stable identifier used by the selected-scheme preference.
    pub id: String,
    /// Name shown in the scheme picker.
    pub name: String,
    /// Hue of the neutral surfaces, from zero to one.
    pub hue: f32,
    /// Saturation of the neutral surfaces, from zero to one.
    pub chroma: f32,
    /// Lightness of the background, in the supported light or dark range.
    pub base: f32,
    /// Opaque accent colour packed as `0xRRGGBB`.
    pub accent: u32,
    /// Optional RGB colours for track, clip and library palette slots.
    #[serde(default)]
    pub track_palette: [Option<u32>; 8],
    /// Optional soft/loud endpoints of the piano-roll velocity gradient.
    #[serde(default)]
    pub velocity_palette: [Option<u32>; 2],
    /// Interior velocity-gradient control points in increasing position order.
    #[serde(default)]
    pub velocity_stops: Vec<GradientStop>,
    /// Optional RGB colours for active, warning, danger and mute indicators.
    #[serde(default)]
    pub signal_palette: [Option<u32>; 4],
}

impl CustomScheme {
    /// Copies a built-in or custom scheme as the starting point for a new palette.
    pub fn from_scheme(id: String, name: String, base: &Scheme<'_>) -> Self {
        let color: gpui::Rgba = base.accent.into();
        let channel = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u32;
        Self {
            id,
            name,
            hue: base.hue,
            chroma: base.chroma,
            base: base.base,
            accent: (channel(color.r) << 16) | (channel(color.g) << 8) | channel(color.b),
            track_palette: base.track_palette,
            velocity_palette: base.velocity_palette,
            velocity_stops: base.velocity_stops.to_vec(),
            signal_palette: base.signal_palette,
        }
    }

    /// Borrows this palette's parameters without requiring static or leaked names.
    pub fn definition(&self) -> Scheme<'_> {
        Scheme {
            id: &self.id,
            name: &self.name,
            hue: self.hue,
            chroma: self.chroma,
            base: self.base,
            accent: gpui::rgb(self.accent).into(),
            track_palette: self.track_palette,
            velocity_palette: self.velocity_palette,
            velocity_stops: &self.velocity_stops,
            signal_palette: self.signal_palette,
        }
    }

    /// Rejects malformed colours and neutral palettes whose labels would be unreadable.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.id.trim().is_empty()
            || self.id.chars().count() > 80
            || self.id.chars().any(char::is_control)
            || scheme(&self.id).is_some()
        {
            return Err("the theme identifier is empty, reserved, or invalid");
        }
        if self.name.trim().is_empty()
            || self.name.chars().count() > 80
            || self.name.chars().any(char::is_control)
        {
            return Err("the theme name must contain between one and eighty visible characters");
        }
        if ![self.hue, self.chroma, self.base]
            .into_iter()
            .all(|value| value.is_finite() && (0.0..=1.0).contains(&value))
            || self.accent > 0xff_ffff
            || self
                .track_palette
                .iter()
                .chain(self.velocity_palette.iter())
                .chain(self.signal_palette.iter())
                .flatten()
                .any(|color| *color > 0xff_ffff)
        {
            return Err("the theme contains an invalid colour");
        }
        let mut previous = 0.0;
        for stop in &self.velocity_stops {
            if !stop.position.is_finite()
                || stop.position <= previous
                || stop.position >= 1.0
                || stop.color > 0xff_ffff
            {
                return Err(
                    "gradient stops require unique increasing positions between zero and one and valid RGB colours",
                );
            }
            previous = stop.position;
        }
        // Mid-tone backgrounds leave too little contrast for the fixed hierarchy of surfaces.
        // Keeping both ranges clear of the extremes also preserves the recessed surface.
        if !(0.04..=0.20).contains(&self.base) && !(0.90..=0.98).contains(&self.base) {
            return Err("the background must be a dark or light shade");
        }
        let theme = Theme::from_scheme(&self.definition());
        for background in [
            theme.background,
            theme.surface,
            theme.surface_raised,
            theme.surface_sunken,
            theme.surface_hover,
        ] {
            if contrast_ratio(theme.text, background) < 7.0
                || contrast_ratio(theme.text_muted, background) < 4.5
                || contrast_ratio(theme.text_faint, background) < 3.5
            {
                return Err("the neutral colours do not provide enough text contrast");
            }
        }
        Ok(())
    }
}

/// Everything in `appearance.json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    /// Id of the chosen colour scheme.
    pub scheme: String,
    /// An installed font family, or the platform interface font when unset.
    pub font_family: Option<String>,
    /// Palettes created in the settings window.
    pub custom_schemes: Vec<CustomScheme>,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            scheme: DEFAULT_SCHEME.to_string(),
            font_family: None,
            custom_schemes: Vec::new(),
        }
    }
}

impl Appearance {
    /// Where the file lives.
    pub fn path() -> PathBuf {
        auris_session::config_dir().join("appearance.json")
    }

    /// Loads the file, falling back to the defaults.
    ///
    /// A missing file is a first run. A malformed one is logged and then also falls back, and so
    /// are invalid custom palettes. An unknown selected scheme falls back without discarding
    /// the font or other palettes: the file is user-editable text that outlives this build.
    pub fn load() -> Self {
        let path = Self::path();
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        match serde_json::from_str::<Self>(&text) {
            Ok(appearance) => appearance.normalised(),
            Err(error) => {
                log::warn!("ignoring malformed {}: {error}", path.display());
                Self::default()
            }
        }
    }

    /// Writes the file, creating the configuration directory if needed.
    pub fn save(&self) -> std::io::Result<()> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(self)
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        std::fs::write(path, text)
    }

    /// The palette this choice describes.
    pub fn theme(&self) -> Theme {
        let mut theme = Theme::from_scheme(&self.selected_scheme());
        theme.font = ui_font_for(self.font_family.as_deref());
        theme
    }

    /// The built-in or valid custom scheme with this identifier.
    pub fn scheme_definition(&self, id: &str) -> Option<Scheme<'_>> {
        scheme(id).copied().or_else(|| {
            self.custom_schemes
                .iter()
                .find(|entry| entry.id == id && entry.validate().is_ok())
                .map(CustomScheme::definition)
        })
    }

    /// The selected scheme, with the default as a fallback for unknown preferences.
    pub fn selected_scheme(&self) -> Scheme<'_> {
        self.scheme_definition(&self.scheme)
            .unwrap_or_else(|| *scheme_or_default(DEFAULT_SCHEME))
    }

    /// Repairs individual invalid preferences while retaining every unrelated valid choice.
    pub fn normalised(mut self) -> Self {
        let mut ids = std::collections::BTreeSet::new();
        let mut names: std::collections::BTreeSet<String> = crate::theme::SCHEMES
            .iter()
            .map(|entry| entry.name.to_lowercase())
            .collect();
        self.custom_schemes.retain_mut(|entry| {
            entry.name = entry.name.trim().to_owned();
            if let Err(error) = entry.validate() {
                log::warn!("ignoring custom colour scheme `{}`: {error}", entry.id);
                return false;
            }
            if ids.contains(&entry.id) || names.contains(&entry.name.to_lowercase()) {
                log::warn!("ignoring duplicate custom colour scheme `{}`", entry.id);
                return false;
            }
            ids.insert(entry.id.clone());
            names.insert(entry.name.to_lowercase());
            true
        });
        if self.scheme_definition(&self.scheme).is_none() {
            log::warn!("ignoring unknown colour scheme `{}`", self.scheme);
            self.scheme = DEFAULT_SCHEME.to_owned();
        }
        self.font_family = self.font_family.and_then(|family| {
            let family = family.trim();
            (!family.is_empty()
                && family.chars().count() <= 200
                && !family.chars().any(char::is_control))
            .then(|| family.to_owned())
        });
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_or_unreadable_file_is_the_default_scheme() {
        let empty: Appearance = serde_json::from_str("{}").unwrap();
        assert_eq!(empty, Appearance::default());
        assert_eq!(empty.theme().scheme, DEFAULT_SCHEME);
    }

    #[test]
    fn a_scheme_this_build_does_not_have_costs_its_colour_and_nothing_else() {
        // The file outlives the build that wrote it, so this is reachable without anybody doing
        // anything wrong — and starting in the default palette beats not starting.
        let stored: Appearance = serde_json::from_str(r#"{"scheme":"chartreuse"}"#).unwrap();
        assert_eq!(stored.theme().scheme, DEFAULT_SCHEME);
    }

    #[test]
    fn a_chosen_scheme_round_trips() {
        let chosen = Appearance {
            scheme: "daylight".to_string(),
            ..Appearance::default()
        };
        let text = serde_json::to_string(&chosen).unwrap();
        assert_eq!(serde_json::from_str::<Appearance>(&text).unwrap(), chosen);
        assert_eq!(chosen.theme().scheme, "daylight");
    }

    fn custom() -> CustomScheme {
        CustomScheme::from_scheme(
            "custom-studio".to_owned(),
            "Studio".to_owned(),
            scheme_or_default("midnight"),
        )
    }

    #[test]
    fn a_custom_palette_and_font_round_trip_without_static_names() {
        let mut custom = custom();
        custom.accent = 0x66_ddee;
        let appearance = Appearance {
            scheme: custom.id.clone(),
            font_family: Some("Example UI".to_owned()),
            custom_schemes: vec![custom],
        };
        let text = serde_json::to_string(&appearance).unwrap();
        let loaded = serde_json::from_str::<Appearance>(&text)
            .unwrap()
            .normalised();
        assert_eq!(loaded, appearance);
        let theme = loaded.theme();
        assert_eq!(theme.scheme, "custom-studio");
        assert_eq!(theme.accent, gpui::Hsla::from(gpui::rgb(0x66_ddee)));
        assert_eq!(theme.font.family.as_ref(), "Example UI");
        assert_eq!(theme.font.fallbacks, crate::theme::ui_font().fallbacks);
    }

    #[test]
    fn every_builtin_can_be_the_starting_point_for_a_custom_palette() {
        for base in crate::theme::SCHEMES {
            let custom = CustomScheme::from_scheme(
                format!("custom-{}", base.id),
                format!("My {}", base.name),
                base,
            );
            assert_eq!(custom.validate(), Ok(()), "{}", base.name);
            assert_eq!(
                Theme::from_scheme(&custom.definition()).track_palette,
                Theme::from_scheme(base).track_palette,
                "copying {} retains its palette",
                base.name
            );
        }
    }

    #[test]
    fn palette_overrides_round_trip_and_old_preferences_keep_the_defaults() {
        let mut palette = custom();
        palette.track_palette[0] = Some(0x123456);
        palette.track_palette[7] = Some(0xfedcba);
        palette.velocity_palette = [Some(0x245678), Some(0xde4567)];
        palette.velocity_stops = vec![GradientStop {
            position: 0.3,
            color: 0xaabbcc,
        }];
        palette.signal_palette = [Some(0x123456), None, Some(0xaabbcc), Some(0xff8800)];
        let json = serde_json::to_string(&palette).unwrap();
        let loaded: CustomScheme = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded, palette);
        let theme = Theme::from_scheme(&loaded.definition());
        assert_eq!(
            theme.track_palette[0],
            gpui::Hsla::from(gpui::rgb(0x123456))
        );
        assert_eq!(
            theme.track_palette[7],
            gpui::Hsla::from(gpui::rgb(0xfedcba))
        );
        let copy = CustomScheme::from_scheme("copy".into(), "Copy".into(), &loaded.definition());
        assert_eq!(copy.track_palette, loaded.track_palette);
        assert_eq!(copy.velocity_palette, loaded.velocity_palette);
        assert_eq!(copy.velocity_stops, loaded.velocity_stops);
        assert_eq!(copy.signal_palette, loaded.signal_palette);
        assert_eq!(theme.velocity_soft, gpui::Hsla::from(gpui::rgb(0x245678)));
        assert_eq!(theme.velocity_loud, gpui::Hsla::from(gpui::rgb(0xde4567)));

        let mut old = serde_json::to_value(&palette).unwrap();
        old.as_object_mut().unwrap().remove("track_palette");
        old.as_object_mut().unwrap().remove("velocity_palette");
        old.as_object_mut().unwrap().remove("velocity_stops");
        old.as_object_mut().unwrap().remove("signal_palette");
        let loaded: CustomScheme = serde_json::from_value(old).unwrap();
        assert_eq!(loaded.track_palette, [None; 8]);
        assert_eq!(loaded.velocity_palette, [None; 2]);
        assert!(loaded.velocity_stops.is_empty());
        assert_eq!(loaded.signal_palette, [None; 4]);
        let fallback = Theme::from_scheme(&loaded.definition());
        assert_eq!(fallback.velocity_soft, fallback.track_palette[0]);
        assert_eq!(fallback.velocity_loud, fallback.track_palette[2]);
        assert!(loaded.validate().is_ok());
        palette.track_palette[1] = Some(0x1000000);
        assert!(palette.validate().is_err());
        palette.track_palette[1] = None;
        palette.velocity_palette[0] = Some(0x1000000);
        assert!(palette.validate().is_err());
        palette.velocity_palette[0] = None;
        palette.signal_palette[0] = Some(0x1000000);
        assert!(palette.validate().is_err());
    }

    #[test]
    fn malformed_gradient_stops_are_rejected_before_theme_construction() {
        let mut palette = custom();
        for position in [f32::NAN, f32::INFINITY, -0.1, 0.0, 1.0, 1.1] {
            palette.velocity_stops = vec![GradientStop {
                position,
                color: 0xffffff,
            }];
            assert!(palette.validate().is_err());
        }
        for positions in [[0.5, 0.5], [0.7, 0.3]] {
            palette.velocity_stops = positions
                .map(|position| GradientStop {
                    position,
                    color: 0xffffff,
                })
                .to_vec();
            assert!(palette.validate().is_err());
        }
        palette.velocity_stops = vec![GradientStop {
            position: 0.5,
            color: 0x1000000,
        }];
        assert!(palette.validate().is_err());
        palette.velocity_stops.clear();
        assert!(palette.validate().is_ok());
    }

    #[test]
    fn invalid_or_missing_schemes_do_not_discard_unrelated_preferences() {
        let valid = custom();
        let mut invalid = valid.clone();
        invalid.id = "invalid".to_owned();
        invalid.name = "Invalid".to_owned();
        invalid.base = 0.5;
        let appearance = Appearance {
            scheme: invalid.id.clone(),
            font_family: Some("  Example UI  ".to_owned()),
            custom_schemes: vec![invalid, valid.clone()],
        }
        .normalised();
        assert_eq!(appearance.scheme, DEFAULT_SCHEME);
        assert_eq!(appearance.font_family.as_deref(), Some("Example UI"));
        assert_eq!(appearance.custom_schemes, vec![valid]);
    }

    #[test]
    fn invalid_preferences_cannot_override_builtin_or_existing_custom_schemes() {
        let first = custom();
        let mut duplicate_id = first.clone();
        duplicate_id.name = "Another".to_owned();
        let mut duplicate_name = first.clone();
        duplicate_name.id = "custom-duplicate".to_owned();
        duplicate_name.name = " studio ".to_owned();
        let mut reserved = first.clone();
        reserved.id = DEFAULT_SCHEME.to_owned();
        let appearance = Appearance {
            custom_schemes: vec![first.clone(), duplicate_id, duplicate_name, reserved],
            ..Appearance::default()
        }
        .normalised();
        assert_eq!(appearance.custom_schemes, vec![first]);
    }

    #[test]
    fn colour_and_name_validation_rejects_malformed_preferences() {
        for value in [f32::NAN, f32::INFINITY, -0.1, 1.1] {
            let mut palette = custom();
            palette.hue = value;
            assert!(palette.validate().is_err());
        }
        for name in ["", "   ", "Studio\nBlue"] {
            let mut palette = custom();
            palette.name = name.to_owned();
            assert!(palette.validate().is_err());
        }
        let mut palette = custom();
        palette.accent = 0x01ff_ffff;
        assert!(palette.validate().is_err());
        let appearance = Appearance {
            font_family: Some("\n\t".to_owned()),
            ..Appearance::default()
        }
        .normalised();
        assert_eq!(appearance.font_family, None);
    }
}
