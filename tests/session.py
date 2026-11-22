"""A minimal Ghostty/Kitty peer. Inspect the actual bytes written by the CLI."""
import base64
import fcntl
import hashlib
import os
from pathlib import Path
import pty
import re
import select
import shlex
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time
import tomllib
import zlib

EXE = sys.argv[1]
APC = re.compile(rb'\x1b_G([^;\x1b]+);([A-Za-z0-9+/=]*)\x1b\\')
FOOT = r'\[\?\] Клавиши'
THEME_FOOT = r'Esc Отмена'
ENGINE = Path(__file__).resolve().parents[2] / "BookwormEngine"

class Session:
    def __init__(self, root, *args, pixels=True):
        self.master, self.slave = pty.openpty()
        self.original = termios.tcgetattr(self.slave)
        self.size(140, 32, 1400 if pixels else 0, 640 if pixels else 0)
        self.proc = subprocess.Popen([EXE, *args], stdin=self.slave, stdout=self.slave,
                                     stderr=self.slave, env=dict(os.environ, HOME=str(root),
                                     XDG_DATA_HOME=str(root / 'data'),
                                     WAYLAND_DISPLAY=str(root / 'no-wayland')))
        self.buffer = b''

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        if self.proc.poll() is None:
            self.proc.kill()
        self.proc.wait()
        os.close(self.master)
        os.close(self.slave)

    def size(self, cols, rows, width, height):
        fcntl.ioctl(self.slave, termios.TIOCSWINSZ, struct.pack('HHHH', rows, cols, width, height))

    def send(self, value):
        os.write(self.master, value.encode() if isinstance(value, str) else value)

    def until(self, pattern):
        pattern = pattern.encode() if isinstance(pattern, str) else pattern
        rx = re.compile(pattern)
        deadline = time.monotonic() + 15
        while True:
            found = rx.search(self.buffer)
            if found:
                data, self.buffer = self.buffer[:found.end()], self.buffer[found.end():]
                return data
            left = deadline - time.monotonic()
            if left <= 0 or not select.select([self.master], [], [], max(0, left))[0]:
                raise AssertionError(f"Timeout waiting for {pattern!r}, tail {self.buffer[-240:]!r}")
            self.buffer += os.read(self.master, 65536)

    def probe(self, result='OK'):
        command = self.until(rb'i=[0-9]+;AAAA\x1b\\')
        assert b'\x1b[?1049h' in command
        iid = re.search(rb'i=([0-9]+);AAAA', command)[1].decode()
        self.send(f'\x1b_Gi={iid};{result}\x1b\\')

    def restored(self, code):
        self.until(rb'\x1b\[\?1049l')
        assert self.proc.wait(timeout=5) == code
        assert termios.tcgetattr(self.slave) == self.original

def images(data):
    result = []
    encoded = b''
    for header, payload in APC.findall(data):
        opts = dict(x.split(b'=') for x in header.split(b','))
        if opts.get(b'a') == b'T':
            assert not encoded and opts[b'f'] == b'100' and opts[b't'] == b'd'
        encoded += payload
        assert len(payload) <= 4096
        if opts[b'm'] == b'0':
            png = base64.b64decode(encoded, validate=True)
            assert png.startswith(b'\x89PNG\r\n\x1a\n')
            assert png[-8:-4] == b'IEND'
            assert png[24:26] == b'\x08\x06', 'The PNG must carry RGBA pixels'
            compressed, offset = bytearray(), 8
            while offset + 12 <= len(png):
                size = struct.unpack_from('>I', png, offset)[0]
                if png[offset + 4:offset + 8] == b'IDAT':
                    compressed.extend(png[offset + 8:offset + 8 + size])
                offset += 12 + size
            # The first pixel in a PNG has no predictor for any filter type.
            assert zlib.decompress(compressed)[1:5] == b'\0' * 4, 'The margin must show the terminal background'
            result.append(struct.unpack('>II', png[16:24]))
            encoded = b''
    assert not encoded
    return result

