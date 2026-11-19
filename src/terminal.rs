use anyhow::{Context, Result, bail, ensure};
use bookworm::{
    graphics,
    reader::Window,
    theme::{self, Palette},
};
use std::{
    collections::VecDeque,
    io::{self, BufWriter, IsTerminal, Write},
    mem::MaybeUninit,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

static STOP: AtomicBool = AtomicBool::new(false);
static RESIZED: AtomicBool = AtomicBool::new(false);
extern "C" fn signal(sig: i32) {
    if sig == libc::SIGWINCH {
        RESIZED.store(true, Ordering::Relaxed);
    } else {
        STOP.store(true, Ordering::Relaxed);
    }
}
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Key {
    Char(char),
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    Home,
    End,
    Backspace,
    Enter,
    Esc,
    Resize,
    Exit,
    Unknown,
}

impl Key {
    pub fn command(self) -> Self {
        match self {
            Self::Char(c) => Self::Char(c.to_ascii_lowercase()),
            key => key,
        }
    }
}

pub struct Terminal {
    out: BufWriter<io::Stdout>,
    original: libc::termios,
    handlers: Vec<(i32, libc::sigaction)>,
    images: Vec<u32>,
    base: u32,
    input: VecDeque<u8>,
    pub window: Window,
    foreground: [u8; 3],
    pub palette: Option<Palette>,
}
impl Terminal {
    pub fn enter() -> Result<Self> {
        ensure!(
            io::stdin().is_terminal() && io::stdout().is_terminal(),
            "Откройте bookworm в интерактивном терминале Ghostty или Kitty"
        );
        unsafe {
            libc::setlocale(libc::LC_CTYPE, c"".as_ptr());
        }
        let mut original = MaybeUninit::uninit();
        if unsafe { libc::tcgetattr(0, original.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error().into());
        }
        let original = unsafe { original.assume_init() };
        let mut t = Self {
            out: BufWriter::with_capacity(1 << 21, io::stdout()),
            original,
            handlers: vec![],
            images: vec![],
            base: std::process::id() * 4 + 1,
            input: VecDeque::new(),
            window: Window::default(),
            foreground: bookworm_core::render::NIGHT.fg,
            palette: None,
        };
        for sig in [
            libc::SIGINT,
            libc::SIGTERM,
            libc::SIGHUP,
            libc::SIGQUIT,
            libc::SIGTSTP,
            libc::SIGWINCH,
        ] {
            let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
            action.sa_sigaction = signal as *const () as usize;
            let mut previous = MaybeUninit::uninit();
            if unsafe { libc::sigaction(sig, &action, previous.as_mut_ptr()) } != 0 {
                return Err(io::Error::last_os_error().into());
            }
            t.handlers.push((sig, unsafe { previous.assume_init() }));
        }
        let mut raw = original;
        unsafe {
            libc::cfmakeraw(&mut raw);
        }
        if unsafe { libc::tcsetattr(0, libc::TCSANOW, &raw) } != 0 {
            return Err(io::Error::last_os_error().into());
        }
        t.out
            .write_all(b"\x1b[?1049h\x1b[?25l\x1b[?7l\x1b[2J\x1b[H")?;
        graphics::query(&mut t.out, t.base)?;
        t.out.flush()?;
        t.wait_for_reply(Duration::from_millis(1600), |buf, id| {
            let end = buf.windows(2).position(|b| b == b"\x1b\\")? + 2;
            let start = buf[..end].windows(3).position(|b| b == b"\x1b_G")?;
            Some((start, end, graphics::probe_ok(&buf[start..end], id)))
        })?
        .filter(|ok| *ok)
        .context("Терминал не подтвердил Kitty Graphics. Откройте Ghostty или Kitty напрямую.")?;
        t.measure()?;
        t.out.write_all(b"\x1b]10;?\x1b\\")?;
        t.out.flush()?;
        if let Some(Some(color)) = t.wait_for_reply(Duration::from_millis(250), |buf, _| {
            let start = buf.windows(5).position(|b| b == b"\x1b]10;")?;
            let tail = &buf[start + 5..];
            let (end, terminator) = osc_end(tail)?;
            Some((start, start + 5 + end + terminator, parse_rgb(&tail[..end])))
        })? {
            t.foreground = color;
        }
        Ok(t)
    }
    fn wait_for_reply<T>(
        &mut self,
        timeout: Duration,
        parse: impl Fn(&[u8], u32) -> Option<(usize, usize, T)>,
    ) -> Result<Option<T>> {
        let deadline = Instant::now() + timeout;
        let mut buf = Vec::new();
        loop {
            if let Some((start, end, result)) = parse(&buf, self.base) {
                self.input.extend(&buf[..start]);
                self.input.extend(&buf[end..]);
                return Ok(Some(result));
            }
            if STOP.load(Ordering::Relaxed) {
                bail!("Чтение прервано");
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() || buf.len() > 8192 {
                self.input.extend(buf);
                return Ok(None);
            }
            buf.extend(read_available(left.min(Duration::from_millis(100)))?);
        }
    }
    fn measure(&mut self) -> Result<()> {
        let mut size: libc::winsize = unsafe { std::mem::zeroed() };
        if unsafe { libc::ioctl(1, libc::TIOCGWINSZ, &mut size) } != 0 {
            return Err(io::Error::last_os_error().into());
        }
        let (mut width, mut height) = (u32::from(size.ws_xpixel), u32::from(size.ws_ypixel));
        if width == 0 || height == 0 {
            self.out.write_all(b"\x1b[14t")?;
            self.out.flush()?;
            let dims = self
                .wait_for_reply(Duration::from_millis(800), |buf, _| {
                    let start = buf.windows(4).position(|b| b == b"\x1b[4;")?;
                    let tail = &buf[start + 4..];
                    let end = tail.iter().position(|&b| b == b't')?;
                    let (h, w) = std::str::from_utf8(&tail[..end]).ok()?.split_once(';')?;
                    Some((
                        start,
                        start + 5 + end,
                        (w.parse::<u32>().ok()?, h.parse::<u32>().ok()?),
                    ))
                })?
                .context("Терминал не сообщил размер области чтения в пикселях")?;
            (width, height) = dims;
        }
        ensure!(
            width > 0 && height > 0 && size.ws_col > 0 && size.ws_row > 0,
            "Пустая область терминала"
        );
        self.window = Window {
            cols: size.ws_col.into(),
            rows: size.ws_row.into(),
            width,
            height,
        };
        Ok(())
    }
    pub fn ink(&self) -> [u8; 3] {
        self.palette.map_or(self.foreground, |p| theme::rgb(p.fg))
    }
    fn background(&self) -> String {
        self.palette
            .map_or_else(|| "0".into(), |p| theme::sgr(48, p.bg))
    }
    fn style(&self, native: &str) -> String {
        let Some(p) = self.palette else {
            return native.into();
        };
        let (color, extra) = match native {
            "0" => (p.fg, ""),
            "1" => (p.fg, ";1"),
            "2" | "90" => (p.muted, ""),
            "36" => (p.accent, ""),
            "1;36" => (p.accent, ";1"),
            "2;90" => (p.muted, ";2"),
            "7" => (p.accent, ";7"),
            _ => return native.into(), // Explicit truecolor for theme swatches.
        };
        format!("{};{}{extra}", self.background(), theme::sgr(38, color))
    }
    pub fn clear(&mut self) -> Result<()> {
        for &id in &self.images {
            graphics::delete(&mut self.out, id)?;
        }
        self.images.clear();
        let bg = self.background();
        write!(self.out, "\x1b[0;{bg}m\x1b[2J\x1b[H\x1b[0m")?;
        Ok(())
    }
    pub fn image(&mut self, row: u32, col: u32, slot: u32, png: &[u8]) -> Result<u32> {
        let id = self.base + 1 + slot;
        if !self.images.contains(&id) {
            self.images.push(id);
        }
        write!(self.out, "\x1b[{row};{col}H")?;
        graphics::send(&mut self.out, id, png)?;
        Ok(id)
    }
    pub fn prune_images(&mut self, keep: &[u32]) -> Result<()> {
        let stale: Vec<u32> = self
            .images
            .iter()
            .copied()
            .filter(|id| !keep.contains(id))
            .collect();
        for id in &stale {
            graphics::delete(&mut self.out, *id)?;
        }
        self.images.retain(|id| keep.contains(id));
        Ok(())
    }
    /// Erase `width` cells of a row without touching Kitty images above them.
    pub fn erase(&mut self, row: u32, col: u32, width: u32) -> Result<()> {
        if row == 0 || row > self.window.rows || col == 0 || col > self.window.cols || width == 0 {
            return Ok(());
        }
        let width = width.min(self.window.cols - col + 1);
        let bg = self.background();
        write!(self.out, "\x1b[{row};{col}H\x1b[0;{bg}m\x1b[{width}X")?;
        Ok(())
    }
    /// Open a synchronized update: the terminal buffers the frame and applies it atomically,
    /// so erase-then-redraw never paints an intermediate blank screen. Ignored by terminals
    /// without DEC 2026, which fall back to the old behavior.
    pub fn begin_frame(&mut self) -> Result<()> {
        self.out.write_all(b"\x1b[?2026h")?;
        Ok(())
    }
    pub fn end_frame(&mut self) -> Result<()> {
        self.out.write_all(b"\x1b[?2026l")?;
        self.flush()
    }
    pub fn line(&mut self, row: u32, text: &str, style: &str) -> Result<()> {
        if row == 0 || row > self.window.rows {
            return Ok(());
        }
        let text = clip(text, self.window.cols);
        let bg = self.background();
        let style = self.style(style);
        write!(
            self.out,
            "\x1b[{row};1H\x1b[0;{bg}m\x1b[2K\x1b[{style}m{text}\x1b[0m"
        )?;
        Ok(())
    }
    pub fn text(&mut self, row: u32, col: u32, width: u32, text: &str, style: &str) -> Result<()> {
        if row == 0 || row > self.window.rows || col == 0 || col > self.window.cols {
            return Ok(());
        }
        let text = clip(text, width.min(self.window.cols - col + 1));
        let style = self.style(style);
        write!(self.out, "\x1b[{row};{col}H\x1b[{style}m{text}\x1b[0m")?;
        Ok(())
    }
    pub fn flush(&mut self) -> Result<()> {
        self.out.flush()?;
        Ok(())
    }
    pub fn copy_to_clipboard(&mut self, text: &str) -> Result<()> {
        self.out.write_all(b"\x1b]52;c;")?;
        graphics::write_base64(&mut self.out, text.as_bytes())?;
        self.out.write_all(b"\x1b\\")?;
        self.flush()
    }
    pub fn key(&mut self) -> Result<Key> {
        loop {
            if STOP.load(Ordering::Relaxed) {
                return Ok(Key::Exit);
            }
            if RESIZED.swap(false, Ordering::Relaxed) {
                self.measure()?;
                return Ok(Key::Resize);
            }
            if self.input.is_empty() {
                self.input
                    .extend(read_available(Duration::from_millis(150))?);
                continue;
            }
            let bytes = self.input.make_contiguous();
            if let Some((n, k)) = decode(bytes, false) {
                self.input.drain(..n);
                return Ok(k);
            }
            let more = read_available(Duration::from_millis(80))?;
            if more.is_empty() {
                let (n, k) = decode(self.input.make_contiguous(), true).unwrap();
                self.input.drain(..n);
                return Ok(k);
            }
            self.input.extend(more);
        }
    }
}
impl Drop for Terminal {
    fn drop(&mut self) {
        // End a potentially interrupted APC before sending any cleanup sequences.
        let _ = self.out.write_all(b"\x1b\\");
        for &id in &self.images {
            let _ = graphics::delete(&mut self.out, id);
        }
        let _ = self.out.write_all(b"\x1b[0m\x1b[?7h\x1b[?25h\x1b[?1049l");
        let _ = self.out.flush();
        unsafe {
            libc::tcsetattr(0, libc::TCSANOW, &self.original);
        }
        for (sig, prev) in self.handlers.drain(..).rev() {
            unsafe {
                libc::sigaction(sig, &prev, std::ptr::null_mut());
            }
        }
    }
}
fn read_available(timeout: Duration) -> io::Result<Vec<u8>> {
    let mut poll = libc::pollfd {
        fd: 0,
        events: libc::POLLIN,
        revents: 0,
    };
    let count = unsafe {
        libc::poll(
            &mut poll,
            1,
            timeout.as_millis().min(i32::MAX as u128) as i32,
        )
    };
    if count < 0 {
        let e = io::Error::last_os_error();
        if e.kind() == io::ErrorKind::Interrupted {
            return Ok(vec![]);
        }
        return Err(e);
    }
    if count == 0 {
        return Ok(vec![]);
    }
    let mut buf = [0u8; 1024];
    let n = unsafe { libc::read(0, buf.as_mut_ptr().cast(), buf.len()) };
    if n == 0 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "Терминал отключён",
        ));
    }
    if n < 0 {
        let e = io::Error::last_os_error();
        if e.kind() == io::ErrorKind::Interrupted {
            return Ok(vec![]);
        }
        return Err(e);
    }
    Ok(buf[..n as usize].to_vec())
}
fn osc_end(bytes: &[u8]) -> Option<(usize, usize)> {
    bytes.iter().enumerate().find_map(|(i, &b)| match b {
        0x07 => Some((i, 1)),
        0x1b if bytes.get(i + 1) == Some(&b'\\') => Some((i, 2)),
        _ => None,
    })
}

