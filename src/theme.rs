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
pub const THEMES: &[Theme] = &[
    Theme {
        id: "terminal",
        name: "Как в терминале",
        kind: "Ghostty / Kitty",
        palette: None,
    },
    theme(
        "amber",
        "Янтарь",
        DARK,
        [0x211c19, 0xbda98e, 0xa18f7b, 0xefb45d],
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
        assert_eq!(
            THEMES.iter().map(|t| t.id).collect::<Vec<_>>(),
            ["terminal", "amber"]
        );
        assert_eq!(index("one-dark-pro"), 0);
        assert_eq!(index("lamp"), 0);
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
