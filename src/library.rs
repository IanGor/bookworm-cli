use crate::storage::{Database, content_id, parse_utc};
use anyhow::{Result, ensure};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};
use walkdir::WalkDir;

pub struct Entry {
    pub id: String,
    pub path: PathBuf,
}
#[derive(Default)]
pub struct Scan {
    pub entries: Vec<Entry>,
    pub warnings: Vec<String>,
}

pub fn completion_month(utc: &str) -> Option<String> {
    let (secs, _) = parse_utc(utc)?;
    let t = secs as libc::time_t;
    let mut local = std::mem::MaybeUninit::uninit();
    if unsafe { libc::localtime_r(&t, local.as_mut_ptr()) }.is_null() {
        return None;
    }
    let local = unsafe { local.assume_init() };
    const MONTHS: [&str; 12] = [
        "Январь",
        "Февраль",
        "Март",
        "Апрель",
        "Май",
        "Июнь",
        "Июль",
        "Август",
        "Сентябрь",
        "Октябрь",
        "Ноябрь",
        "Декабрь",
    ];
    let month = MONTHS.get(local.tm_mon as usize)?;
    Some(format!("{month} {}", local.tm_year + 1900))
}

impl Scan {
    pub fn sort(&mut self, db: &Database) {
        self.entries.sort_by(|a, b| {
            let ar = &db.books[&a.id];
            let br = &db.books[&b.id];
            let ac = ar.completed_at().and_then(parse_utc);
            let bc = br.completed_at().and_then(parse_utc);
            ac.is_some()
                .cmp(&bc.is_some())
                .then_with(|| match (ac, bc) {
                    (Some(at), Some(bt)) => bt.cmp(&at),
                    _ => br
                        .last_read
                        .as_deref()
                        .and_then(parse_utc)
                        .cmp(&ar.last_read.as_deref().and_then(parse_utc)),
                })
                .then_with(|| ar.title().to_lowercase().cmp(&br.title().to_lowercase()))
                .then_with(|| a.path.cmp(&b.path))
        });
    }
}
pub fn metadata(db: &mut Database, id: &str, path: &Path, doc: &bookworm_core::model::Document) {
    let record = db.books.entry(id.into()).or_default();
    record.source_title = if doc.title.trim().is_empty() {
        path.file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
    } else {
        doc.title.clone()
    };
    record.source_author = doc.authors.join(", ");
}
pub fn scan(root: &Path, db: &mut Database) -> Result<Scan> {
    ensure!(root.is_dir(), "Папка недоступна: {}", root.display());
    let mut out = Scan::default();
    let mut seen = HashSet::new();
    for entry in WalkDir::new(root).sort_by_file_name() {
        let e = match entry {
            Ok(e) => e,
            Err(e) => {
                out.warnings.push(e.to_string());
                continue;
            }
        };
        let path = e.path();
        if !e.file_type().is_file() {
            continue;
        }
        let name = e.file_name().to_string_lossy().to_lowercase();
        if ![".fb2", ".epub", ".fb2.zip", ".zip"]
            .iter()
            .any(|ext| name.ends_with(ext))
        {
            continue;
        }
        let result = (|| -> Result<()> {
            let bytes = fs::read(path)?;
            let id = content_id(&bytes);
            if seen.contains(&id) {
                return Ok(());
            }
            if !db.books.contains_key(&id) {
                let doc = bookworm_core::book::parse(&bytes).map_err(anyhow::Error::msg)?;
                metadata(db, &id, path, &doc);
            }
            seen.insert(id.clone());
            out.entries.push(Entry {
                id,
                path: path.into(),
            });
            Ok(())
        })();
        if let Err(e) = result {
            out.warnings.push(format!("{} — {e}", path.display()));
        }
    }
    out.sort(db);
    Ok(out)
}
