#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub bg: u32,
    pub fg: u32,
    pub muted: u32,
    pub accent: u32,
}

pub struct Theme {
    pub id: &'static str,
    pub name: &'static str,
    pub kind: &'static str,
    pub palette: Option<Palette>,
}

const fn theme(
    id: &'static str,
    name: &'static str,
    kind: &'static str,
    colors: [u32; 4],
) -> Theme {
    let [bg, fg, muted, accent] = colors;
    Theme {
        id,
        name,
        kind,
        palette: Some(Palette {
            bg,
            fg,
            muted,
            accent,
        }),
    }
}

const DARK: &str = "Тёмная";
const LIGHT: &str = "Светлая";

// Reading palettes inspired by editor themes; muted and accent colors are tuned for text UI.
pub const THEMES: &[Theme] = &[
    Theme {
        id: "terminal",
        name: "Как в терминале",
        kind: "Ghostty / Kitty",
        palette: None,
    },
    theme(
        "one-dark-pro",
        "One Dark Pro",
        DARK,
        [0x282c34, 0xabb2bf, 0x919bab, 0x61afef],
    ),
    theme(
        "dracula",
        "Dracula",
        DARK,
        [0x282a36, 0xe6e6df, 0xa7a8ba, 0xbd93f9],
    ),
    theme(
        "tokyo-night",
        "Tokyo Night",
        DARK,
        [0x1a1b26, 0xc0caf5, 0x9099b7, 0x7aa2f7],
    ),
    theme(
        "catppuccin-mocha",
        "Catppuccin Mocha",
        DARK,
        [0x1e1e2e, 0xcdd6f4, 0xa6adc8, 0x89b4fa],
    ),
    theme(
        "nord",
        "Nord",
        DARK,
        [0x2e3440, 0xd8dee9, 0xa8b2c3, 0x88c0d0],
    ),
    theme(
        "github-light",
        "GitHub Light",
        LIGHT,
        [0xf6f8fa, 0x24292f, 0x57606a, 0x0969da],
    ),
    theme(
        "solarized-light",
        "Solarized Light",
        LIGHT,
        [0xfdf6e3, 0x435960, 0x536970, 0x176399],
    ),
    theme(
        "gruvbox-light",
        "Gruvbox Light",
        LIGHT,
        [0xfbf1c7, 0x3c3836, 0x65594f, 0x79520e],
    ),
    theme(
        "catppuccin-latte",
        "Catppuccin Latte",
        LIGHT,
        [0xeff1f5, 0x4c4f69, 0x5c5f77, 0x6940aa],
    ),
    theme(
        "atom-one-light",
        "Atom One Light",
        LIGHT,
        [0xfafafa, 0x383a42, 0x5e606c, 0x326ab3],
    ),
];

pub fn index(id: &str) -> usize {
    THEMES.iter().position(|theme| theme.id == id).unwrap_or(0)
}

pub fn default_id() -> String {
    THEMES[0].id.into()
}

pub fn rgb(color: u32) -> [u8; 3] {
    let [_, r, g, b] = color.to_be_bytes();
    [r, g, b]
}

pub fn sgr(which: u8, color: u32) -> String {
    let [r, g, b] = rgb(color);
    format!("{which};2;{r};{g};{b}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contrast(a: u32, b: u32) -> f64 {
        fn luminance(color: u32) -> f64 {
            rgb(color)
                .into_iter()
                .zip([0.2126, 0.7152, 0.0722])
                .map(|(c, weight)| {
                    let x = f64::from(c) / 255.0;
                    weight
                        * if x <= 0.04045 {
                            x / 12.92
                        } else {
                            ((x + 0.055) / 1.055).powf(2.4)
                        }
                })
                .sum()
        }
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    #[test]
    fn presets_have_distinct_ids_and_readable_colors() {
        assert_eq!(THEMES.len(), 11);
        assert_eq!(index("unknown"), 0);
        for (i, theme) in THEMES.iter().enumerate() {
            assert_eq!(index(theme.id), i);
            if let Some(p) = theme.palette {
                assert!(contrast(p.fg, p.bg) >= 6.0, "{} body", theme.name);
                for color in [p.muted, p.accent] {
                    assert!(contrast(color, p.bg) >= 4.5, "{} controls", theme.name);
                }
            }
        }
    }
}
