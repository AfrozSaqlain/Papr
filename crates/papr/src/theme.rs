//! Built-in and user-defined color themes.

use std::{fs, path::Path};

use ratatui::style::Color;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Theme loading and color parsing errors.
#[derive(Debug, Error)]
pub enum ThemeError {
    /// Theme file could not be read.
    #[error("could not read theme: {0}")]
    Io(#[from] std::io::Error),
    /// Theme TOML is invalid.
    #[error("invalid theme TOML: {0}")]
    Toml(#[from] toml::de::Error),
    /// A color was not a `#RRGGBB` value.
    #[error("invalid color '{0}'; expected #RRGGBB")]
    InvalidColor(String),
    /// Requested built-in theme is unknown.
    #[error("unknown theme '{0}'")]
    UnknownTheme(String),
}

/// Serializable theme palette.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThemeSpec {
    /// Theme display name.
    pub name: String,
    /// Main terminal background.
    pub background: String,
    /// Raised panel background.
    pub surface: String,
    /// Primary foreground.
    pub text: String,
    /// Muted labels.
    pub muted: String,
    /// Primary selection and focus color.
    pub accent: String,
    /// Secondary accent.
    pub secondary: String,
    /// Success state.
    pub success: String,
    /// Warning state.
    pub warning: String,
    /// Error state.
    pub error: String,
    /// Borders and separators.
    pub border: String,
}

/// Parsed palette used directly by ratatui widgets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    /// Theme display name.
    pub name: String,
    /// Main terminal background.
    pub background: Color,
    /// Raised panel background.
    pub surface: Color,
    /// Primary foreground.
    pub text: Color,
    /// Muted labels.
    pub muted: Color,
    /// Primary selection and focus color.
    pub accent: Color,
    /// Secondary accent.
    pub secondary: Color,
    /// Success state.
    pub success: Color,
    /// Warning state.
    pub warning: Color,
    /// Error state.
    pub error: Color,
    /// Borders and separators.
    pub border: Color,
}

impl Theme {
    /// List of built-in theme names.
    pub const BUILTIN_THEMES: &'static [&'static str] = &[
        "catppuccin-mocha",
        "catppuccin-macchiato",
        "catppuccin-frappe",
        "catppuccin-latte",
        "tokyo-night",
        "gruvbox",
        "nord",
        "dracula",
        "light",
        "rose-pink-dark",
        "rose-pink-light",
        "everforest",
        "kanagawa",
        "one-dark",
        "cyberpunk",
        "ember",
        "verdant",
        "lavender",
        "parchment",
    ];

    /// Load a built-in theme by name, or a custom theme from a TOML path.
    ///
    /// # Errors
    ///
    /// Returns an error for unknown themes, malformed files, or invalid colors.
    pub fn load(name_or_path: &str) -> Result<Self, ThemeError> {
        let path = Path::new(name_or_path);
        let spec = if path.is_file() {
            toml::from_str(&fs::read_to_string(path)?)?
        } else {
            builtin(name_or_path).ok_or_else(|| ThemeError::UnknownTheme(name_or_path.into()))?
        };
        Self::try_from(spec)
    }
}

impl TryFrom<ThemeSpec> for Theme {
    type Error = ThemeError;

    fn try_from(spec: ThemeSpec) -> Result<Self, Self::Error> {
        Ok(Self {
            name: spec.name,
            background: parse_color(&spec.background)?,
            surface: parse_color(&spec.surface)?,
            text: parse_color(&spec.text)?,
            muted: parse_color(&spec.muted)?,
            accent: parse_color(&spec.accent)?,
            secondary: parse_color(&spec.secondary)?,
            success: parse_color(&spec.success)?,
            warning: parse_color(&spec.warning)?,
            error: parse_color(&spec.error)?,
            border: parse_color(&spec.border)?,
        })
    }
}

fn parse_color(value: &str) -> Result<Color, ThemeError> {
    let value = value.strip_prefix('#').unwrap_or(value);
    if value.len() != 6 {
        return Err(ThemeError::InvalidColor(value.into()));
    }
    let component = |range| {
        u8::from_str_radix(&value[range], 16).map_err(|_| ThemeError::InvalidColor(value.into()))
    };
    Ok(Color::Rgb(
        component(0..2)?,
        component(2..4)?,
        component(4..6)?,
    ))
}

