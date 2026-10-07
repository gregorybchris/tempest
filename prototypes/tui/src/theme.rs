//! The four Tempest palettes, as terminal truecolor.

use ratatui::style::Color;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Theme {
    pub name: &'static str,
    pub bg: Color,
    pub raise: Color,
    pub line: Color,
    pub fg: Color,
    pub dim: Color,
    pub faint: Color,
    pub down: Color,
    pub up: Color,
    pub hot: Color,
    pub warn: Color,
    pub bad: Color,
    pub cell_off: Color,
}

const fn rgb(hex: u32) -> Color {
    Color::Rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

impl Theme {
    pub const GREEN: Theme = Theme {
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

    pub const AMBER: Theme = Theme {
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

    pub const VECTOR: Theme = Theme {
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

    pub const BEIGE: Theme = Theme {
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

    pub const ALL: [Theme; 4] = [Theme::GREEN, Theme::AMBER, Theme::VECTOR, Theme::BEIGE];

    pub fn next(self) -> Theme {
        let i = Self::ALL.iter().position(|t| *t == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }

    /// Blends `a` toward `b` by `t` (0 = a, 1 = b).
    pub fn mix(a: Color, b: Color, t: f32) -> Color {
        match (a, b) {
            (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) => {
                let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t.clamp(0.0, 1.0)) as u8;
                Color::Rgb(m(r1, r2), m(g1, g2), m(b1, b2))
            }
            _ => a,
        }
    }
}
