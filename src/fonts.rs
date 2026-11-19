use anyhow::{Context, Result};
use bookworm_core::font::{self, Face, Family, FontSet};
use std::{path::Path, sync::OnceLock};

pub const BUILTIN: [&str; 4] = ["PT Serif", "PT Sans", "Charis SIL", "Literata"];
macro_rules! font_path {
    ($file:expr) => {
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../BookwormEngine/fonts/",
            $file
        )
    };
}
pub const LICENSES: &str = concat!(
    include_str!(font_path!("OFL-ptserif.txt")),
    "\n",
    include_str!(font_path!("OFL-charissil.txt")),
    "\n",
    include_str!(font_path!("OFL-literata.txt")),
    "\n",
    include_str!(font_path!("OFL-liberation.txt"))
);

// The engine owns face bytes for the life of the process. Initialize each embedded
// face once, then copy its lightweight parser handles on subsequent layouts.
fn face_copy(src: &Face) -> Face {
    Face {
        rb: src.rb.clone(),
        ttf: src.ttf.clone(),
        upem: src.upem,
        ascender: src.ascender,
        descender: src.descender,
        x_height: src.x_height,
    }
}
macro_rules! embedded {
    ($file:expr)=>{{
        static FACE:OnceLock<Face>=OnceLock::new();
        face_copy(FACE.get_or_init(|| Face::from_bytes(include_bytes!(font_path!($file)).to_vec()).expect("встроенный шрифт")))
    }}
}
macro_rules! static_family {
    ($prefix:literal) => {
        vec![
            embedded!(concat!($prefix, "-Regular.ttf")),
            embedded!(concat!($prefix, "-Italic.ttf")),
            embedded!(concat!($prefix, "-Bold.ttf")),
            embedded!(concat!($prefix, "-BoldItalic.ttf")),
        ]
    };
}
fn weight(mut f: Face, value: f32) -> Face {
    let tag = rustybuzz::ttf_parser::Tag::from_bytes(b"wght");
    f.ttf.set_variation(tag, value);
    f.rb.set_variations(&[rustybuzz::Variation { tag, value }]);
    f
}
pub fn discover(dir: &Path) -> Result<Vec<Family>> {
    let mut paths = Vec::new();
    for item in dir
        .read_dir()
        .with_context(|| format!("Папка шрифтов {}", dir.display()))?
    {
        let p = item?.path();
        if p.is_file()
            && p.extension()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s.eq_ignore_ascii_case("ttf") || s.eq_ignore_ascii_case("otf"))
            && let Some(p) = p.to_str()
        {
            paths.push(p.to_owned());
        }
    }
    paths.sort();
    let mut families = font::families(&paths);
    families.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(families)
}
pub fn load(name: &str, custom: &[Family]) -> Result<FontSet> {
    let mut faces = match name {
        "PT Serif" => static_family!("PTSerif"),
        "PT Sans" => static_family!("PTSans"),
        "Charis SIL" => static_family!("CharisSIL"),
        "Literata" => {
            let r = embedded!("Literata.ttf");
            let i = embedded!("Literata-Italic.ttf");
            vec![
                weight(face_copy(&r), 400.),
                weight(face_copy(&i), 400.),
                weight(r, 700.),
                weight(i, 700.),
            ]
        }
        _ => {
            let f = custom
                .iter()
                .find(|f| Some(f.name.as_str()) == name.strip_prefix("custom:"))
                .context("Выбранный шрифт недоступен")?;
            f.faces
                .iter()
                .map(|s| Face::from_file_weight(&s.path, s.wght).map_err(anyhow::Error::msg))
                .collect::<Result<Vec<_>>>()?
        }
    };
    faces.push(embedded!("LiberationMono-Regular.ttf"));
    Ok(FontSet::new(faces))
}
