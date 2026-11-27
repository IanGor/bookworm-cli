mod report;
mod selection;
mod terminal;
mod ui;

use anyhow::{Context, Result, ensure};
use bookworm::{
    fonts,
    library::{self, Scan},
    quotes::{self, Quote, QuoteStore},
    reader::{Frame, PAGE_ROW, Reader, SIDEBAR_COLS},
    storage::{self, Anchor, Store},
    theme::{self, THEMES},
};
use bookworm_core::{book::Book, font::Family, render};
use std::{env, fmt, fs, path::Path, process::ExitCode};
use terminal::{Key, Terminal};
use ui::{List, MUTED, NORMAL, QuoteAction, TITLE};

const HELP: &str = "Bookworm — локальная читалка FB2 и EPUB

  bookworm              открыть библиотеку
  bookworm ПУТЬ         читать файл FB2, FB2.ZIP или EPUB
  bookworm --licenses   лицензии встроенных шрифтов

В книге: ← → страницы, T оглавление, N сноски, C цитаты, F шрифт, S настройки,
+ и - кегль, V разворот, Y темы, D выделить фрагмент, B библиотека, ? помощь, Q выход.
На последней странице E открывает подтверждение завершения.";
#[derive(Debug)]
struct Quit;
impl fmt::Display for Quit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Выход")
    }
}
impl std::error::Error for Quit {}
fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) if e.is::<Quit>() => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!(
                "bookworm — {}",
                format!("{e:#}").replace(|c: char| c.is_control(), " ")
            );
            ExitCode::FAILURE
        }
    }
}
fn run() -> Result<()> {
    let mut args = env::args_os().skip(1);
    let arg = args.next();
    ensure!(args.next().is_none(), "Использование: bookworm [ПУТЬ]");
    if let Some(a) = arg.as_deref().and_then(|a| a.to_str()) {
        match a {
            "-h" | "--help" => {
                println!("{HELP}");
                return Ok(());
            }
            "--licenses" => {
                println!("{}", fonts::LICENSES);
                return Ok(());
            }
            _ => (),
        }
    }
    let store = Store::open(&storage::data_path()?)?;
    let mut term = Terminal::enter()?;
    term.palette = THEMES[theme::index(&store.db.theme)].palette;
    let mut app = App {
        term,
        store,
        scan: None,
        custom: Vec::new(),
        font_issue: None,
    };
    if let Some(path) = &app.store.db.font_folder {
        match fonts::discover(path) {
            Ok(f) => app.custom = f,
            Err(e) => app.font_issue = Some(e.to_string()),
        }
    }
    if let Some(p) = arg {
        app.read(Path::new(&p), true, None)?;
    }
    app.library()
}

