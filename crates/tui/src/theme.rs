//! The four palettes (D-089, D-135) as terminal truecolor. Green phosphor is the default.

use ratatui::style::Color;

/// A palette: the colors every screen draws with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    /// Its name, as settings and the header show it.
    pub name: &'static str,
    /// The background.
    pub bg: Color,
    /// Raised surfaces: popups and the selected row's base.
    pub raise: Color,
    /// Rules and borders.
    pub line: Color,
    /// Body text.
    pub fg: Color,
    /// Secondary text: labels and units.
    pub dim: Color,
    /// Text that matters least: placeholders and empty values.
    pub faint: Color,
    /// Downloads.
    pub down: Color,
    /// Uploads and sharing.
    pub up: Color,
    /// Emphasis: the cursor, inputs and work in progress.
    pub hot: Color,
    /// Things that need attention but aren't failures.
    pub warn: Color,
    /// Failures.
    pub bad: Color,
    /// The empty part of bars and maps.
    pub cell_off: Color,
}

const fn rgb(hex: u32) -> Color {
    Color::Rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

impl Theme {
    /// Green phosphor, the default.
    pub const GREEN: Self = Self {
        name: "green",
        bg: rgb(0x020b05),
        raise: rgb(0x06180c),
        line: rgb(0x174a29),
        fg: rgb(0x8af2a4),
        dim: rgb(0x4b9d63),
        faint: rgb(0x2a6a40),
        down: rgb(0x57ff8e),
        up: rgb(0xd2ffe0),
        hot: rgb(0xe9fff0),
        warn: rgb(0xd7ff6a),
        bad: rgb(0xff8a6a),
        cell_off: rgb(0x0b2414),
    };

    /// Amber phosphor.
    pub const AMBER: Self = Self {
        name: "amber",
        bg: rgb(0x0d0700),
        raise: rgb(0x1c1002),
        line: rgb(0x5a3608),
        fg: rgb(0xffbf5c),
        dim: rgb(0xb07624),
        faint: rgb(0x7a4e10),
        down: rgb(0xffad33),
        up: rgb(0xffe3ad),
        hot: rgb(0xfff4dc),
        warn: rgb(0xfff07a),
        bad: rgb(0xff7a59),
        cell_off: rgb(0x241503),
    };

    /// Vector display: cyan and magenta on deep blue.
    pub const VECTOR: Self = Self {
        name: "vector",
        bg: rgb(0x04020d),
        raise: rgb(0x0d0a2a),
        line: rgb(0x2c2870),
        fg: rgb(0xd9e3ff),
        dim: rgb(0x7f89c9),
        faint: rgb(0x454a90),
        down: rgb(0x3ef3ff),
        up: rgb(0xff52d9),
        hot: rgb(0xf4ff4f),
        warn: rgb(0xf4ff4f),
        bad: rgb(0xff5a3d),
        cell_off: rgb(0x14113d),
    };

    /// Beige: dark text on a light terminal-case background.
    pub const BEIGE: Self = Self {
        name: "beige",
        bg: rgb(0xdcd5c1),
        raise: rgb(0xebe5d4),
        line: rgb(0xa99f84),
        fg: rgb(0x23201a),
        dim: rgb(0x5f5948),
        faint: rgb(0x918870),
        down: rgb(0x22529e),
        up: rgb(0x2e7a4c),
        hot: rgb(0xb56a12),
        warn: rgb(0x8a6a00),
        bad: rgb(0xa52a17),
        cell_off: rgb(0xcbc3ab),
    };

    /// Every palette, the default first.
    pub const ALL: [Self; 4] = [Self::GREEN, Self::AMBER, Self::VECTOR, Self::BEIGE];

    /// The palette named `name`.
    pub fn named(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|theme| theme.name == name)
    }

    /// The palette after this one, wrapping around.
    pub fn next(self) -> Self {
        let index = Self::ALL
            .iter()
            .position(|theme| *theme == self)
            .unwrap_or(0);
        Self::ALL[(index + 1) % Self::ALL.len()]
    }

    /// `a` blended toward `b` by `t`, from 0 (all `a`) to 1 (all `b`).
    pub fn mix(a: Color, b: Color, t: f32) -> Color {
        match (a, b) {
            (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) => {
                let t = t.clamp(0.0, 1.0);
                let blend = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t) as u8;
                Color::Rgb(blend(r1, r2), blend(g1, g2), blend(b1, b2))
            }
            _ => a,
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::GREEN
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The relative luminance of an sRGB color, as WCAG defines it.
    fn luminance(color: Color) -> f64 {
        let Color::Rgb(r, g, b) = color else {
            panic!("palettes are truecolor")
        };
        let channel = |c: u8| {
            let c = f64::from(c) / 255.0;
            if c <= 0.039_28 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b)
    }

    fn contrast(a: Color, b: Color) -> f64 {
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    #[test]
    fn text_keeps_a_contrast_of_at_least_4_5() {
        for theme in Theme::ALL {
            for (role, color) in [("fg", theme.fg), ("dim", theme.dim)] {
                for (surface, bg) in [("bg", theme.bg), ("raise", theme.raise)] {
                    let ratio = contrast(color, bg);
                    assert!(
                        ratio >= 4.5,
                        "{} {role} on {surface}: {ratio:.2}",
                        theme.name
                    );
                }
            }
        }
    }

    #[test]
    fn themes_cycle_through_all_four() {
        let mut theme = Theme::default();
        assert_eq!(theme, Theme::GREEN);
        for expected in [Theme::AMBER, Theme::VECTOR, Theme::BEIGE, Theme::GREEN] {
            theme = theme.next();
            assert_eq!(theme, expected);
        }
        assert_eq!(Theme::named("vector"), Some(Theme::VECTOR));
        assert_eq!(Theme::named("pink"), None);
    }
}
