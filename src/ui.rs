use crate::terminal::{Key, Terminal, cell_width};
use anyhow::{Context, Result, ensure};
use bookworm::{
    quotes::Quote,
    theme::{self, THEMES},
};
use std::{
    env,
    path::{Path, PathBuf},
};

pub const TITLE: &str = "1";
pub const MUTED: &str = "2";
pub const SELECTED: &str = "7";
pub const NORMAL: &str = "0";

pub fn wrap_lines(text: &str, cols: u32) -> Vec<String> {
    let cols = cols.max(1);
    let mut out = Vec::new();
    for raw in text.split('\n') {
        if raw.is_empty() {
            out.push(String::new());
            continue;
        }
        let mut cur = String::new();
        let mut cur_w = 0;
        for (wi, word) in raw.split(' ').enumerate() {
            let clean: String = word
                .chars()
                .map(|c| if c.is_control() { ' ' } else { c })
                .collect();
            let word_w: u32 = clean.chars().map(cell_width).sum();
            let sep_w = u32::from(wi > 0 && !cur.is_empty());
            if cur_w + sep_w + word_w <= cols {
                if sep_w > 0 {
                    cur.push(' ');
                    cur_w += 1;
                }
                cur.push_str(&clean);
                cur_w += word_w;
            } else if word_w <= cols {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                cur = clean;
                cur_w = word_w;
            } else {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                    cur_w = 0;
                }
                for ch in clean.chars() {
                    let cw = cell_width(ch);
                    if !cur.is_empty() && cur_w + cw > cols {
                        out.push(std::mem::take(&mut cur));
                        cur_w = 0;
                    }
                    cur.push(ch);
                    cur_w += cw;
                }
            }
        }
        out.push(cur);
    }
    out
}

pub fn confirm(t: &mut Terminal, heading: &str, info: &str) -> Result<bool> {
    loop {
        t.begin_frame()?;
        title(t, heading)?;
        t.line(3, &format!("  {info}"), NORMAL)?;
        t.line(5, "  Y / Д Подтвердить   Esc Отмена", TITLE)?;
        t.end_frame()?;
        match t.key()?.command() {
            Key::Resize => continue,
            Key::Exit => return Err(crate::Quit.into()),
            Key::Char('y' | 'д' | 'Д') => return Ok(true),
            _ => return Ok(false),
        }
    }
}

