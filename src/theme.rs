use ratatui::style::Color;
use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Theme {
    #[default]
    Dark,
    Light,
    HighContrast,
    Terminal,
}

impl Theme {
    pub const ALL: [Self; 4] = [Self::Dark, Self::Light, Self::HighContrast, Self::Terminal];

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "dark" | "catppuccin" | "mocha" => Some(Self::Dark),
            "light" | "latte" => Some(Self::Light),
            "high-contrast" | "high_contrast" | "contrast" | "hc" => Some(Self::HighContrast),
            "terminal" | "ansi" | "default" => Some(Self::Terminal),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
            Self::HighContrast => "high-contrast",
            Self::Terminal => "terminal",
        }
    }

    pub fn next(self) -> Self {
        Self::ALL[(self as usize + 1) % Self::ALL.len()]
    }

    pub fn activate(self) {
        ACTIVE.store(self as u8, Ordering::Relaxed);
    }
}

#[derive(Clone, Copy)]
pub struct Palette {
    pub bg: Color,
    pub surface: Color,
    pub text: Color,
    pub sub: Color,
    pub accent: Color,
    pub green: Color,
    pub peach: Color,
    pub mauve: Color,
    pub red: Color,
    pub code_bg: Color,
    pub diff_del_bg: Color,
    pub diff_add_bg: Color,
    pub diff_del_text: Color,
    pub diff_add_text: Color,
}

static ACTIVE: AtomicU8 = AtomicU8::new(Theme::Dark as u8);

pub fn current() -> Palette {
    match ACTIVE.load(Ordering::Relaxed) {
        1 => LIGHT,
        2 => HIGH_CONTRAST,
        3 => TERMINAL,
        _ => DARK,
    }
}

const DARK: Palette = Palette {
    bg: Color::Rgb(30, 30, 46),
    surface: Color::Rgb(49, 50, 68),
    text: Color::Rgb(205, 214, 244),
    sub: Color::Rgb(166, 173, 200),
    accent: Color::Rgb(137, 180, 250),
    green: Color::Rgb(166, 227, 161),
    peach: Color::Rgb(250, 179, 135),
    mauve: Color::Rgb(203, 166, 247),
    red: Color::Rgb(243, 139, 168),
    code_bg: Color::Rgb(40, 42, 58),
    diff_del_bg: Color::Rgb(52, 36, 42),
    diff_add_bg: Color::Rgb(36, 48, 42),
    diff_del_text: Color::Rgb(242, 200, 205),
    diff_add_text: Color::Rgb(190, 230, 200),
};

const LIGHT: Palette = Palette {
    bg: Color::Rgb(239, 241, 245),
    surface: Color::Rgb(220, 224, 232),
    text: Color::Rgb(76, 79, 105),
    sub: Color::Rgb(108, 111, 133),
    accent: Color::Rgb(30, 102, 245),
    green: Color::Rgb(64, 160, 43),
    peach: Color::Rgb(254, 100, 11),
    mauve: Color::Rgb(136, 57, 239),
    red: Color::Rgb(210, 15, 57),
    code_bg: Color::Rgb(204, 208, 218),
    diff_del_bg: Color::Rgb(255, 220, 224),
    diff_add_bg: Color::Rgb(218, 244, 216),
    diff_del_text: Color::Rgb(140, 20, 40),
    diff_add_text: Color::Rgb(30, 105, 35),
};

const HIGH_CONTRAST: Palette = Palette {
    bg: Color::Black,
    surface: Color::Rgb(28, 28, 28),
    text: Color::White,
    sub: Color::Gray,
    accent: Color::Cyan,
    green: Color::LightGreen,
    peach: Color::Yellow,
    mauve: Color::LightMagenta,
    red: Color::LightRed,
    code_bg: Color::Rgb(20, 20, 20),
    diff_del_bg: Color::Rgb(70, 0, 0),
    diff_add_bg: Color::Rgb(0, 55, 0),
    diff_del_text: Color::White,
    diff_add_text: Color::White,
};

const TERMINAL: Palette = Palette {
    bg: Color::Reset,
    surface: Color::Reset,
    text: Color::Reset,
    sub: Color::DarkGray,
    accent: Color::Blue,
    green: Color::Green,
    peach: Color::Yellow,
    mauve: Color::Magenta,
    red: Color::Red,
    code_bg: Color::Reset,
    diff_del_bg: Color::Reset,
    diff_add_bg: Color::Reset,
    diff_del_text: Color::Red,
    diff_add_text: Color::Green,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_names_and_cycles() {
        assert_eq!(Theme::parse("LATTE"), Some(Theme::Light));
        assert_eq!(Theme::parse("hc"), Some(Theme::HighContrast));
        assert_eq!(Theme::Terminal.next(), Theme::Dark);
        assert_eq!(Theme::parse("unknown"), None);
    }
}