struct App {
    term: Terminal,
    store: Store,
    scan: Option<Scan>,
    custom: Vec<Family>,
    font_issue: Option<String>,
}
impl App {
    fn settings(&mut self) -> Result<bool> {
        let initial = self.store.db.bionic;
        let mut selected = 0;
        loop {
            let mode = if self.store.db.bionic {
                "Вкл."
            } else {
                "Выкл."
            };
            let items = [
                format!("Бионическое чтение   {mode}"),
                "Цветовые темы".into(),
            ];
            match ui::list(
                &mut self.term,
                List {
                    title: "Настройки",
                    subtitle: "Оформление и чтение",
                    items: &items,
                    ..Default::default()
                },
                &mut selected,
            )? {
                Key::Exit => return Err(Quit.into()),
                Key::Esc | Key::Char('q' | 's') => return Ok(initial != self.store.db.bionic),
                Key::Enter if selected == 0 => {
                    self.store.db.bionic = !self.store.db.bionic;
                    self.store.save()?;
                }
                Key::Enter => self.themes()?,
                _ => (),
            }
        }
    }
    fn themes(&mut self) -> Result<()> {
        let current = theme::index(&self.store.db.theme);
        if let Some(index) = ui::theme_picker(&mut self.term, current)? {
            let preset = &THEMES[index];
            self.store.db.theme = preset.id.into();
            self.store.save()?;
            self.term.palette = preset.palette;
        }
        Ok(())
    }
    fn message(&mut self, title: &str, msg: &str) -> Result<()> {
        if ui::notice(&mut self.term, title, msg)? == Key::Exit {
            return Err(Quit.into());
        }
        Ok(())
    }
    fn busy(&mut self, label: &str) -> Result<()> {
        ui::title(&mut self.term, label)?;
        self.term.flush()
    }
    fn scan_folder(&mut self, path: &Path) -> Result<()> {
        self.busy("Читаю каталог книг…")?;
        match library::scan(path, &mut self.store.db) {
            Ok(scan) => {
                self.scan = Some(scan);
                self.store.db.folder = Some(path.to_owned());
                self.store.save()?;
            }
            Err(e) => self.message("Не удалось открыть папку", &e.to_string())?,
        }
        Ok(())
    }
    fn library(&mut self) -> Result<()> {
        if self.scan.is_none()
            && let Some(path) = self.store.db.folder.clone()
        {
            self.scan_folder(&path)?;
        }
        let mut selected = 0;
        loop {
            let folder = self
                .store
                .db
                .folder
                .as_deref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "Выберите папку с книгами клавишей D".into());
            let scan = self.scan.get_or_insert_with(Scan::default);
            scan.sort(&self.store.db);
            let resume = scan.entries.iter().position(|entry| {
                let book = &self.store.db.books[&entry.id];
                book.completed_at().is_none() && book.last_read.is_some() && book.progress < 100
            });
            let mut sections = Vec::new();
            let mut last_month = None;
            let mut completed = Vec::with_capacity(scan.entries.len());
            let rows: Vec<_> = scan
                .entries
                .iter()
                .enumerate()
                .map(|(index, entry)| {
                    let r = &self.store.db.books[&entry.id];
                    if let Some(ts) = r.completed_at() {
                        completed.push(true);
                        let month = library::completion_month(ts);
                        if month != last_month {
                            if let Some(label) = &month {
                                sections.push((index, format!("── {label} ──")));
                            }
                            last_month = month;
                        }
                    } else {
                        completed.push(false);
                    }
                    let author = if r.author().is_empty() {
                        "Автор неизвестен"
                    } else {
                        r.author()
                    };
                    let mark = if resume == Some(index) { "o~" } else { "  " };
                    format!("{mark} {:>3}%   {}  —  {}", r.progress, r.title(), author)
                })
                .collect();
            let subtitle = if scan.warnings.is_empty() {
                format!("Книг — {}   ·   {folder}", rows.len())
            } else {
                format!(
                    "Книг — {}   ·   Пропущено — {} (W)   ·   {folder}",
                    rows.len(),
                    scan.warnings.len()
                )
            };
            let key = ui::list(
                &mut self.term,
                List {
                    title: "Библиотека",
                    subtitle: &subtitle,
                    items: &rows,
                    sections: &sections,
                    base_action: "C Цитаты",
                    item_action: Some((&completed, "E Не прочитано")),
                    empty: "Книг пока нет. D — выбрать папку, R — обновить.",
                    footer: "  S Настройки   Y Темы   D Папка   R Обновить   W Ошибки   Q Выход",
                },
                &mut selected,
            )?;
            match key {
                Key::Exit | Key::Esc | Key::Char('q') => return Ok(()),
                Key::Char('s') => {
                    self.settings()?;
                }
                Key::Char('y') => self.themes()?,
                Key::Char('d') => {
                    if let Some(dir) = ui::directory(&mut self.term, "Папка библиотеки")?
                    {
                        self.scan_folder(&dir)?;
                        selected = 0;
                    }
                }
                Key::Char('r') => {
                    if let Some(dir) = self.store.db.folder.clone() {
                        self.scan_folder(&dir)?;
                    }
                }
                Key::Char('w') => {
                    let warnings = &scan.warnings;
                    if ui::list(
                        &mut self.term,
                        List {
                            title: "Пропущенные файлы",
                            subtitle: "Ошибки при последнем обновлении библиотеки",
                            items: warnings,
                            empty: "Ошибок нет",
                            footer: "  Esc Назад",
                            ..Default::default()
                        },
                        &mut 0,
                    )? == Key::Exit
                    {
                        return Err(Quit.into());
                    }
                }
                Key::Char('c') => {
                    if let Some(entry) = scan.entries.get(selected) {
                        let id = entry.id.clone();
                        let path = entry.path.clone();
                        let r = &self.store.db.books[&id];
                        let (title, author) = (r.title().to_owned(), r.author().to_owned());
                        if let Some(target) = self.quotes_menu(&id, &title, &author)? {
                            self.read(&path, false, Some(target))?;
                        }
                    }
                }
                Key::Char('e') => {
                    if let Some(entry) = scan.entries.get(selected)
                        && self.store.db.books[&entry.id].completed_at().is_some()
                    {
                        let id = entry.id.clone();
                        if ui::confirm(
                            &mut self.term,
                            "Снять пометку о прочтении?",
                            "Книга вернётся в общий список с текущим процентом чтения.",
                        )? {
                            self.store.db.books.get_mut(&id).unwrap().rereading = true;
                            self.store.save()?;
                            scan.sort(&self.store.db);
                            if let Some(pos) = scan.entries.iter().position(|e| e.id == id) {
                                selected = pos;
                            }
                        }
                    }
                }
                Key::Enter => {
                    if let Some(entry) = scan.entries.get(selected) {
                        let path = entry.path.clone();
                        self.read(&path, false, None)?;
                    }
                }
                _ => (),
            }
        }
    }
    fn layout(&self, bytes: &[u8], frame: Frame, anchor: Anchor) -> Result<(Reader, bool)> {
        let (set, fallback) = match fonts::load(&self.store.db.font, &self.custom) {
            Ok(f) => (f, false),
            Err(_) => (fonts::load("PT Serif", &[])?, true),
        };
        let mut g = frame.geometry(self.store.db.size, set.faces[0].x_height);
        g.bionic = self.store.db.bionic;
        let book = Book::open(bytes, set, g).map_err(anyhow::Error::msg)?;
        Ok((Reader::new(book, frame, anchor)?, fallback))
    }
    fn frame(&mut self) -> Result<Option<Frame>> {
        loop {
            match Frame::new(
                self.term.window,
                self.store.db.size,
                self.store.db.single_page,
            ) {
                Ok(f) => return Ok(Some(f)),
                Err(e) => {
                    ui::title(&mut self.term, "Область чтения")?;
                    self.term.line(2, &e.to_string(), NORMAL)?;
                    self.term
                        .line(3, "  - Кегль меньше   B Библиотека   Q Выход", MUTED)?;
                    self.term.flush()?;
                    match self.term.key()?.command() {
                        Key::Exit | Key::Char('q') => return Err(Quit.into()),
                        Key::Esc | Key::Char('b') => return Ok(None),
                        Key::Char('-') => {
                            self.store.db.size = self.store.db.size.saturating_sub(2).max(12);
                            self.store.save()?;
                        }
                        _ => (),
                    }
                }
            }
        }
    }
    fn save_position(&mut self, id: &str, r: &Reader) -> Result<()> {
        let record = self.store.db.books.get_mut(id).unwrap();
        record.anchor = r.anchor;
        record.progress = r.state().progress();
        record.last_read = Some(storage::now());
        self.store.save()
    }
    fn read(&mut self, path: &Path, direct: bool, jump: Option<Anchor>) -> Result<()> {
        self.busy("Открываю книгу…")?;
        let bytes = match fs::read(path) {
            Ok(b) => b,
            Err(e) if direct => {
                return Err(e).with_context(|| format!("Книга недоступна — {}", path.display()));
            }
            Err(e) => {
                self.message("Книга недоступна", &format!("{} — {e}", path.display()))?;
                return Ok(());
            }
        };
        let id = storage::content_id(&bytes);
        let anchor = jump
            .or_else(|| self.store.db.books.get(&id).map(|r| r.anchor))
            .unwrap_or_default();
        let Some(frame) = self.frame()? else {
            return Ok(());
        };
        let (mut r, mut fallback) = match self.layout(&bytes, frame, anchor) {
            Ok(r) => r,
            Err(e) if direct => {
                return Err(e).with_context(|| format!("Не удалось открыть {}", path.display()));
            }
            Err(e) => {
                self.message("Не удалось открыть книгу", &e.to_string())?;
                return Ok(());
            }
        };
        if let Some(target) = jump {
            r.jump_to(target);
        }
        library::metadata(&mut self.store.db, &id, path, &r.book.doc);
        self.save_position(&id, &r)?;
        let mut status = if fallback {
            "Шрифт недоступен. Выбран PT Serif для этой книги. F — сменить.".into()
        } else {
            String::new()
        };
        let mut toc_focus = None;
        let mut full = true;
        loop {
            self.draw_reader(&id, &r, &status, toc_focus, None, full)?;
            let clears = self.term.clears;
            status.clear();
            let mut reflow = false;
            let key = self.term.key()?.command();
            if key == Key::Char('g') {
                self.store.db.bionic = !self.store.db.bionic;
                reflow = true;
                status = if self.store.db.bionic {
                    "Бионическое чтение включено".into()
                } else {
                    "Бионическое чтение выключено".into()
                };
            } else if let Some(selected) = toc_focus.as_mut() {
                let count = r.book.doc.toc.len();
                let step = toc_visible(self.term.window.rows);
                match key {
                    Key::Exit | Key::Char('q') => return Err(Quit.into()),
                    Key::Esc | Key::Char('t') | Key::Resize => toc_focus = None,
                    Key::Char('s') => reflow = self.settings()?,
                    Key::Char('y') => self.themes()?,
                    Key::Enter => {
                        r.jump(r.book.doc.toc[*selected].block);
                        self.save_position(&id, &r)?;
                        toc_focus = None;
                    }
                    Key::Up | Key::Char('k') => *selected = selected.saturating_sub(1),
                    Key::Down | Key::Char('j') => *selected = (*selected + 1).min(count - 1),
                    Key::PageUp => *selected = selected.saturating_sub(step),
                    Key::PageDown => *selected = (*selected + step).min(count - 1),
                    Key::Home => *selected = 0,
                    Key::End => *selected = count - 1,
                    _ => (),
                }
            } else {
                match key {
                    Key::Exit | Key::Char('q') => return Err(Quit.into()),
                    Key::Esc | Key::Char('b') => return Ok(()),
                    Key::Right | Key::PageDown | Key::Char(' ') | Key::Down => {
                        if r.turn(true) {
                            self.save_position(&id, &r)?;
                        }
                    }
                    Key::Left | Key::PageUp | Key::Up => {
                        if r.turn(false) {
                            self.save_position(&id, &r)?;
                        }
                    }
                    Key::Char('t') if r.frame.sidebar && !r.book.doc.toc.is_empty() => {
                        toc_focus = Some(current_chapter(&r));
                    }
                    Key::Char('t') => {
                        if self.toc(&mut r)? {
                            self.save_position(&id, &r)?;
                        }
                    }
                    Key::Char('n') => self.notes(&r)?,
                    Key::Char('c') => {
                        let rec = &self.store.db.books[&id];
                        let (title, author) = (rec.title().to_owned(), rec.author().to_owned());
                        if let Some(target) = self.quotes_menu(&id, &title, &author)? {
                            r.jump_to(target);
                            self.save_position(&id, &r)?;
                        }
                    }
                    Key::Char('e') if r.state().at_end() => {
                        let again = !self.store.db.books[&id].completions.is_empty();
                        let heading = if again {
                            "Завершить перечитывание?"
                        } else {
                            "Завершить книгу?"
                        };
                        let info = if again {
                            "Предыдущее завершение сохранится. Записать ещё одно?"
                        } else {
                            "Записать дату и время завершения чтения?"
                        };
                        if ui::confirm(&mut self.term, heading, info)? {
                            r.state()
                                .finish(true, self.store.db.books.get_mut(&id).unwrap());
                            self.save_position(&id, &r)?;
                            status = "Завершение записано".into();
                        }
                    }
                    Key::Char('v') => {
                        self.store.db.single_page = !self.store.db.single_page;
                        self.store.save()?;
                    }
                    Key::Char(c @ ('+' | '=' | '-')) => {
                        let size = self.store.db.size;
                        self.store.db.size = if c == '-' {
                            size.saturating_sub(2).max(12)
                        } else {
                            (size + 2).min(72)
                        };
                        reflow = size != self.store.db.size;
                        if reflow {
                            self.store.save()?;
                        }
                        status = format!("  Кегль {} px", self.store.db.size);
                    }
                    Key::Char('f') => reflow = self.font_menu()?,
                    Key::Char('d') => {
                        if let Some(msg) = self.select_fragment(path, &id, &r, fallback)? {
                            status = msg;
                        }
                    }
                    Key::Char('s') => reflow = self.settings()?,
                    Key::Char('y') => self.themes()?,
                    Key::Char('?') => self.help()?,
                    _ => (),
                }
            }
            let Some(frame) = self.frame()? else {
                return Ok(());
            };
            full = reflow || frame != r.frame || self.term.clears != clears;
            if reflow || frame.width != r.frame.width || frame.height != r.frame.height {
                (r, fallback) = self.layout(&bytes, frame, r.anchor)?;
                self.save_position(&id, &r)?;
            } else if frame != r.frame {
                r.frame = frame;
                self.save_position(&id, &r)?;
            }
        }
    }
    fn quotes_menu(&mut self, id: &str, title: &str, author: &str) -> Result<Option<Anchor>> {
        let path = quotes::quotes_path(id)?;
        let mut store = match QuoteStore::open(&path) {
            Ok(s) => s,
            Err(e) => {
                self.message("Ошибка чтения цитат", &e.to_string())?;
                return Ok(None);
            }
        };
        let mut selected = 0;
        let mut scroll = 0;
        loop {
            match ui::quotes_view(
                &mut self.term,
                title,
                author,
                &store.quotes,
                &mut selected,
                &mut scroll,
            )? {
                QuoteAction::Back => return Ok(None),
                QuoteAction::Jump(i) => return Ok(Some(store.quotes[i].start)),
                QuoteAction::Edit(i) => {
                    let current = store.quotes[i].comment.clone();
                    let text = store.quotes[i].text.clone();
                    if let Some(updated) = ui::quote_comment(
                        &mut self.term,
                        "Комментарий к цитате",
                        &text,
                        current.as_deref(),
                    )? {
                        store.quotes[i].comment = updated;
                        if let Err(e) = store.save() {
                            self.message("Не удалось сохранить цитату", &e.to_string())?;
                        }
                    }
                }
                QuoteAction::Delete(i) => {
                    if ui::confirm(
                        &mut self.term,
                        "Удалить цитату?",
                        "Цитата и комментарий к ней будут удалены.",
                    )? {
                        store.quotes.remove(i);
                        if let Err(e) = store.save() {
                            self.message("Не удалось сохранить цитаты", &e.to_string())?;
                        }
                    }
                }
            }
        }
    }
    fn select_fragment(
        &mut self,
        path: &Path,
        id: &str,
        r: &Reader,
        fallback: bool,
    ) -> Result<Option<String>> {
        let Some(mut selection) = selection::Selection::new(r) else {
            self.message(
                "Выделение текста",
                "На видимых страницах нет текстовых блоков.",
            )?;
            return Ok(None);
        };
        let mut empty = false;
        let (preview, range) = loop {
            let status = match (selection.anchor.is_some(), empty) {
                (_, true) => "Сдвинь курсор до конца фрагмента · Enter Выбрать · Esc Отмена",
                (false, false) => {
                    "Enter Начало · ←→ Текст · ↑↓ Строка · PgUp/PgDn Страница · Esc Отмена"
                }
                (true, false) => {
                    "Enter Выбрать · ←→ Текст · ↑↓ Строка · PgUp/PgDn Страница · Esc Отмена"
                }
            };
            let preview = selection.preview();
            self.draw_reader(id, r, status, None, Some(preview), false)?;
            match self.term.key()?.command() {
                Key::Enter if selection.anchor.is_none() => selection.anchor = Some(preview.cursor),
                Key::Enter => match preview.range {
                    Some(range) => break (preview, range),
                    None => empty = true,
                },
                Key::Esc | Key::Resize => return Ok(None),
                Key::Exit => return Err(Quit.into()),
                key => {
                    empty = false;
                    selection.move_cursor(r, key);
                }
            }
        };
        let prompt = "C Цитата / комментарий   ·   D Дефект движка   ·   Esc Отмена";
        let report_mode = loop {
            self.draw_reader(id, r, prompt, None, Some(preview), false)?;
            match self.term.key()?.command() {
                Key::Char('c') => break false,
                Key::Char('d') => break true,
                Key::Esc | Key::Resize => return Ok(None),
                Key::Exit => return Err(Quit.into()),
                _ => (),
            }
        };
        if !report_mode {
            let start = Anchor {
                block: range.start.block as u32,
                offset: range.start.offset,
            };
            let end = Anchor {
                block: range.end.block as u32,
                offset: range.end.offset,
            };
            let (chapter, text) = quotes::extract_quote(&r.book.doc, start, end);
            if text.trim().is_empty() {
                self.message("Новая цитата", "В выбранном диапазоне нет текста.")?;
                return Ok(None);
            }
            let Some(comment) = ui::quote_comment(&mut self.term, "Новая цитата", &text, None)?
            else {
                return Ok(None);
            };
            let saved = (|| -> Result<()> {
                let mut store = QuoteStore::open(&quotes::quotes_path(id)?)?;
                store.add(Quote {
                    start,
                    end,
                    chapter,
                    text,
                    comment,
                    created_at: storage::now(),
                });
                store.save()
            })();
            return match saved {
                Ok(()) => Ok(Some("Цитата сохранена".into())),
                Err(e) => {
                    self.message("Не удалось сохранить цитату", &e.to_string())?;
                    Ok(None)
                }
            };
        }
        let Some(description) = ui::report_description(&mut self.term, range)? else {
            return Ok(None);
        };
        let font = if fallback {
            "PT Serif"
        } else {
            &self.store.db.font
        };
        match report::save(path, id, r, range, font, &description) {
            Ok(file) => ui::report_saved(&mut self.term, &file)?,
            Err(e) => self.message("Не удалось сохранить промпт", &e.to_string())?,
        }
        Ok(None)
    }
    fn draw_reader(
        &mut self,
        id: &str,
        r: &Reader,
        status: &str,
        toc_focus: Option<usize>,
        selection: Option<selection::Preview>,
        full: bool,
    ) -> Result<()> {
        self.term.begin_frame()?;
        if full {
            self.term.clear()?;
        } else {
            // The frame geometry is unchanged: update the page images in place and erase only
            // the text chrome around them. The image spans rows PAGE_ROW..window.rows - 2.
            let window = self.term.window;
            for row in [
                2,
                PAGE_ROW - 1,
                window.rows - 2,
                window.rows - 1,
                window.rows,
            ] {
                self.term.erase(row, 1, window.cols)?;
            }
            if r.frame.sidebar {
                for row in 4..window.rows - 2 {
                    self.term.erase(row, 3, SIDEBAR_COLS - 3)?;
                }
            }
        }
        let mut drawn = Vec::new();
        let mut state = r.state();
        state.page = selection.map_or(r.page, |s| s.first_page);
        let last = (state.page + r.frame.pages).min(state.total);
        for (slot, n) in (state.page..last).enumerate() {
            let mut ink = render::transparent_canvas(r.frame.width, r.frame.height);
            if let Some(page) = r.book.page(n) {
                for (index, line) in page.lines.iter().enumerate() {
                    if let Some(s) = selection {
                        let accent = self
                            .term
                            .palette
                            .map_or([92, 159, 220], |p| bookworm::theme::rgb(p.accent));
                        let width = r.frame.width as usize;
                        let top = (line.baseline - r.book.g.unit() * 0.75).max(0.0) as usize;
                        let bottom = (line.baseline + r.book.g.unit() * 0.20).max(0.0) as usize;
                        let mut mark = |left: f32, right: f32, color: [u8; 4]| {
                            let l = (left.floor() as usize).min(width);
                            let rgt = (right.ceil() as usize).min(width);
                            if l < rgt {
                                for y in top..bottom.min(r.frame.height as usize) {
                                    let row = y * width;
                                    ink.data_mut()[(row + l) * 4..(row + rgt) * 4]
                                        .as_chunks_mut::<4>()
                                        .0
                                        .fill(color);
                                }
                            }
                        };
                        if let Some(range) = s.range {
                            let mut left = f32::INFINITY;
                            let mut right = f32::NEG_INFINITY;
                            for (a, b, x0, x1) in selection::glyphs(line) {
                                if range.overlaps(line.block, a, b) {
                                    left = left.min(x0);
                                    right = right.max(x1);
                                }
                            }
                            // The canvas uses premultiplied RGBA. Keep the terminal background visible.
                            mark(
                                left,
                                right,
                                [accent[0] / 4, accent[1] / 4, accent[2] / 4, 64],
                            );
                        }
                        if s.cursor.page == n && s.cursor.line == index {
                            mark(
                                s.cursor.x - 1.0,
                                s.cursor.x + 1.0,
                                [accent[0], accent[1], accent[2], 255],
                            );
                        }
                    }
                    render::draw_runs(
                        &mut ink,
                        &r.book.fonts,
                        line.x,
                        line.baseline,
                        &line.runs,
                        self.term.ink(),
                    );
                }
            }
            let png = ink.encode_png()?;
            let slot = slot as u32;
            let placed = self.term.image(
                PAGE_ROW,
                r.frame.first_col + slot * r.frame.stride,
                slot,
                &png,
            )?;
            drawn.push(placed);
        }
        self.term.prune_images(&drawn)?;
        let window = self.term.window;
        let bottom = window.rows - 2;
        let left = r.frame.first_col;
        let page_cols = r.frame.stride - 1;
        let right = left + (last - state.page - 1) as u32 * r.frame.stride + page_cols - 1;
        if r.frame.sidebar {
            self.term.text(2, 3, SIDEBAR_COLS - 2, "bookworm", TITLE)?;
            self.draw_contents(r, toc_focus)?;
        }
        let record = &self.store.db.books[id];
        let heading = if record.author().is_empty() {
            record.title().to_owned()
        } else {
            format!("{}   ·   {}", record.title(), record.author())
        };
        let header_col = if r.frame.sidebar { SIDEBAR_COLS + 1 } else { 3 };
        self.term
            .text(2, header_col, window.cols - header_col, &heading, TITLE)?;
        for (row, a, b) in [(PAGE_ROW - 1, "┌──", "──┐"), (bottom, "└──", "──┘")]
        {
            self.term.text(row, left - 1, 3, a, "2;90")?;
            self.term.text(row, right - 1, 3, b, "2;90")?;
        }
        for (slot, n) in (state.page..last).enumerate() {
            let label = (n + 1).to_string();
            let center = left + slot as u32 * r.frame.stride + page_cols / 2;
            self.term.text(
                bottom,
                center.saturating_sub(label.len() as u32 / 2),
                label.len() as u32,
                &label,
                MUTED,
            )?;
        }
        let range = if last == state.page + 1 {
            last.to_string()
        } else {
            format!("{}–{last}", state.page + 1)
        };
        let progress = state.progress();
        let summary = format!("{progress}%   {range} / {}", state.total);
        let footer_left = header_col;
        let hint = "[?] Клавиши";
        let hint_col = (window.cols + 1).saturating_sub(hint.chars().count() as u32 + 2);
        let space = hint_col.saturating_sub(footer_left + summary.chars().count() as u32 + 4);
        let track = space.min(27);
        let fill = (u32::from(progress) * track + 50) / 100;
        let mut summary_col = footer_left;
        if track >= 6 {
            self.term.text(
                window.rows,
                footer_left,
                fill,
                &"━".repeat(fill as usize),
                "36",
            )?;
            self.term.text(
                window.rows,
                footer_left + fill,
                track - fill,
                &"─".repeat((track - fill) as usize),
                "90",
            )?;
            summary_col += track + 2;
        }
        self.term.text(
            window.rows,
            summary_col,
            window.cols - summary_col + 1,
            &summary,
            NORMAL,
        )?;
        let message = if !status.is_empty() {
            status
        } else if state.at_end() {
            "E Завершить книгу"
        } else {
            ""
        };
        if !message.is_empty() {
            self.term.text(
                window.rows - 1,
                footer_left,
                window.cols - footer_left + 1,
                message,
                TITLE,
            )?;
        } else if window.cols >= 90 && toc_focus.is_none() {
            let turn_hint = "[←] [→] Листать";
            let turn_width = turn_hint.chars().count() as u32;
            let turn_col = hint_col.saturating_sub(turn_width + 3);
            if turn_col > summary_col + summary.chars().count() as u32 {
                self.term
                    .text(window.rows, turn_col, turn_width, turn_hint, MUTED)?;
            }
        }
        if hint_col > summary_col + summary.chars().count() as u32 {
            self.term.text(
                window.rows,
                hint_col,
                window.cols - hint_col + 1,
                hint,
                MUTED,
            )?;
        }
        self.term.end_frame()
    }
    fn draw_contents(&mut self, r: &Reader, focus: Option<usize>) -> Result<()> {
        let count = r.book.doc.toc.len();
        let width = SIDEBAR_COLS - 4;
        let rows = self.term.window.rows;
        self.term.text(4, 3, width, "Оглавление", TITLE)?;
        if count == 0 {
            return self.term.text(7, 3, width, "Нет разделов", MUTED);
        }
        let chapter = current_chapter(r);
        let visible = toc_visible(rows);
        let first = (focus.unwrap_or(chapter) / visible) * visible;
        for (i, entry) in r.book.doc.toc.iter().enumerate().skip(first).take(visible) {
            let indent = " ".repeat(usize::from(entry.level.saturating_sub(1).min(4)));
            let mark = if i == chapter { "▸" } else { " " };
            let row = 7 + 2 * (i - first) as u32;
            let style = if focus == Some(i) {
                ui::SELECTED
            } else if i == chapter {
                "1;36"
            } else {
                NORMAL
            };
            self.term.text(row, 3, 3, &format!("{:02}", i + 1), MUTED)?;
            self.term.text(
                row,
                6,
                width - 3,
                &format!("{mark} {indent}{}", entry.title),
                style,
            )?;
        }
        let hint = if focus.is_some() {
            "Enter Перейти · Esc Назад"
        } else {
            "T  К разделам"
        };
        self.term.text(rows - 3, 3, width, hint, MUTED)
    }
    fn toc(&mut self, r: &mut Reader) -> Result<bool> {
        let items: Vec<_> = r
            .book
            .toc()
            .iter()
            .map(|(level, n, title)| {
                format!(
                    "{:>4}   {}{}",
                    n + 1,
                    "  ".repeat(usize::from((*level).min(6))),
                    title
                )
            })
            .collect();
        let mut i = r
            .book
            .doc
            .toc
            .iter()
            .rposition(|e| e.block <= r.anchor.block as usize)
            .unwrap_or(0);
        loop {
            match ui::list(
                &mut self.term,
                List {
                    title: "Оглавление",
                    subtitle: "Страница   ·   Раздел",
                    items: &items,
                    empty: "В книге нет оглавления",
                    footer: "  Enter Перейти к разделу   Esc Вернуться к чтению",
                    ..Default::default()
                },
                &mut i,
            )? {
                Key::Exit => return Err(Quit.into()),
                Key::Enter if !items.is_empty() => {
                    r.jump(r.book.doc.toc[i].block);
                    return Ok(true);
                }
                Key::Esc | Key::Char('q') => return Ok(false),
                _ => (),
            }
        }
    }
    fn notes(&mut self, r: &Reader) -> Result<()> {
        let ids = r.notes();
        let labels: Vec<_> = ids
            .iter()
            .map(|id| {
                format!(
                    "{}   {}",
                    id,
                    r.book.doc.notes[id]
                        .iter()
                        .map(|b| b.text())
                        .find(|t| !t.trim().is_empty())
                        .unwrap_or_default()
                )
            })
            .collect();
        let mut selected = 0;
        loop {
            match ui::list(
                &mut self.term,
                List {
                    title: "Сноски",
                    subtitle: "Ссылки на открытых страницах",
                    items: &labels,
                    empty: "На этих страницах нет сносок",
                    footer: "  Enter Читать сноску   Esc Вернуться к книге",
                    ..Default::default()
                },
                &mut selected,
            )? {
                Key::Exit => return Err(Quit.into()),
                Key::Esc | Key::Char('q') => return Ok(()),
                Key::Enter if !ids.is_empty() => {
                    self.note(r, &ids[selected])?;
                    return Ok(());
                }
                _ => (),
            }
        }
    }
    fn note(&mut self, r: &Reader, id: &str) -> Result<()> {
        let (lines, height) = r.book.note(id).context("Сноска недоступна")?;
        let mut top = 0f32;
        let mut full = true;
        loop {
            self.term.begin_frame()?;
            if full {
                self.term.clear()?;
            }
            // A long note is drawn in slices, with complete lines shared at the seam.
            let h = r.frame.height;
            let mut pix = render::transparent_canvas(r.frame.width, h);
            for l in &lines {
                let y = l.baseline - top;
                if y >= -r.book.g.unit() && y <= h as f32 + r.book.g.unit() {
                    render::draw_runs(&mut pix, &r.book.fonts, l.x, y, &l.runs, self.term.ink());
                }
            }
            self.term
                .image(1, r.frame.first_col, 0, &pix.encode_png()?)?;
            let rows = self.term.window.rows;
            self.term
                .line(rows.saturating_sub(1), &format!("  Сноска {id}"), TITLE)?;
            self.term
                .line(rows, "  ↑↓ Листать сноску   Esc Вернуться к книге", MUTED)?;
            self.term.end_frame()?;
            full = false;
            let step = (h as f32 - 2.0 * r.book.g.unit()).max(1.0);
            match self.term.key()?.command() {
                Key::Exit => return Err(Quit.into()),
                Key::Esc | Key::Resize | Key::Char('q') => return Ok(()),
                Key::Down | Key::PageDown | Key::Right | Key::Char(' ') => {
                    top = (top + step).min((height - h as f32).max(0.))
                }
                Key::Up | Key::PageUp | Key::Left => top = (top - step).max(0.),
                _ => (),
            }
        }
    }
    fn font_menu(&mut self) -> Result<bool> {
        let mut selected = 0;
        loop {
            let mut names: Vec<_> = fonts::BUILTIN.iter().map(|s| s.to_string()).collect();
            let mut labels = names.clone();
            for f in &self.custom {
                names.push(format!("custom:{}", f.name));
                labels.push(format!(
                    "{}   ·   своя гарнитура{}",
                    f.name,
                    if f.cyrillic {
                        ""
                    } else {
                        " без кириллицы"
                    }
                ));
            }
            for (i, name) in names.iter().enumerate() {
                if name == &self.store.db.font {
                    labels[i].push_str("   ✓");
                }
            }
            labels.push("Выбрать папку с TTF / OTF…".into());
            let detail = self.font_issue.clone().unwrap_or_else(|| {
                format!(
                    "Кегль {} px   ·   {}",
                    self.store.db.size,
                    self.store
                        .db
                        .font_folder
                        .as_deref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "Встроенные шрифты".into())
                )
            });
            match ui::list(
                &mut self.term,
                List {
                    title: "Шрифт",
                    subtitle: &detail,
                    items: &labels,
                    footer: "  D Папка шрифтов   Enter Выбрать   Esc Назад",
                    ..Default::default()
                },
                &mut selected,
            )? {
                Key::Exit => return Err(Quit.into()),
                Key::Esc | Key::Char('q') => return Ok(false),
                Key::Enter if selected < names.len() => {
                    self.store.db.font = names[selected].clone();
                    self.store.save()?;
                    return Ok(true);
                }
                Key::Char('d') | Key::Enter => {
                    if let Some(dir) = ui::directory(&mut self.term, "Папка шрифтов TTF / OTF")?
                    {
                        match fonts::discover(&dir) {
                            Ok(custom) => {
                                self.custom = custom;
                                self.font_issue = if self.custom.is_empty() {
                                    Some("В папке нет доступных шрифтов TTF / OTF".into())
                                } else {
                                    None
                                };
                                self.store.db.font_folder = Some(dir);
                                self.store.save()?;
                                selected = fonts::BUILTIN.len();
                            }
                            Err(e) => self.message("Ошибка чтения шрифтов", &e.to_string())?,
                        }
                    }
                }
                _ => (),
            }
        }
    }
    fn help(&mut self) -> Result<()> {
        let items = [
            "← / →, PgUp / PgDn   Предыдущие / следующие страницы",
            "Пробел   Следующая страница",
            "T   Оглавление и переход к разделу",
            "N   Сноски открытых страниц",
            "C   Цитаты и комментарии к книге",
            "F   Выбор гарнитуры и папки с TTF / OTF",
            "S   Настройки чтения и оформления",
            "G   Включить / выключить бионическое чтение",
            "Y   Настройки цветовой темы",
            "D   Выделить фрагмент: цитата или дефект вёрстки",
            "+ / -   Увеличить / уменьшить кегль",
            "V   Одна страница / автоматический разворот",
            "E   Подтвердить завершение в конце книги",
            "B, Esc   Вернуться в библиотеку",
            "Q, Ctrl+C   Выйти из читалки",
        ]
        .map(String::from);
        let mut i = 0;
        loop {
            match ui::list(
                &mut self.term,
                List {
                    title: "Клавиши чтения",
                    subtitle: "Буквенные команды — в латинской раскладке",
                    items: &items,
                    footer: "  Esc Вернуться к чтению",
                    ..Default::default()
                },
                &mut i,
            )? {
                Key::Exit => return Err(Quit.into()),
                Key::Esc | Key::Enter | Key::Char('q' | '?') => return Ok(()),
                _ => (),
            }
        }
    }
}

fn current_chapter(r: &Reader) -> usize {
    r.book
        .doc
        .toc
        .iter()
        .rposition(|entry| entry.block <= r.anchor.block as usize)
        .unwrap_or(0)
}

fn toc_visible(rows: u32) -> usize {
    (rows.saturating_sub(10) / 2).max(1) as usize
}