pub fn quote_comment(
    t: &mut Terminal,
    heading: &str,
    preview: &str,
    initial: Option<&str>,
) -> Result<Option<Option<String>>> {
    let mut input = initial.unwrap_or_default().to_owned();
    loop {
        t.begin_frame()?;
        title(t, heading)?;
        let width = t.window.cols.saturating_sub(6).max(1);
        let preview_lines = wrap_lines(preview, width);
        for (i, line) in preview_lines.iter().take(2).enumerate() {
            let ellipsis = if i == 1 && preview_lines.len() > 2 {
                "…"
            } else {
                ""
            };
            t.line(2 + i as u32, &format!("  │ {line}{ellipsis}"), MUTED)?;
        }
        t.line(
            4,
            "  Комментарий (Enter без текста сохранит чистую цитату):",
            NORMAL,
        )?;
        let edit_width = t.window.cols.saturating_sub(4).max(1);
        let edit_rows = t.window.rows.saturating_sub(6).max(1) as usize;
        let wrapped = wrap_lines(&format!("{input}▏"), edit_width);
        for (row, line) in wrapped[wrapped.len().saturating_sub(edit_rows)..]
            .iter()
            .enumerate()
        {
            t.line(6 + row as u32, &format!("  {line}"), TITLE)?;
        }
        t.line(
            t.window.rows,
            "  Enter Сохранить   Backspace Стереть   Esc Отмена",
            MUTED,
        )?;
        t.end_frame()?;
        match t.key()? {
            Key::Enter => {
                let trimmed = input.trim();
                return Ok(Some(if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_owned())
                }));
            }
            Key::Esc => return Ok(None),
            Key::Exit => return Err(crate::Quit.into()),
            Key::Backspace => {
                input.pop();
            }
            Key::Char(c) if input.len() + c.len_utf8() <= 8192 => input.push(c),
            _ => (),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum QuoteAction {
    Back,
    Jump(usize),
    Edit(usize),
    Delete(usize),
}

#[derive(Clone, Copy)]
enum FeedKind {
    Chapter,
    Body,
    Comment,
    Gap,
}

struct FeedRow {
    quote: usize,
    kind: FeedKind,
    text: String,
}

pub(crate) struct QuoteFeed {
    rows: Vec<FeedRow>,
    spans: Vec<(usize, usize)>,
}

pub(crate) fn build_quote_feed(quotes: &[Quote], cols: u32) -> QuoteFeed {
    let text_cols = cols.saturating_sub(6).max(1);
    let mut rows = Vec::new();
    let mut spans = Vec::with_capacity(quotes.len());
    for (idx, q) in quotes.iter().enumerate() {
        let start = rows.len();
        if !q.chapter.is_empty() {
            for line in wrap_lines(&q.chapter, text_cols) {
                rows.push(FeedRow {
                    quote: idx,
                    kind: FeedKind::Chapter,
                    text: line,
                });
            }
        }
        for line in wrap_lines(&q.text, text_cols) {
            rows.push(FeedRow {
                quote: idx,
                kind: FeedKind::Body,
                text: line,
            });
        }
        if let Some(comment) = &q.comment {
            let comment_cols = text_cols.saturating_sub(2).max(1);
            for (ci, line) in wrap_lines(comment, comment_cols).into_iter().enumerate() {
                let prefix = if ci == 0 { "— " } else { "  " };
                rows.push(FeedRow {
                    quote: idx,
                    kind: FeedKind::Comment,
                    text: format!("{prefix}{line}"),
                });
            }
        }
        let end = rows.len();
        spans.push((start, end));
        if idx + 1 < quotes.len() {
            rows.push(FeedRow {
                quote: idx,
                kind: FeedKind::Gap,
                text: String::new(),
            });
        }
    }
    QuoteFeed { rows, spans }
}

pub(crate) fn step_quote(
    feed: &QuoteFeed,
    viewport: usize,
    forward: bool,
    selected: &mut usize,
    scroll: &mut usize,
) {
    if feed.spans.is_empty() {
        return;
    }
    let viewport = viewport.max(1);
    *selected = (*selected).min(feed.spans.len() - 1);
    let (start, end) = feed.spans[*selected];
    let step = 3.min(viewport);
    if forward {
        if end > *scroll + viewport {
            *scroll = (*scroll + step).min(end.saturating_sub(viewport));
        } else if *selected + 1 < feed.spans.len() {
            *selected += 1;
            let (ns, ne) = feed.spans[*selected];
            if ne - ns > viewport || ns < *scroll {
                *scroll = ns;
            } else if ne > *scroll + viewport {
                *scroll = ne - viewport;
            }
        }
    } else if start < *scroll {
        *scroll = scroll.saturating_sub(step).max(start);
    } else if *selected > 0 {
        *selected -= 1;
        let (ps, pe) = feed.spans[*selected];
        if pe - ps > viewport || pe > *scroll + viewport {
            *scroll = pe.saturating_sub(viewport);
        } else if ps < *scroll {
            *scroll = ps;
        }
    }
}

pub fn quotes_view(
    t: &mut Terminal,
    book_title: &str,
    author: &str,
    quotes: &[Quote],
    selected: &mut usize,
    scroll: &mut usize,
) -> Result<QuoteAction> {
    loop {
        t.begin_frame()?;
        title(t, &format!("Цитаты · {book_title}"))?;
        let author_label = if author.is_empty() {
            "Автор неизвестен"
        } else {
            author
        };
        if quotes.is_empty() {
            t.line(2, &format!("  Цитат — 0   ·   {author_label}"), MUTED)?;
            t.line(
                4,
                "  Цитат пока нет. В книге нажмите D, чтобы выделить фрагмент.",
                NORMAL,
            )?;
            t.line(t.window.rows, "  Esc Назад", MUTED)?;
            t.end_frame()?;
            match t.key()?.command() {
                Key::Resize => continue,
                Key::Exit => return Err(crate::Quit.into()),
                Key::Esc | Key::Char('q' | 'c') => return Ok(QuoteAction::Back),
                _ => continue,
            }
        }
        *selected = (*selected).min(quotes.len() - 1);
        t.line(
            2,
            &format!(
                "  Цитата {} из {}   ·   {author_label}",
                *selected + 1,
                quotes.len()
            ),
            MUTED,
        )?;
        let viewport = t.window.rows.saturating_sub(5).max(1) as usize;
        let feed = build_quote_feed(quotes, t.window.cols);
        let max_scroll = feed.rows.len().saturating_sub(viewport);
        *scroll = (*scroll).min(max_scroll);
        let (s_start, s_end) = feed.spans[*selected];
        if s_end <= *scroll {
            *scroll = s_start.min(max_scroll);
        } else if s_start >= *scroll + viewport {
            *scroll = if s_end - s_start > viewport {
                s_start
            } else {
                s_end.saturating_sub(viewport)
            }
            .min(max_scroll);
        }
        for (offset, row) in feed.rows.iter().skip(*scroll).take(viewport).enumerate() {
            let y = 4 + offset as u32;
            let active = row.quote == *selected;
            match row.kind {
                FeedKind::Gap => (),
                FeedKind::Chapter => {
                    let style = if active { TITLE } else { MUTED };
                    t.line(y, &format!("    {}", row.text), style)?;
                }
                FeedKind::Body => {
                    let (bar, style) = if active {
                        ("▌", TITLE)
                    } else {
                        ("│", NORMAL)
                    };
                    t.line(y, &format!("  {bar} {}", row.text), style)?;
                }
                FeedKind::Comment => {
                    let style = if active { "1;36" } else { "36" };
                    t.line(y, &format!("    {}", row.text), style)?;
                }
            }
        }
        t.line(
            t.window.rows.saturating_sub(1),
            "  ↑↓ / PgUp/PgDn Листать   Enter К тексту   E Комментарий   X Удалить",
            MUTED,
        )?;
        t.line(t.window.rows, "  Esc Назад", MUTED)?;
        t.end_frame()?;
        match t.key()?.command() {
            Key::Down | Key::Char('j') => step_quote(&feed, viewport, true, selected, scroll),
            Key::Up | Key::Char('k') => step_quote(&feed, viewport, false, selected, scroll),
            Key::PageDown => {
                *scroll = (*scroll + viewport).min(max_scroll);
                if feed.spans[*selected].1 <= *scroll
                    && let Some(i) = feed.spans.iter().position(|&(_, e)| e > *scroll)
                {
                    *selected = i;
                }
            }
            Key::PageUp => {
                *scroll = scroll.saturating_sub(viewport);
                if feed.spans[*selected].0 >= *scroll + viewport
                    && let Some(i) = feed
                        .spans
                        .iter()
                        .rposition(|&(s, _)| s < *scroll + viewport)
                {
                    *selected = i;
                }
            }
            Key::Home => {
                *selected = 0;
                *scroll = 0;
            }
            Key::End => {
                *selected = quotes.len() - 1;
                let (s, e) = feed.spans[*selected];
                *scroll = if e - s > viewport {
                    s
                } else {
                    e.saturating_sub(viewport)
                }
                .min(max_scroll);
            }
            Key::Enter => return Ok(QuoteAction::Jump(*selected)),
            Key::Char('e') => return Ok(QuoteAction::Edit(*selected)),
            Key::Char('x') => return Ok(QuoteAction::Delete(*selected)),
            Key::Esc | Key::Char('q' | 'c') => return Ok(QuoteAction::Back),
            Key::Exit => return Err(crate::Quit.into()),
            _ => (),
        }
    }
}

pub fn report_description(
    t: &mut Terminal,
    range: crate::selection::Range,
) -> Result<Option<String>> {
    let mut input = String::new();
    loop {
        t.begin_frame()?;
        title(t, "Дефект вёрстки")?;
        t.line(
            2,
            &format!(
                "  Стр. {}:{}:{} → {}:{}:{} (страница:блок:смещение)",
                range.start.page + 1,
                range.start.block,
                range.start.offset,
                range.end.page + 1,
                range.end.block,
                range.end.offset
            ),
            MUTED,
        )?;
        t.line(3, "  Что не так и как должно выглядеть?", NORMAL)?;
        let width = t.window.cols.saturating_sub(4).max(1) as usize;
        let rows = t.window.rows.saturating_sub(5).max(1) as usize;
        let tail: Vec<_> = input.chars().chain(['▏']).collect();
        for (row, part) in tail[tail.len().saturating_sub(width * rows)..]
            .chunks(width)
            .enumerate()
        {
            t.line(
                row as u32 + 5,
                &format!("  {}", part.iter().collect::<String>()),
                TITLE,
            )?;
        }
        t.line(t.window.rows, "  Enter Сохранить   Esc Отмена", MUTED)?;
        t.end_frame()?;
        match t.key()? {
            Key::Enter if !input.trim().is_empty() => return Ok(Some(input.trim().to_owned())),
            Key::Esc => return Ok(None),
            Key::Exit => return Err(crate::Quit.into()),
            Key::Backspace => {
                input.pop();
            }
            Key::Char(c) if input.len() + c.len_utf8() <= 4096 => input.push(c),
            _ => (),
        }
    }
}

pub fn report_saved(t: &mut Terminal, path: &Path) -> Result<()> {
    let command = crate::report::command(path);
    let mut status = String::new();
    loop {
        t.begin_frame()?;
        title(t, "Промпт сохранён")?;
        t.line(2, "  Запусти команду в другом терминале:", MUTED)?;
        let text = match &command {
            Ok(cmd) => cmd.clone(),
            Err(_) => path.to_string_lossy().into_owned(),
        };
        let width = t.window.cols.saturating_sub(4).max(1) as usize;
        let chars: Vec<_> = text.chars().collect();
        for (i, row) in chars
            .chunks(width)
            .take(t.window.rows.saturating_sub(5) as usize)
            .enumerate()
        {
            t.line(
                i as u32 + 4,
                &format!("  {}", row.iter().collect::<String>()),
                NORMAL,
            )?;
        }
        t.line(t.window.rows.saturating_sub(1), &status, TITLE)?;
        t.line(t.window.rows, "  C Копировать команду   Enter Назад", MUTED)?;
        t.end_frame()?;
        match t.key()?.command() {
            Key::Char('c') => {
                status = match &command {
                    Ok(cmd) if t.copy_to_clipboard(cmd).is_ok() => {
                        "  Команда отправлена в буфер. В другом терминале: Ctrl+Shift+V.".into()
                    }
                    _ => "  Не удалось скопировать. Промпт сохранён на диске.".into(),
                }
            }
            Key::Enter | Key::Esc => return Ok(()),
            Key::Exit => return Err(crate::Quit.into()),
            _ => (),
        }
    }
}

pub fn theme_picker(t: &mut Terminal, current: usize) -> Result<Option<usize>> {
    let mut selected = current;
    let mut first = 0;
    loop {
        t.begin_frame()?;
        t.clear()?;
        let width = t.window.cols.min(52);
        let col = 1 + (t.window.cols - width) / 2;
        let visible = t
            .window
            .rows
            .saturating_sub(5)
            .clamp(1, THEMES.len() as u32) as usize;
        let top = 1 + t.window.rows.saturating_sub(visible as u32 + 5) / 2;
        first = first
            .min(selected)
            .max((selected + 1).saturating_sub(visible));
        first = first.min(THEMES.len() - visible);
        let edge = "─".repeat(width as usize);
        t.text(top, col, width, &edge, MUTED)?;
        t.text(top + 1, col, width, "Настройки · Цветовые темы", TITLE)?;
        t.text(
            top + 2,
            col,
            width,
            &format!("{} / {}", selected + 1, THEMES.len()),
            MUTED,
        )?;
        for (i, preset) in THEMES.iter().enumerate().skip(first).take(visible) {
            let row = top + 3 + (i - first) as u32;
            let pointer = if i == selected { '›' } else { ' ' };
            let active = if i == current { '✓' } else { ' ' };
            let style = if i == selected { SELECTED } else { NORMAL };
            let text = format!("{pointer} {active} {:<20} {}", preset.name, preset.kind);
            t.text(row, col, width, &text, style)?;
            if width >= 46
                && let Some(p) = preset.palette
            {
                let swatch = format!("{};{}", theme::sgr(48, p.bg), theme::sgr(38, p.fg));
                t.text(row, col + width - 5, 5, " Аа  ", &swatch)?;
            }
        }
        t.text(top + visible as u32 + 3, col, width, &edge, MUTED)?;
        let hint = if width >= 39 {
            "↑↓ Выбор   Enter Применить   Esc Отмена"
        } else {
            "↑↓   Enter   Esc Отмена"
        };
        t.text(
            (top + visible as u32 + 4).min(t.window.rows),
            col,
            width,
            hint,
            MUTED,
        )?;
        t.end_frame()?;
        match t.key()?.command() {
            Key::Down | Key::Char('j') => selected = (selected + 1).min(THEMES.len() - 1),
            Key::Up | Key::Char('k') => selected = selected.saturating_sub(1),
            Key::Home => selected = 0,
            Key::End => selected = THEMES.len() - 1,
            Key::Enter => return Ok(Some(selected)),
            Key::Exit => return Err(crate::Quit.into()),
            Key::Esc | Key::Char('y' | 'q') => return Ok(None),
            _ => (),
        }
    }
}

pub fn title(t: &mut Terminal, name: &str) -> Result<()> {
    t.clear()?;
    t.line(1, &format!("  {name}"), TITLE)
}
pub fn notice(t: &mut Terminal, heading: &str, message: &str) -> Result<Key> {
    loop {
        t.begin_frame()?;
        title(t, heading)?;
        let width = t.window.cols.saturating_sub(4).max(1) as usize;
        let chars: Vec<_> = message.chars().collect();
        for (i, part) in chars
            .chunks(width)
            .enumerate()
            .take(t.window.rows.saturating_sub(3) as usize)
        {
            t.line(
                3 + i as u32,
                &format!("  {}", part.iter().collect::<String>()),
                NORMAL,
            )?;
        }
        t.line(t.window.rows, "  Enter / Esc  Назад", MUTED)?;
        t.end_frame()?;
        match t.key()?.command() {
            Key::Resize => continue,
            k => return Ok(k),
        }
    }
}
#[derive(Default)]
pub struct List<'a> {
    pub title: &'a str,
    pub subtitle: &'a str,
    pub items: &'a [String],
    pub sections: &'a [(usize, String)],
    pub base_action: &'a str,
    pub item_action: Option<(&'a [bool], &'a str)>,
    pub empty: &'a str,
    pub footer: &'a str,
}

