use crate::storage::{Anchor, Record, now};
use anyhow::{Result, ensure};
use bookworm_core::{
    book::Book,
    layout::{Geometry, REF_X_HEIGHT},
    model::Position,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Window {
    pub cols: u32,
    pub rows: u32,
    pub width: u32,
    pub height: u32,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub pages: usize,
    pub first_col: u32,
    pub stride: u32,
    pub sidebar: bool,
}
pub const PAGE_ROW: u32 = 5;
pub const SIDEBAR_COLS: u32 = 30;
const CHROME_ROWS: u32 = 7;
impl Frame {
    pub fn new(w: Window, size: u16, single: bool) -> Result<Self> {
        ensure!(
            w.cols >= 12 && w.rows > CHROME_ROWS && w.width > 0 && w.height > 0,
            "Увеличьте окно терминала"
        );
        let em = u32::from(size);
        let height =
            (u64::from(w.height) * u64::from(w.rows - CHROME_ROWS) / u64::from(w.rows)) as u32;
        let px = |cols: u32| (u64::from(w.width) * u64::from(cols) / u64::from(w.cols)) as u32;
        let available = w.cols - 4;
        let holds_spread = |cols: u32| px(cols.saturating_sub(1) / 2) >= 25 * em;
        let sidebar = w.rows >= 18 && holds_spread(available.saturating_sub(SIDEBAR_COLS));
        let side = if sidebar { SIDEBAR_COLS } else { 0 };
        let available = available - side;
        let pages = if !single && holds_spread(available) {
            2
        } else {
            1
        };
        let slot = (available - (pages as u32 - 1)) / pages as u32;
        let cols = slot
            .min(((40 * em) as u64 * u64::from(w.cols) / u64::from(w.width)) as u32)
            .max(1);
        let width = px(cols);
        let stride = cols + 1;
        let first_col = 3 + side + (available - (cols * pages as u32 + pages as u32 - 1)) / 2;
        ensure!(
            width >= em * 6 && height >= em * 6,
            "Увеличьте окно терминала для кегля {em} px"
        );
        ensure!(
            u64::from(width) * u64::from(height) <= 32_000_000,
            "Слишком большая область страницы"
        );
        Ok(Self {
            width,
            height,
            pages,
            first_col,
            stride,
            sidebar,
        })
    }
    pub fn geometry(&self, size: u16, x_height: f32) -> Geometry {
        let nominal = f32::from(size);
        let font_size = nominal * REF_X_HEIGHT / x_height.clamp(0.35, 0.7);
        Geometry {
            width: self.width as f32,
            height: self.height as f32,
            font_size,
            leading: 1.4 * nominal / font_size,
            margin_x: (self.width as f32 * 0.07).min(2.5 * nominal),
            margin_y: (self.height as f32 * 0.08).min(2.0 * nominal),
            scroll: false,
            bionic: false,
        }
    }
}
#[derive(Clone, Copy)]
pub struct Reading {
    pub page: usize,
    pub total: usize,
    pub pages: usize,
}
impl Reading {
    pub fn at_end(self) -> bool {
        self.total > 0 && self.page + self.pages >= self.total
    }
    pub fn finish(self, confirmed: bool, r: &mut Record) -> bool {
        if !confirmed || !self.at_end() {
            return false;
        }
        r.rereading = false;
        r.completions.push(now());
        true
    }
    pub fn progress(self) -> u8 {
        ((self.page + self.pages).min(self.total) * 100 / self.total.max(1)) as u8
    }
}

pub struct Reader {
    pub book: Book,
    pub frame: Frame,
    pub anchor: Anchor,
    pub page: usize,
}
impl Reader {
    pub fn new(book: Book, frame: Frame, anchor: Anchor) -> Result<Self> {
        ensure!(book.page_count() > 0, "В книге нет страниц");
        let page = book.page_of(anchor.into());
        Ok(Self {
            book,
            frame,
            anchor,
            page,
        })
    }
    pub fn state(&self) -> Reading {
        Reading {
            page: self.page,
            total: self.book.page_count(),
            pages: self.frame.pages,
        }
    }
    pub fn turn(&mut self, forward: bool) -> bool {
        let page = if forward {
            if self.state().at_end() {
                return false;
            }
            self.page + self.frame.pages
        } else {
            self.page.saturating_sub(self.frame.pages)
        };
        if page == self.page {
            return false;
        }
        self.page = page;
        self.anchor = self.book.page(page).unwrap().start.into();
        true
    }
    pub fn jump(&mut self, block: usize) {
        let pos = Position {
            block: block as u32,
            offset: 0,
        };
        self.anchor = pos.into();
        self.page = self.book.page_of(pos);
    }
    pub fn jump_to(&mut self, anchor: Anchor) {
        let block = (anchor.block as usize).min(self.book.doc.blocks.len().saturating_sub(1));
        let max_offset = self.book.doc.blocks.get(block).map_or(0, |b| {
            b.spans.iter().flat_map(|s| s.text.chars()).count() as u32
        });
        let pos = Position {
            block: block as u32,
            offset: anchor.offset.min(max_offset),
        };
        self.anchor = pos.into();
        self.page = self.book.page_of(pos);
    }
    pub fn notes(&self) -> Vec<String> {
        let mut ids = Vec::new();
        for n in self.page..(self.page + self.frame.pages).min(self.book.page_count()) {
            for (_, id) in self.book.note_areas(n) {
                if self.book.doc.notes.contains_key(&id) && !ids.contains(&id) {
                    ids.push(id);
                }
            }
        }
        ids
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::Anchor;
    use bookworm_core::{book::Book, font::FontSet, model::Position};

    fn fonts() -> FontSet {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../BookwormEngine/fonts");
        FontSet::from_prefix(
            &format!("{root}/PTSerif"),
            &format!("{root}/LiberationMono-Regular.ttf"),
        )
        .unwrap()
    }

    fn sample_book() -> Book {
        let f = fonts();
        let xh = f.faces[0].x_height;
        let paragraphs: String = (0..12)
            .map(|i| {
                format!(
                    "<p>Абзац номер {i} с достаточным количеством слов, чтобы занять \
                     несколько строк при крупном кегле и дать много страниц.</p>"
                )
            })
            .collect();
        let xml = format!("<FictionBook><body>{paragraphs}</body></FictionBook>");
        Book::open(
            xml.as_bytes(),
            f,
            Geometry::new(500.0, 300.0, 40.0, xh, false),
        )
        .unwrap()
    }

    fn frame(pages: usize) -> Frame {
        Frame {
            width: 430,
            height: 250,
            pages,
            first_col: 3,
            stride: 44,
            sidebar: false,
        }
    }

    #[test]
    fn frame_new_lays_out_wide_window_as_spread_with_sidebar() {
        let w = Window {
            cols: 120,
            rows: 40,
            width: 1720,
            height: 640,
        };
        let f = Frame::new(w, 16, false).unwrap();
        assert!(f.sidebar);
        assert_eq!(f.pages, 2);
        assert_eq!((f.width, f.height), (602, 528));
        assert_eq!((f.first_col, f.stride), (33, 43));
    }

    #[test]
    fn frame_new_single_page_centers_one_column() {
        let w = Window {
            cols: 120,
            rows: 40,
            width: 1720,
            height: 640,
        };
        let f = Frame::new(w, 16, true).unwrap();
        assert!(f.sidebar);
        assert_eq!(f.pages, 1);
        assert_eq!(f.width, 630);
        assert_eq!((f.first_col, f.stride), (54, 45));
    }

    #[test]
    fn frame_new_narrow_window_drops_sidebar_and_spread() {
        let w = Window {
            cols: 60,
            rows: 20,
            width: 800,
            height: 500,
        };
        let f = Frame::new(w, 16, false).unwrap();
        assert!(!f.sidebar);
        assert_eq!(f.pages, 1);
        assert_eq!((f.width, f.height), (640, 325));
        assert_eq!(f.first_col, 7);
    }

    #[test]
    fn frame_new_rejects_unusable_windows() {
        let w = Window {
            cols: 10,
            rows: 40,
            width: 100,
            height: 100,
        };
        assert!(Frame::new(w, 16, false).is_err());
        // Columns too narrow for the requested font size.
        let w = Window {
            cols: 20,
            rows: 20,
            width: 100,
            height: 100,
        };
        assert!(Frame::new(w, 72, false).is_err());
    }

    #[test]
    fn reading_tracks_end_progress_and_completion() {
        let end = Reading {
            page: 98,
            total: 100,
            pages: 2,
        };
        assert!(end.at_end());
        assert_eq!(end.progress(), 100);
        assert!(
            !Reading {
                page: 97,
                total: 100,
                pages: 2
            }
            .at_end()
        );
        assert_eq!(
            Reading {
                page: 25,
                total: 100,
                pages: 2
            }
            .progress(),
            27
        );
        assert_eq!(
            Reading {
                page: 0,
                total: 0,
                pages: 1
            }
            .progress(),
            0
        );
        assert!(
            !Reading {
                page: 0,
                total: 0,
                pages: 1
            }
            .at_end()
        );

        let mut r = Record::default();
        assert!(end.finish(true, &mut r));
        assert_eq!(r.completions.len(), 1);
        let mut r = Record::default();
        assert!(!end.finish(false, &mut r));
        assert!(
            !Reading {
                page: 97,
                total: 100,
                pages: 2
            }
            .finish(true, &mut r)
        );
        assert!(r.completions.is_empty());
    }

    #[test]
    fn reader_turn_walks_pages_and_stops_at_ends() {
        let b = sample_book();
        let total = b.page_count();
        assert!(
            total >= 3,
            "fixture must produce multiple pages, got {total}"
        );
        let mut r = Reader::new(b, frame(1), Anchor::default()).unwrap();
        assert_eq!(r.page, 0);
        assert!(r.turn(true));
        assert_eq!(r.page, 1);
        assert_eq!(r.anchor, r.book.page(1).unwrap().start.into());
        while r.turn(true) {}
        assert_eq!(r.page, total - 1);
        assert!(r.state().at_end());
        assert!(!r.turn(true));
        assert!(r.turn(false));
        assert_eq!(r.page, total - 2);
        r.jump(0);
        assert_eq!(r.page, 0);
        assert_eq!(
            r.anchor,
            Anchor {
                block: 0,
                offset: 0
            }
        );
    }

    #[test]
    fn reader_turn_backwards_stops_at_start_and_jump_moves_anchor() {
        let b = sample_book();
        let mut r = Reader::new(b, frame(2), Anchor::default()).unwrap();
        assert!(!r.turn(false)); // Already on the first page.
        assert_eq!(r.page, 0);
        r.jump(5);
        assert_eq!(r.anchor.block, 5);
        assert_eq!(
            r.page,
            r.book.page_of(Position {
                block: 5,
                offset: 0
            })
        );
    }

    #[test]
    fn reader_notes_of_a_plain_book_are_empty() {
        let b = sample_book();
        let r = Reader::new(b, frame(1), Anchor::default()).unwrap();
        assert!(r.notes().is_empty());
    }
}
