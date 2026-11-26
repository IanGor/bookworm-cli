use crate::terminal::{Key, Terminal};
use anyhow::{Context, Result, ensure};
use bookworm::theme::{self, THEMES};
use std::{
    env,
    path::{Path, PathBuf},
};

pub const TITLE: &str = "1";
pub const MUTED: &str = "2";
pub const SELECTED: &str = "7";
pub const NORMAL: &str = "0";

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
    pub item_action: Option<(&'a [bool], &'a str)>,
    pub empty: &'a str,
    pub footer: &'a str,
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
        let keys = match list.item_action {
            Some((flags, action)) if flags.get(*selected) == Some(&true) => {
                format!("  ↑↓ Выбор   PgUp/PgDn   Enter Открыть   {action}   Esc Назад")
            }
            _ => "  ↑↓ Выбор   PgUp/PgDn Листать   Enter Открыть   Esc Назад".into(),
        };
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
}
