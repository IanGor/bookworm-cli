use crate::terminal::Key;
use bookworm::reader::Reader;
use bookworm_core::layout::PlacedLine;

#[derive(Clone, Copy, Debug)]
pub struct Cursor {
    pub page: usize,
    pub line: usize,
    pub block: usize,
    pub offset: u32,
    pub x: f32,
}

impl Cursor {
    fn position(self) -> (usize, u32) {
        (self.block, self.offset)
    }
}

#[derive(Clone, Copy)]
pub struct Range {
    pub start: Cursor,
    pub end: Cursor,
}

impl Range {
    pub fn between(a: Cursor, b: Cursor) -> Option<Self> {
        match a.position().cmp(&b.position()) {
            std::cmp::Ordering::Less => Some(Self { start: a, end: b }),
            std::cmp::Ordering::Greater => Some(Self { start: b, end: a }),
            std::cmp::Ordering::Equal => None,
        }
    }

    pub fn overlaps(self, block: usize, start: u32, end: u32) -> bool {
        self.start.position() < (block, end) && (block, start) < self.end.position()
    }
}

#[derive(Clone, Copy)]
pub struct Preview {
    pub first_page: usize,
    pub cursor: Cursor,
    pub range: Option<Range>,
}

pub fn glyphs(line: &PlacedLine) -> impl Iterator<Item = (u32, u32, f32, f32)> {
    line.runs
        .iter()
        .flat_map(|run| &run.glyphs)
        .filter_map(|g| {
            g.source
                .map(|(a, b)| (a, b, line.x + g.left, line.x + g.right))
        })
}

fn stops(r: &Reader, page: usize) -> Vec<Cursor> {
    let Some(p) = r.book.page(page) else {
        return Vec::new();
    };
    let mut result = Vec::new();
    for (index, line) in p.lines.iter().enumerate() {
        let start = result.len();
        for (a, b, left, right) in glyphs(line) {
            for (offset, x) in [(a, left), (b, right)] {
                result.push(Cursor {
                    page,
                    line: index,
                    block: line.block,
                    offset,
                    x,
                });
            }
        }
        result[start..].sort_by_key(|c| c.offset);
    }
    result.dedup_by_key(|c| (c.line, c.offset));
    result
}

pub struct Selection {
    pub anchor: Option<Cursor>,
    first_page: usize,
    stops: Vec<Cursor>,
    index: usize,
}

impl Selection {
    pub fn new(r: &Reader) -> Option<Self> {
        for page in r.page..(r.page + r.frame.pages).min(r.state().total) {
            let stops = stops(r, page);
            if !stops.is_empty() {
                return Some(Self {
                    anchor: None,
                    first_page: r.page,
                    stops,
                    index: 0,
                });
            }
        }
        None
    }

    pub fn cursor(&self) -> Cursor {
        self.stops[self.index]
    }

    pub fn preview(&self) -> Preview {
        let cursor = self.cursor();
        Preview {
            first_page: self.first_page,
            cursor,
            range: self.anchor.and_then(|a| Range::between(a, cursor)),
        }
    }

    pub fn move_cursor(&mut self, r: &Reader, key: Key) {
        let c = self.cursor();
        let first = self.stops.partition_point(|s| s.line < c.line);
        let after = self.stops.partition_point(|s| s.line <= c.line);
        let nearest = |slice: &[Cursor]| {
            slice
                .iter()
                .enumerate()
                .min_by(|(_, a), (_, b)| (a.x - c.x).abs().total_cmp(&(b.x - c.x).abs()))
                .map_or(0, |(i, _)| i)
        };
        self.index = match key {
            Key::Home => first,
            Key::End => after - 1,
            Key::Right if self.index + 1 < self.stops.len() => self.index + 1,
            Key::Left if self.index > 0 => self.index - 1,
            Key::Up if first > 0 => {
                let line = self.stops[first - 1].line;
                let start = self.stops.partition_point(|s| s.line < line);
                start + nearest(&self.stops[start..first])
            }
            Key::Down if after < self.stops.len() => {
                let line = self.stops[after].line;
                let end = self.stops.partition_point(|s| s.line <= line);
                after + nearest(&self.stops[after..end])
            }
            Key::Left | Key::Right | Key::Up | Key::Down | Key::PageUp | Key::PageDown => {
                let forward = matches!(key, Key::Right | Key::Down | Key::PageDown);
                let step = if forward { 1 } else { -1 };
                let mut page = c.page;
                loop {
                    let Some(next) = page
                        .checked_add_signed(step)
                        .filter(|&n| n < r.state().total)
                    else {
                        return;
                    };
                    page = next;
                    let candidates = stops(r, page);
                    if candidates.is_empty() {
                        continue;
                    }
                    let i = match key {
                        Key::Left => candidates.len() - 1,
                        Key::Right | Key::PageUp | Key::PageDown => 0,
                        Key::Down => nearest(
                            &candidates
                                [..candidates.partition_point(|s| s.line == candidates[0].line)],
                        ),
                        Key::Up => {
                            let last = candidates.last().unwrap().line;
                            let start = candidates.partition_point(|s| s.line < last);
                            start + nearest(&candidates[start..])
                        }
                        _ => unreachable!(),
                    };
                    self.stops = candidates;
                    if page < self.first_page {
                        self.first_page = page;
                    } else if page >= self.first_page + r.frame.pages {
                        self.first_page = page + 1 - r.frame.pages;
                    }
                    break i;
                }
            }
            _ => return,
        };
    }
}