fn parse_rgb(reply: &[u8]) -> Option<[u8; 3]> {
    let mut parts = std::str::from_utf8(reply.strip_prefix(b"rgb:")?)
        .ok()?
        .split('/');
    let mut rgb = [0; 3];
    for channel in &mut rgb {
        let part = parts.next()?;
        if !(1..=4).contains(&part.len()) || !part.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let max = (1 << (4 * part.len())) - 1;
        let value = u32::from_str_radix(part, 16).ok()?;
        *channel = ((value * 255 + max / 2) / max) as u8;
    }
    parts.next().is_none().then_some(rgb)
}

fn decode(b: &[u8], expired: bool) -> Option<(usize, Key)> {
    if b[0] == 27 {
        if b.len() == 1 && !expired {
            return None;
        }
        if let Some(tail) = b.strip_prefix(b"\x1b]10;") {
            if let Some((end, terminator)) = osc_end(tail) {
                return Some((5 + end + terminator, Key::Unknown));
            }
            if !expired && tail.len() < 64 {
                return None;
            }
        }
        if b.len() >= 2 && matches!(b[1], b'[' | b'O') {
            if let Some(end) = b[2..]
                .iter()
                .position(|c| (0x40..=0x7e).contains(c))
                .map(|i| i + 2)
            {
                let k = match &b[2..=end] {
                    b"A" => Key::Up,
                    b"B" => Key::Down,
                    b"C" => Key::Right,
                    b"D" => Key::Left,
                    b"5~" => Key::PageUp,
                    b"6~" => Key::PageDown,
                    b"H" | b"1~" => Key::Home,
                    b"F" | b"4~" => Key::End,
                    _ => Key::Unknown,
                };
                return Some((end + 1, k));
            } else if !expired && b.len() < 64 {
                return None;
            }
        }
        return Some((1, Key::Esc));
    }
    let len = match b[0] {
        0..=127 => 1,
        194..=223 => 2,
        224..=239 => 3,
        240..=244 => 4,
        _ => 1,
    };
    if b.len() < len && !expired {
        return None;
    }
    let n = len.min(b.len());
    let k = match b[0] {
        3 | 4 | 26 => Key::Exit,
        10 | 13 => Key::Enter,
        8 | 127 => Key::Backspace,
        _ => std::str::from_utf8(&b[..n])
            .ok()
            .and_then(|s| s.chars().next())
            .filter(|c| !c.is_control())
            .map(Key::Char)
            .unwrap_or(Key::Unknown),
    };
    Some((n, k))
}