fn builtin(name: &str) -> Option<ThemeSpec> {
    let colors = match name.to_ascii_lowercase().as_str() {
        "catppuccin" | "catppuccin-mocha" | "catppuccinmocha" => (
            "1e1e2e", "313244", "cdd6f4", "7f849c", "89b4fa", "cba6f7", "a6e3a1", "f9e2af",
            "f38ba8", "45475a",
        ),
        "catppuccin-macchiato" | "catppuccinmacchiato" => (
            "24273a", "363a4f", "cad3f5", "8087a2", "8aadf4", "c6a0f6", "a6da95", "eed49f",
            "ed8796", "494d64",
        ),
        "catppuccin-frappe" | "catppuccinfrappe" => (
            "303446", "414559", "c6d0f5", "838ba7", "8caaee", "ca9ee6", "a6d189", "e5c890",
            "e78284", "51576d",
        ),
        "catppuccin-latte" | "catppuccinlatte" => (
            "eff1f5", "ccd0da", "4c4f69", "8c8fa1", "1e66f5", "8839ef", "40a02b", "df8e1d",
            "d20f39", "bcc0cc",
        ),
        "tokyo-night" | "tokyonight" => (
            "1a1b26", "24283b", "c0caf5", "565f89", "7aa2f7", "bb9af7", "9ece6a", "e0af68",
            "f7768e", "414868",
        ),
        "gruvbox" => (
            "282828", "3c3836", "ebdbb2", "928374", "83a598", "d3869b", "b8bb26", "fabd2f",
            "fb4934", "504945",
        ),
        "nord" => (
            "2e3440", "3b4252", "eceff4", "7b88a1", "88c0d0", "b48ead", "a3be8c", "ebcb8b",
            "bf616a", "4c566a",
        ),
        "dracula" => (
            "282a36", "44475a", "f8f8f2", "6272a4", "8be9fd", "bd93f9", "50fa7b", "f1fa8c",
            "ff5555", "6272a4",
        ),
        "light" => (
            "f7f7f5", "ffffff", "202124", "6b7280", "2563eb", "7c3aed", "15803d", "a16207",
            "b91c1c", "d1d5db",
        ),
        "rose-pink" | "rosepink" | "rose-pink-dark" | "rosepinkdark" | "rose-pine-dark"
        | "rosepinedark" => (
            "241b1d", "2d2224", "f5e6e8", "a38f92", "ffb3c1", "e0b1cb", "b7e4c7", "fcd5a1",
            "ff85a1", "3d2d30",
        ),
        "rose-pink-light" | "rosepinklight" | "rose-pine-light" | "rosepinelight" => (
            "faebed", "ffffff", "3c2a2e", "8a7074", "d96a80", "9c528b", "388e3c", "f57c00",
            "d32f2f", "e8d0d4",
        ),
        "everforest" => (
            "2d353b", "343f44", "d3c6aa", "859289", "a7c080", "7fbbb3", "a7c080", "dbbc7f",
            "e67e80", "475258",
        ),
        "kanagawa" => (
            "1f1f28", "2a2a37", "dcd7ba", "727169", "e6c384", "7e9cd8", "76946a", "ff9e3b",
            "c3404b", "363646",
        ),
        "one-dark" | "onedark" => (
            "282c34", "21252b", "abb2bf", "5c6370", "61afef", "c678dd", "98c379", "d19a66",
            "e06c75", "3e4452",
        ),
        "cyberpunk" => (
            "0f0f1a", "1a1a2e", "ffffff", "707080", "00ffff", "ff007f", "39ff14", "ffb300",
            "ff3333", "2a2a40",
        ),
        "ember" => (
            "211512", "34201a", "f5e6d3", "a98e7d", "f07c3e", "d64c32", "8eb65b", "e6ad43",
            "d9554f", "54342a",
        ),
        "verdant" => (
            "14231b", "203329", "e4f0df", "91a998", "75b798", "c4a85d", "8fbd65", "d4ac57",
            "cf6b62", "365044",
        ),
        "lavender" => (
            "211b2e", "302842", "f0eaff", "a69bbd", "bd93f9", "e7a6d7", "a9c78d", "efc078",
            "e5829a", "493d60",
        ),
        "parchment" => (
            "f4eddc", "fffaf0", "342b22", "87796a", "aa6a2a", "8a4d69", "4f7b48", "a06d1e",
            "ae4139", "d9cdb9",
        ),
        _ => return None,
    };
    Some(ThemeSpec {
        name: name.into(),
        background: format!("#{}", colors.0),
        surface: format!("#{}", colors.1),
        text: format!("#{}", colors.2),
        muted: format!("#{}", colors.3),
        accent: format!("#{}", colors.4),
        secondary: format!("#{}", colors.5),
        success: format!("#{}", colors.6),
        warning: format!("#{}", colors.7),
        error: format!("#{}", colors.8),
        border: format!("#{}", colors.9),
    })
}

#[cfg(test)]
mod tests {
    use ratatui::style::Color;

    use super::Theme;

    #[test]
    fn loads_every_builtin_theme() -> Result<(), Box<dyn std::error::Error>> {
        for name in [
            "catppuccin-mocha",
            "catppuccin-macchiato",
            "catppuccin-frappe",
            "catppuccin-latte",
            "tokyo-night",
            "gruvbox",
            "nord",
            "dracula",
            "light",
            "rose-pink-dark",
            "rose-pink-light",
            "everforest",
            "kanagawa",
            "one-dark",
            "cyberpunk",
            "ember",
            "verdant",
            "lavender",
            "parchment",
        ] {
            let theme = Theme::load(name)?;
            assert_ne!(theme.text, theme.background);
        }
        Ok(())
    }

    #[test]
    fn parses_rgb_colors() -> Result<(), Box<dyn std::error::Error>> {
        let theme = Theme::load("catppuccin")?;
        assert_eq!(theme.accent, Color::Rgb(137, 180, 250));
        Ok(())
    }
}