pub(crate) fn list_action_line(base: Option<&str>, extra: Option<&str>) -> String {
    match (base, extra) {
        (Some(b), Some(e)) => format!("  ↑↓ Выбор   Enter Открыть   {b}   {e}   Esc Назад"),
        (Some(b), None) => format!("  ↑↓ Выбор   PgUp/PgDn   Enter Открыть   {b}   Esc Назад"),
        (None, Some(e)) => format!("  ↑↓ Выбор   PgUp/PgDn   Enter Открыть   {e}   Esc Назад"),
        (None, None) => "  ↑↓ Выбор   PgUp/PgDn Листать   Enter Открыть   Esc Назад".into(),
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Row<'a> {
    Section(&'a str),
    Item(usize, &'a str),
}

fn layout_rows<'a>(
    items: &'a [String],
    sections: &'a [(usize, String)],
    selected: usize,
    visible: usize,
    first: &mut usize,
) -> Vec<Row<'a>> {
    let mut rows = Vec::with_capacity(items.len() + sections.len());
    let mut sel_row = 0;
    for (i, item) in items.iter().enumerate() {
        for (at, label) in sections {
            if *at == i {
                rows.push(Row::Section(label));
            }
        }
        if i == selected {
            sel_row = rows.len();
        }
        rows.push(Row::Item(i, item));
    }
    let anchor = if visible >= 2 && sel_row > 0 && matches!(rows[sel_row - 1], Row::Section(_)) {
        sel_row - 1
    } else {
        sel_row
    };
    *first = (*first)
        .min(anchor)
        .max((sel_row + 1).saturating_sub(visible))
        .min(rows.len().saturating_sub(visible));
    rows.into_iter().skip(*first).take(visible).collect()
}

pub fn list(t: &mut Terminal, list: List<'_>, selected: &mut usize) -> Result<Key> {
    let mut first = 0;
    loop {
        t.begin_frame()?;
        title(t, list.title)?;
        t.line(2, &format!("  {}", list.subtitle), MUTED)?;
        let count = list.items.len();
        let visible = t.window.rows.saturating_sub(5).max(1) as usize;
        *selected = (*selected).min(count.saturating_sub(1));
        if count == 0 {
            t.line(4, &format!("  {}", list.empty), NORMAL)?;
        }
        for (offset, row) in layout_rows(list.items, list.sections, *selected, visible, &mut first)
            .into_iter()
            .enumerate()
        {
            let y = (4 + offset) as u32;
            match row {
                Row::Section(label) => t.line(y, &format!("  {label}"), MUTED)?,
                Row::Item(i, item) => {
                    let (mark, style) = if i == *selected {
                        ("›", SELECTED)
                    } else {
                        (" ", NORMAL)
                    };
                    t.line(y, &format!(" {mark} {item}"), style)?;
                }
            }
        }
        let base = (!list.base_action.is_empty() && count > 0).then_some(list.base_action);
        let extra = list
            .item_action
            .and_then(|(flags, action)| (flags.get(*selected) == Some(&true)).then_some(action));
        let keys = list_action_line(base, extra);
        t.line(t.window.rows.saturating_sub(1), &keys, MUTED)?;
        t.line(t.window.rows, list.footer, MUTED)?;
        t.end_frame()?;
        match t.key()?.command() {
            Key::Down | Key::Char('j') => *selected = (*selected + 1).min(count.saturating_sub(1)),
            Key::Up | Key::Char('k') => *selected = selected.saturating_sub(1),
            Key::PageDown => *selected = (*selected + visible).min(count.saturating_sub(1)),
            Key::PageUp => *selected = selected.saturating_sub(visible),
            Key::Home => *selected = 0,
            Key::End => *selected = count.saturating_sub(1),
            Key::Resize => (),
            key => return Ok(key),
        }
    }
}
pub fn directory(t: &mut Terminal, heading: &str) -> Result<Option<PathBuf>> {
    let mut input = String::new();
    let mut message = String::new();
    loop {
        t.begin_frame()?;
        title(t, heading)?;
        t.line(
            3,
            "  Введите путь к папке. Для домашней папки можно использовать ~.",
            NORMAL,
        )?;
        // Keep the end of a long path visible as it is typed.
        let cap = t.window.cols.saturating_sub(6) as usize;
        let suffix: String = input
            .chars()
            .skip(input.chars().count().saturating_sub(cap))
            .collect();
        t.line(5, &format!("  › {suffix}▏"), TITLE)?;
        t.line(7, &message, MUTED)?;
        t.line(
            t.window.rows,
            "  Enter Выбрать   Backspace Стереть   Esc Отмена",
            MUTED,
        )?;
        t.end_frame()?;
        match t.key()? {
            Key::Esc => return Ok(None),
            Key::Exit => return Err(super::Quit.into()),
            Key::Backspace => {
                input.pop();
                message.clear();
            }
            Key::Char(c) => {
                input.push(c);
                message.clear();
            }
            Key::Enter => match resolve_directory(&input) {
                Ok(p) => return Ok(Some(p)),
                Err(e) => message = format!("  {e}"),
            },
            _ => (),
        }
    }
}
fn resolve_directory(input: &str) -> Result<PathBuf> {
    ensure!(!input.is_empty(), "Введите путь к папке");
    let path = Path::new(input);
    let path = if let Ok(rest) = path.strip_prefix("~") {
        PathBuf::from(env::var_os("HOME").context("Переменная HOME не задана")?).join(rest)
    } else {
        path.to_owned()
    };
    let path = path
        .canonicalize()
        .with_context(|| format!("Не удалось открыть {}", path.display()))?;
    ensure!(path.is_dir(), "Выберите папку");
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_rows_keeps_section_header_with_selected_book_and_caps_height() {
        let items: Vec<_> = ["Активная", "Неначатая", "Ноябрьская", "Октябрьская"]
            .into_iter()
            .map(String::from)
            .collect();
        let sections = [
            (2, "── Ноябрь 2026 ──".into()),
            (3, "── Октябрь 2026 ──".into()),
        ];
        let mut first = 0;

        // Moving down to item 2 in a 3-row viewport includes both the header and item 2.
        let slice = layout_rows(&items, &sections, 2, 3, &mut first);
        assert_eq!(
            slice,
            vec![
                Row::Item(1, "Неначатая"),
                Row::Section("── Ноябрь 2026 ──"),
                Row::Item(2, "Ноябрьская"),
            ]
        );

        // Moving down to item 3 advances the viewport without exceeding 3 rows.
        let slice = layout_rows(&items, &sections, 3, 3, &mut first);
        assert_eq!(
            slice,
            vec![
                Row::Item(2, "Ноябрьская"),
                Row::Section("── Октябрь 2026 ──"),
                Row::Item(3, "Октябрьская"),
            ]
        );

        // Moving back up to item 2 pulls its month header back into view above it.
        let slice = layout_rows(&items, &sections, 2, 2, &mut first);
        assert_eq!(
            slice,
            vec![
                Row::Section("── Ноябрь 2026 ──"),
                Row::Item(2, "Ноябрьская"),
            ]
        );

        // On a 1-row viewport, the selected book wins over its section header.
        let mut tiny_first = 0;
        assert_eq!(
            layout_rows(&items, &sections, 2, 1, &mut tiny_first),
            vec![Row::Item(2, "Ноябрьская")]
        );
    }

    #[test]
    fn wrap_lines_handles_wide_chars_long_words_and_paragraph_breaks() {
        unsafe { libc::setlocale(libc::LC_CTYPE, c"C.UTF-8".as_ptr()) };
        assert_eq!(
            wrap_lines("первый абзац\n\nвторой мир", 10),
            vec!["первый", "абзац", "", "второй мир"]
        );
        // Wide CJK glyphs take 2 columns each: 3 glyphs = 6 cols, 4th wraps.
        assert_eq!(wrap_lines("字字字字", 6), vec!["字字字", "字"]);
        // Unbroken token wider than viewport is split without losing characters.
        assert_eq!(wrap_lines("abcdefgh", 3), vec!["abc", "def", "gh"]);
    }

    #[test]
    fn tall_quote_scrolls_to_edges_before_switching_selection() {
        use bookworm::{quotes::Quote, storage::Anchor};
        let tall_text = (1..=10)
            .map(|i| format!("Строка {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let quotes = vec![
            Quote {
                start: Anchor {
                    block: 0,
                    offset: 0,
                },
                end: Anchor {
                    block: 0,
                    offset: 5,
                },
                chapter: "Глава 1".into(),
                text: tall_text,
                comment: Some("Мой комментарий".into()),
                created_at: "2026-11-27T01:00:00Z".into(),
            },
            Quote {
                start: Anchor {
                    block: 2,
                    offset: 0,
                },
                end: Anchor {
                    block: 2,
                    offset: 5,
                },
                chapter: String::new(),
                text: "Короткая вторая".into(),
                comment: None,
                created_at: "2026-11-27T01:01:00Z".into(),
            },
        ];
        let feed = build_quote_feed(&quotes, 40);
        let mut selected = 0;
        let mut scroll = 0;
        let viewport = 5;

        // Quote 0 spans > 5 rows: pressing Down scrolls inside Quote 0 before moving to Quote 1.
        step_quote(&feed, viewport, true, &mut selected, &mut scroll);
        assert_eq!(selected, 0);
        assert!(scroll > 0);
        let mut reached_bottom = false;
        while selected == 0 {
            if scroll + viewport >= feed.spans[0].1 {
                reached_bottom = true;
            }
            let prev = scroll;
            step_quote(&feed, viewport, true, &mut selected, &mut scroll);
            assert!(scroll > prev || selected == 1);
        }
        assert!(reached_bottom);
        assert_eq!(selected, 1);

        // Moving Up from Quote 1 lands at the bottom of tall Quote 0 and scrolls back to its top.
        step_quote(&feed, viewport, false, &mut selected, &mut scroll);
        assert_eq!(selected, 0);
        while scroll > feed.spans[0].0 {
            let prev = scroll;
            step_quote(&feed, viewport, false, &mut selected, &mut scroll);
            assert_eq!(selected, 0);
            assert!(scroll < prev);
        }
    }

    #[test]
    fn library_action_bar_fits_within_72_columns() {
        unsafe { libc::setlocale(libc::LC_CTYPE, c"C.UTF-8".as_ptr()) };
        let bar = list_action_line(Some("C Цитаты"), Some("E Не прочитано"));
        let cols: u32 = bar.chars().map(crate::terminal::cell_width).sum();
        assert!(cols <= 72, "bar width {cols} > 72: {bar}");
    }
}