def record(root):
    data = tomllib.loads((root / 'data/bookworm/library.toml').read_text())
    return data, next(iter(data['books'].values()))

with tempfile.TemporaryDirectory() as tmp:
    root = Path(tmp)
    books = root / 'Книги' / 'Вложенная'
    books.mkdir(parents=True)
    path = books / 'example.fb2'
    path.write_text('<FictionBook xmlns:l="http://www.w3.org/1999/xlink"><description><title-info>'
                    '<book-title>Проверка экрана</book-title></title-info></description><body>'
                    '<section><title><p>Начало</p></title><p>Ссылка<a type="note" l:href="#n">1</a></p>'
                    + '<p>' + 'Длинный абзац для проверки позиции чтения. ' * 120 + '</p></section>'
                    '<section><title><p>Конец</p></title><p>Последняя строка.</p></section></body>'
                    '<body name="notes"><section id="n"><p>Текст сноски.</p></section></body></FictionBook>')
    with Session(root, path, pixels=False) as s:
        s.size(172, 32, 0, 0)
        s.probe()
        s.until(rb'\x1b\[14t')
        s.send('\x1b[4;640;1720t')
        s.until(rb'\x1b\]10;\?\x1b\\')
        s.send('\x1b]10;rgb:2222/6699/aaff\x1b\\')
        screen = s.until(FOOT)
        assert '[←] [→] Листать'.encode() in screen
        before_theme = record(root)[1]['anchor']
        s.send('S')
        assert 'Бионическое чтение   Выкл.'.encode() in s.until('Esc Назад')
        s.send('\r')
        assert 'Бионическое чтение   Вкл.'.encode() in s.until('Esc Назад')
        s.send('\x1b')
        bionic = s.until(FOOT)
        assert record(root)[0].get('bionic') is True
        assert record(root)[1]['anchor'] == before_theme
        assert [m[1] for m in APC.findall(bionic)] != [m[1] for m in APC.findall(screen)]
        s.send('t')
        toc_hint = 'Enter Перейти · Esc Назад'
        assert toc_hint.encode() in s.until(FOOT)
        s.send('g')
        regular = s.until(FOOT)
        assert record(root)[0].get('bionic') is False
        assert record(root)[1]['anchor'] == before_theme
        assert toc_hint.encode() in regular
        assert [m[1] for m in APC.findall(regular)] == [m[1] for m in APC.findall(screen)]
        s.send('\x1b')
        assert toc_hint.encode() not in s.until(FOOT)
        s.send('Y')
        popup = s.until(THEME_FOOT)
        for name in ('Цветовые темы', 'Как в терминале', 'Янтарь', 'Светлый янтарь'):
            assert name.encode() in popup
        assert b'One Dark Pro' not in popup
        assert 'Лампа'.encode() not in popup
        s.send('\x1b[B')
        s.until(THEME_FOOT)
        s.send('\x1b')
        assert b'\x1b[2J' in s.until(FOOT), 'Canceling the theme picker must clear its text'
        assert record(root)[0].get('theme', 'terminal') == 'terminal'
        s.send('y')
        s.until(THEME_FOOT)
        s.send('\x1b[B')
        s.until(THEME_FOOT)
        s.send('\r')
        amber = s.until(FOOT)
        assert b'48;2;33;28;25' in amber and b'38;2;239;180;93' in amber
        assert b'38;2;189;169;142' in amber
        assert images(amber) == [(680, 500)] * 2
        assert record(root)[0]['theme'] == 'amber'
        assert record(root)[1]['anchor'] == before_theme
        s.send('y')
        s.until(THEME_FOOT)
        s.send('\r')
        assert b'\x1b[2J' in s.until(FOOT), 'Reapplying the same theme must clear the picker'
        s.send('?')
        s.until('Esc Вернуться к чтению')
        s.send('\x1b')
        assert b'\x1b[2J' in s.until(FOOT), 'Closing help must clear its text'
        s.send('y')
        s.until(THEME_FOOT)
        s.send('\x1b[H')
        s.until(THEME_FOOT)
        s.send('\r')
        assert b'48;2;' not in s.until(FOOT)
        assert record(root)[0].get('theme', 'terminal') == 'terminal'
        assert '┌──'.encode() in screen and '──┘'.encode() in screen
        reports = root / 'data/bookworm/reports'
        s.send('d')
        s.until('Enter Начало')
        s.send('\x1b')
        s.until(FOOT)
        assert not reports.exists()
        s.send('d')
        s.until('Enter Начало')
        for _ in range(2):
            s.send('\x1b[6~')
            s.until('Enter Начало')
        s.send('\x1b[C')
        s.until('Enter Начало')
        s.send('\r')
        s.until('Enter Описать')
        s.send('\x1b[5~')
        s.until('Enter Описать')
        s.send('\r')
        s.until('Enter Сохранить')
        s.send('Слишком широкие пробелы\r')
        s.until('Промпт сохранён')
        saved = s.until('Назад')
        assert b'hermes chat --query-file' in saved
        report, = reports.glob('*.md')
        s.send('c')
        copy_frame = s.until('Назад')
        osc = re.search(rb'\x1b\]52;c;([A-Za-z0-9+/=]+)\x1b\\', copy_frame)
        assert osc, 'Copy must go through the terminal, without a desktop helper'
        assert shlex.split(base64.b64decode(osc[1]).decode()) == [
            'hermes', 'chat', '--query-file', str(report)]
        text = report.read_text()
        assert str(path.resolve()) in text and 'Слишком широкие пробелы' in text
        assert re.search(r'Начало: страница 2, блок \d+, смещение \d+', text), text
        assert re.search(r'Конец: страница 3, блок \d+, смещение \d+', text), text
        assert 'Длинный абзац' in text and 'Ссылка' not in text
        assert hashlib.sha256(path.read_bytes()).hexdigest() in text
        assert report.stat().st_mode & 0o777 == 0o600
        assert record(root)[1]['anchor'] == before_theme
        s.send('\r')
        s.until(FOOT)
        assert '│'.encode() not in screen and 'Оглавление'.encode() in screen
        pages = images(screen)
        assert pages == [(680, 500)] * 2, pages
        assert b"\x1b[5;33H" in screen and b"\x1b[5;102H" in screen
        s.send("T")
        focused = s.until(FOOT)
        assert "Enter Перейти".encode() in focused and len(images(focused)) == 2
        s.send("\x1b[B")
        s.until(FOOT)
        s.send("\r")
        moved = s.until(FOOT)
        assert len(images(moved)) == 1 and record(root)[1]['anchor']['block'] > 0
        s.send("T")
        s.until(FOOT)
        s.send("\x1b[H")
        s.until(FOOT)
        s.send("\r")
        assert len(images(s.until(FOOT))) == 2
        s.send("T")
        s.until(FOOT)
        before_cancel = record(root)[1]['anchor']
        s.send("\x1b[B")
        s.until(FOOT)
        s.send("\x1b")
        s.until(FOOT)
        assert record(root)[1]['anchor'] == before_cancel
        s.size(140, 32, 1400, 640)
        s.proc.send_signal(signal.SIGWINCH)
        compact = s.until(FOOT)
        assert "Оглавление".encode() not in compact and len(images(compact)) == 2
        s.send('N')
        s.until('Enter Читать сноску')
        s.send('\r')
        assert len(images(s.until('Листать сноску'))) == 1
        s.send('\x1b[B')
        note_scroll = s.until('Листать сноску')
        assert b'a=d,d=I' not in note_scroll and b'\x1b[2J' not in note_scroll
        assert b'\x1b[?2026h' in note_scroll
        s.until(rb'\x1b\[\?2026l')
        s.send('\x1b')
        assert b'\x1b[2J' in s.until(FOOT), 'Leaving a footnote must redraw the book screen'
        s.send('e')
        s.until(FOOT)
        assert record(root)[1]['completions'] == []
        s.send('\x1b[C')
        turn = s.until(FOOT)
        assert b'a=d,d=I' not in turn, 'A page turn must not delete the page images'
        assert b'\x1b[2J' not in turn, 'A page turn must not erase the whole screen'
        assert b'\x1b[?2026h' in turn and b'a=T' in turn, 'The turn must be one synchronized frame'
        s.until(rb'\x1b\[\?2026l')
        assert len(images(turn)) == 2
        anchor = record(root)[1]['anchor']
        assert anchor['offset'] > 0, anchor
        s.send('b')
        s.until('Q Выход')
        s.send('d')
        s.until('Backspace Стереть')
        s.send('~/Книги\r')
        assert b'o~' in s.until('Q Выход')
        s.send('\r')
        s.until(FOOT)
        assert record(root)[1]['anchor'] == anchor
        s.send('v')
        assert len(images(s.until(FOOT))) == 1
        assert record(root)[1]['anchor'] == anchor
        s.size(72, 32, 720, 640)
        s.proc.send_signal(signal.SIGWINCH)
        assert images(s.until(FOOT)) == [(680, 500)]
        s.send('+')
        s.until(FOOT)
        assert record(root)[0]['size'] == 26 and record(root)[1]['anchor'] == anchor
        s.send('F')
        s.until('D Папка шрифтов')
        s.send('d')
        s.until('Backspace Стереть')
        (root / 'Fonts').mkdir()
        (root / 'Fonts/Test.otf').symlink_to(ENGINE / 'fonts/PTSerif-Regular.ttf')
        s.send('~/Fonts\r')
        s.until('своя гарнитура')
        s.send('\r')
        s.until(FOOT)
        assert record(root)[0]['font'].startswith('custom:')
        assert record(root)[1]['anchor'] == anchor
        s.send('T')
        toc_list = s.until('Перейти к разделу')
        assert b'\x1b[?2026h' in toc_list, 'Text screens must redraw inside a synchronized frame'
        s.send('\x1b[F\r')
        s.until('E Завершить книгу')
        s.until(FOOT)
        assert record(root)[1]['progress'] == 100 and not record(root)[1]['completions']
        for count in (1, 2):
            s.send('E')
            s.until('Esc Отмена')
            s.send('y')
            s.until(FOOT)
            dates = record(root)[1]['completions']
            assert len(set(dates)) == count
        s.send('b')
        s.until('D Папка')
        s.send('D')
        s.until('Backspace Стереть')
        s.send('~/Книги\r')
        s.until('Проверка экрана')
        s.until('Q Выход')
        assert Path(record(root)[0]['folder']) == books.parent
        s.send('Y')
        s.until(THEME_FOOT)
        s.send('\x1b[F')
        s.until(THEME_FOOT)
        s.send('\r')
        light_amber = s.until('Q Выход')
        assert b'48;2;255;245;227' in light_amber and b'38;2;133;71;5' in light_amber
        assert record(root)[0]['theme'] == 'light-amber'
        s.send('q')
        s.restored(0)

    with Session(root, path) as s:
        s.probe()
        light_amber = s.until(FOOT)
        assert b'48;2;255;245;227' in light_amber and b'38;2;48;37;27' in light_amber
        assert images(light_amber)
        s.send('G')
        before_restart = APC.findall(s.until(FOOT))
        assert before_restart
        s.proc.send_signal(signal.SIGTERM)
        s.restored(0)

    with Session(root) as s:
        s.probe('EINVAL')
        s.restored(1)

    with Session(root, path) as s:
        s.probe()
        after_restart = APC.findall(s.until(FOOT))
        assert record(root)[0]['bionic'] is True
        assert [payload for _, payload in after_restart] == [payload for _, payload in before_restart]
        db = root / 'data/bookworm/library.toml'
        db.write_text('external = [')
        s.send('\x1b[D')
        s.restored(1)
        assert db.read_text() == 'external = ['
