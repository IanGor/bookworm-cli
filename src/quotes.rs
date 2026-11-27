use crate::storage::{Anchor, Snapshot, data_path, parse_utc, publish, snapshot};
use anyhow::{Context, Result, ensure};
use bookworm_core::model::Document;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const CONFLICT: &str = "Файл цитат изменён извне. Сохранение остановлено.";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Quote {
    pub start: Anchor,
    pub end: Anchor,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub chapter: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct QuoteFile {
    version: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    quotes: Vec<Quote>,
}

impl Default for QuoteFile {
    fn default() -> Self {
        Self {
            version: 1,
            quotes: Vec::new(),
        }
    }
}

impl QuoteFile {
    fn validate(&self) -> Result<()> {
        ensure!(self.version == 1, "Неизвестная версия файла цитат");
        for q in &self.quotes {
            ensure!(q.start < q.end, "Некорректные границы цитаты");
            ensure!(!q.text.trim().is_empty(), "Пустой текст цитаты");
            ensure!(
                parse_utc(&q.created_at).is_some(),
                "Нужна дата и время UTC с суффиксом Z"
            );
        }
        Ok(())
    }
}

fn sort_quotes(quotes: &mut [Quote]) {
    quotes.sort_by(|a, b| {
        (a.start, a.end, parse_utc(&a.created_at), &a.created_at).cmp(&(
            b.start,
            b.end,
            parse_utc(&b.created_at),
            &b.created_at,
        ))
    });
}

pub fn quotes_path(book_id: &str) -> Result<PathBuf> {
    ensure!(
        book_id.len() == 64
            && book_id
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
        "Неверный SHA-256 книги"
    );
    Ok(data_path()?
        .with_file_name("quotes")
        .join(format!("{book_id}.toml")))
}

pub struct QuoteStore {
    pub path: PathBuf,
    pub quotes: Vec<Quote>,
    original: Option<Snapshot>,
}

impl QuoteStore {
    pub fn open(path: &Path) -> Result<Self> {
        let original = snapshot(path).with_context(|| format!("Чтение {}", path.display()))?;
        let mut file: QuoteFile =
            match &original {
                Some(s) => toml::from_str(std::str::from_utf8(&s.bytes).with_context(|| {
                    format!("Файл цитат {} должен быть в UTF-8", path.display())
                })?)
                .with_context(|| format!("Некорректный файл цитат {}", path.display()))?,
                None => QuoteFile::default(),
            };
        file.validate()
            .with_context(|| format!("Некорректный файл цитат {}", path.display()))?;
        sort_quotes(&mut file.quotes);
        Ok(Self {
            path: path.to_owned(),
            quotes: file.quotes,
            original,
        })
    }

    pub fn add(&mut self, quote: Quote) {
        self.quotes.push(quote);
        sort_quotes(&mut self.quotes);
    }

    pub fn save(&mut self) -> Result<()> {
        sort_quotes(&mut self.quotes);
        let file = QuoteFile {
            version: 1,
            quotes: self.quotes.clone(),
        };
        file.validate()?;
        let bytes = toml::to_string_pretty(&file)?.into_bytes();
        publish(&self.path, &mut self.original, bytes, Some(0o700), CONFLICT)
    }
}

pub fn extract_quote(doc: &Document, start: Anchor, end: Anchor) -> (String, String) {
    let chapter = doc
        .toc
        .iter()
        .rfind(|entry| entry.block <= start.block as usize)
        .map_or_else(String::new, |entry| entry.title.clone());
    if doc.blocks.is_empty() || start >= end {
        return (chapter, String::new());
    }
    let last = (end.block as usize).min(doc.blocks.len() - 1);
    let mut parts = Vec::new();
    for block in (start.block as usize)..=last {
        let from = if block == start.block as usize {
            start.offset as usize
        } else {
            0
        };
        let to = if block == end.block as usize {
            end.offset as usize
        } else {
            usize::MAX
        };
        if to <= from {
            continue;
        }
        let part: String = doc.blocks[block]
            .spans
            .iter()
            .flat_map(|s| s.text.chars())
            .skip(from)
            .take(to - from)
            .collect();
        if !part.is_empty() {
            parts.push(part);
        }
    }
    (chapter, parts.join("\n\n"))
}