unsafe extern "C" {
    fn wcwidth(c: libc::wchar_t) -> i32;
}
pub fn clip(text: &str, cols: u32) -> String {
    let mut width = 0;
    let mut line = String::new();
    for c in text.chars() {
        let c = if c.is_control() { ' ' } else { c };
        let n = unsafe { wcwidth(c as libc::wchar_t) }.max(0) as u32;
        if width + n > cols {
            break;
        }
        width += n;
        line.push(c);
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_maps_csi_sequences_and_final_bytes() {
        assert_eq!(decode(b"\x1b[A", false), Some((3, Key::Up)));
        assert_eq!(decode(b"\x1b[B", false), Some((3, Key::Down)));
        assert_eq!(decode(b"\x1b[C", false), Some((3, Key::Right)));
        assert_eq!(decode(b"\x1b[D", false), Some((3, Key::Left)));
        assert_eq!(decode(b"\x1b[5~", false), Some((4, Key::PageUp)));
        assert_eq!(decode(b"\x1b[6~", false), Some((4, Key::PageDown)));
        assert_eq!(decode(b"\x1b[H", false), Some((3, Key::Home)));
        assert_eq!(decode(b"\x1b[1~", false), Some((4, Key::Home)));
        assert_eq!(decode(b"\x1b[F", false), Some((3, Key::End)));
        assert_eq!(decode(b"\x1b[4~", false), Some((4, Key::End)));
        assert_eq!(decode(b"\x1b[Z", false), Some((3, Key::Unknown)));
        // Parameterised sequence is swallowed whole, never split into pieces.
        assert_eq!(decode(b"\x1b[1;5A", false), Some((6, Key::Unknown)));
    }

    #[test]
    fn decode_waits_for_incomplete_sequences_and_expires() {
        assert_eq!(decode(b"\x1b", false), None);
        assert_eq!(decode(b"\x1b[", false), None);
        assert_eq!(decode(b"\x1b]10;rgb:ffff", false), None);
        assert_eq!(decode(b"\xd0", false), None); // First half of "Ж".
        assert_eq!(decode(b"\x1b", true), Some((1, Key::Esc)));
        assert_eq!(decode(b"\xd0", true), Some((1, Key::Unknown)));
    }

    #[test]
    fn decode_swallows_osc10_reply() {
        let reply: &[u8] = b"\x1b]10;rgb:ffff/ffff/ffff\x1b\\";
        assert_eq!(decode(reply, false), Some((reply.len(), Key::Unknown)));
    }

    #[test]
    fn decode_maps_control_bytes_and_utf8() {
        assert_eq!(decode(b"\r", false), Some((1, Key::Enter)));
        assert_eq!(decode(b"\n", false), Some((1, Key::Enter)));
        assert_eq!(decode(b"\x03", false), Some((1, Key::Exit)));
        assert_eq!(decode(b"\x04", false), Some((1, Key::Exit)));
        assert_eq!(decode(b"\x1a", false), Some((1, Key::Exit)));
        assert_eq!(decode(b"\x08", false), Some((1, Key::Backspace)));
        assert_eq!(decode(b"\x7f", false), Some((1, Key::Backspace)));
        assert_eq!(decode(b"a", false), Some((1, Key::Char('a'))));
        assert_eq!(decode("Ж".as_bytes(), false), Some((2, Key::Char('Ж'))));
        assert_eq!(decode(b"\x01", false), Some((1, Key::Unknown)));
        assert_eq!(decode(b"\xff", false), Some((1, Key::Unknown)));
    }

    #[test]
    fn command_lowercases_only_chars() {
        assert_eq!(Key::Char('A').command(), Key::Char('a'));
        assert_eq!(Key::Char('Ж').command(), Key::Char('Ж'));
        assert_eq!(Key::Up.command(), Key::Up);
    }

    #[test]
    fn clip_limits_by_display_columns() {
        // The app sets this in Terminal::enter; do the same for wcwidth() to
        // know about wide CJK cells and zero-width combining marks.
        unsafe { libc::setlocale(libc::LC_CTYPE, c"C.UTF-8".as_ptr()) };
        assert_eq!(clip("hello", 10), "hello");
        assert_eq!(clip("hello", 3), "hel");
        assert_eq!(clip("ab", 0), "");
        assert_eq!(clip("a字", 3), "a字"); // 字 takes two columns.
        assert_eq!(clip("字字", 3), "字");
        // Combining marks take no column of their own.
        assert_eq!(clip("a\u{301}b", 1), "a\u{301}");
        // Control bytes are replaced by spaces, never passed through.
        assert_eq!(clip("a\u{7}b", 5), "a b");
    }

    #[test]
    fn parse_rgb_scales_every_quanta_width() {
        assert_eq!(parse_rgb(b"rgb:ffff/8080/0000"), Some([255, 128, 0]));
        assert_eq!(parse_rgb(b"rgb:ff/00/7f"), Some([255, 0, 127]));
        assert_eq!(parse_rgb(b"rgb:f/8/0"), Some([255, 136, 0]));
    }

    #[test]
    fn parse_rgb_rejects_malformed_replies() {
        for bad in [
            &b""[..],
            b"rgb:",
            b"rgb:12/34",
            b"rgb:1/2/3/4",
            b"rgb:12345/1/1",
            b"rgb:gg/11/22",
            b"RGB:ff/00/ff",
        ] {
            assert_eq!(parse_rgb(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn osc_end_finds_bel_and_st_terminators() {
        assert_eq!(osc_end(b"rgb:ff\x07"), Some((6, 1)));
        assert_eq!(osc_end(b"rgb:ff\x1b\\"), Some((6, 2)));
        assert_eq!(osc_end(b"rgb:ff"), None);
    }
}
