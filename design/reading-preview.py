#!/usr/bin/env python3
"""Visual Ghostty mock. Uses Bookworm's actual rasterized glyphs and a disposable library."""
import fcntl
import os
import pty
import re
import select
import shutil
import struct
import subprocess
import tempfile
import termios
import time
import tty
from pathlib import Path
from xml.sax.saxutils import escape

TITLE = 'Северный берег'
AUTHOR = 'Алексей Ветров'
CHAPTERS = ['Письмо с острова', 'Маяк в тумане', 'Ночная переправа', 'Третий огонь', 'Возвращение']
PASSAGES = [
    'К рассвету ветер переменился. Туман шёл с воды, медленно закрывая камни, и маяк казался не выше рыбацкого сарая. С его площадки ещё можно было различить белую полосу прибоя, но дальше уже не было ни моря, ни неба.',
    'Я поставил фонарь на подоконник и развернул карту. Кто-то до меня провёл карандашом линию от мыса к проливу. Она обрывалась посреди чистого листа, как если бы человек, державший карандаш, вдруг услышал за дверью своё имя.',
    'Смотритель вошёл не постучав. Он снял плащ, повесил его на спинку стула и долго грел руки над жестяной кружкой. Потом спросил, видел ли я лодку. Я ответил, что слышал вёсла. Для него, кажется, это было совсем не одно и то же.',
    'Внизу, у самой воды, лежала бухта каната. Вчера она была сухой, а теперь с неё стекали капли. Смотритель наклонился и вытянул из-под витка короткую синюю ленту. Он бережно расправил её на ладони, словно письмо, которое нельзя прочесть дважды.',
    'Мы спустились к пристани. Доски ещё держали ночной холод, и от мачты тянуло солью. По ту сторону пролива поднималась полоска света. Я подумал, что это солнце, но полоска исчезла, а потом появилась снова, чуть левее прежнего места.',
    'У старого причала стояла лодка, которую я прежде видел только на фотографии. Краска на корме вздулась, и сквозь синие буквы проступало другое имя. Смотритель сказал, что до вечера никто не станет спрашивать, куда она делась. Я не сразу понял, что он предлагает отплыть.',
    'Туман начал редеть только над самой водой. Сначала проступили поплавки, потом цепь, натянутая от причала, потом чёрная кромка каменного мола. Ни одного человека вокруг не было видно, но с берега по-прежнему доносился мерный стук, будто кто-то ставил чашки на блюдца.',
    'На пороге смотритель остановился. «Если услышишь колокол, не оборачивайся», — сказал он. Так можно было предупредить о чём угодно: о ветре, о мелях или о человеке, которого ждёшь слишком долго. Я спрятал карту за пазуху и взялся за весло.',
]
ESC = '\x1b'
APC = re.compile(rb'\x1b_G([^;\x1b]+);([A-Za-z0-9+/=]*)\x1b\\')


def make_book(path):
    chapters = []
    for i, heading in enumerate(CHAPTERS):
        paragraphs = (PASSAGES * 3)[:17] if i == 1 else (PASSAGES[i:] + PASSAGES[:i])[:6]
        body = ''.join('<p>' + escape(p) + '</p>' for p in paragraphs)
        chapters.append(f'<section><title><p>{heading}</p></title>{body}</section>')
    path.write_text('<FictionBook xmlns="http://www.gribuser.ru/xml/fictionbook/2.0">'
                    '<description><title-info><author><first-name>Алексей</first-name>'
                    '<last-name>Ветров</last-name></author><book-title>' + TITLE +
                    '</book-title><lang>ru</lang></title-info></description><body>' +
                    ''.join(chapters) + '</body></FictionBook>', encoding='utf-8')


def render(root, cols, rows, cw, ch, color):
    file = root / 'demo.fb2'
    make_book(file)
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', rows, cols, cols*cw, rows*ch))
    binary = shutil.which('bookworm')
    if not binary:
        raise RuntimeError('Не найден bookworm в PATH')
    proc = subprocess.Popen([binary, file], stdin=slave, stdout=slave, stderr=slave,
                            env=dict(os.environ, XDG_DATA_HOME=str(root)))
    buf = b''

    def read_until(pattern):
        nonlocal buf
        limit = time.monotonic() + 5
        while not (m := re.search(pattern, buf)):
            if not select.select([master], [], [], max(0, limit-time.monotonic()))[0]:
                raise RuntimeError(f'Bookworm did not reach {pattern!r}: {buf[-160:]!r}')
            buf += os.read(master, 65536)
        out, buf = buf[:m.end()], buf[m.end():]
        return out, m

    try:
        _, m = read_until(rb'i=([0-9]+);AAAA\x1b\\')
        os.write(master, b'\x1b_Gi=' + m[1] + b';OK\x1b\\')
        read_until(rb'\x1b\]10;\?\x1b\\')
        os.write(master, ('\x1b]10;rgb:' + '/'.join(c*2 for c in re.findall('..', color)) + '\x1b\\').encode())
        foot = 'Q Выход'.encode() + rb'\x1b\[0m'
        read_until(foot)
        os.write(master, b't')
        read_until('Esc Вернуться к чтению'.encode() + rb'\x1b\[0m')
        os.write(master, b'\x1b[B\r')
        capture, _ = read_until(foot)
        ranges = re.findall(rb'(\d+)(?:\xe2\x80\x93(\d+))? / (\d+)\s+(\d+)%', capture)
        first, last, total, percent = [int(v or b'0') for v in ranges[-1]]
        chunks, images = [], []
        for m in APC.finditer(capture):
            opts = dict(p.split(b'=') for p in m[1].split(b','))
            if opts.get(b'a') == b'T':
                assert not chunks
                chunks = [m[0]]
            elif chunks:
                chunks.append(m[0])
            if chunks and opts.get(b'm') == b'0':
                images.append(b''.join(chunks))
                chunks = []
        assert len(images) == 2 and last == first + 1, (len(images), ranges)
        os.write(master, b'q')
        proc.wait(timeout=3)
        return images, first, last, total, percent
    finally:
        if proc.poll() is None:
            proc.kill()
            proc.wait()
        os.close(master)
        os.close(slave)


def main():
    rows, cols, width, height = struct.unpack('HHHH', fcntl.ioctl(1, termios.TIOCGWINSZ, bytes(8)))
    if cols < 160 or rows < 30:
        raise SystemExit('Для предпросмотра разверните Ghostty: от 160 столбцов и 30 строк.')
    if not width or not height:
        raise SystemExit('Запустите предпросмотр непосредственно в Ghostty.')
    cw, ch = round(width/cols), round(height/rows)
    side = 28
    left, right = side + 3, cols - 2
    top, bottom = 4, rows - 4
    inner = right - left - 1
    # Keep the mock aligned to the same one-cell gutter as Bookworm's actual spread.
    page_width = min((inner-1)//2, (40*24)//cw)
    start = left + 1 + (inner-(page_width*2+1))//2
    gutter = start + page_width
    original = termios.tcgetattr(0)
    out = os.fdopen(os.dup(1), 'wb')
    tty.setraw(0)

    def put(r, c, text, style='0'):
        out.write(f'{ESC}[{r};{c}H{ESC}[{style}m{text}{ESC}[0m'.encode())

    try:
        out.write(b'\x1b[?1049h\x1b[?25l\x1b[?7l\x1b[2J\x1b]10;?\x1b\\'); out.flush()
        answer = b''
        limit = time.monotonic() + .35
        while select.select([0], [], [], max(0, limit-time.monotonic()))[0]:
            answer += os.read(0, 1024)
            if b'\\' in answer or b'\x07' in answer:
                break
        m = re.search(rb'rgb:([0-9a-fA-F]{4})/([0-9a-fA-F]{4})/([0-9a-fA-F]{4})', answer)
        color = ''.join(x[:2].decode() for x in m.groups()) if m else 'f5f5f7'
        with tempfile.TemporaryDirectory() as folder:
            # Child Bookworm reserves three rows for its own chrome, which is not copied.
            imgs, first, last, total, percent = render(Path(folder), inner, bottom-top+1, cw, ch, color)
        put(2, 3, 'bookworm', '1')
        put(2, left, TITLE, '1')
        put(2, left+len(TITLE)+4, AUTHOR, '90')
        put(2, cols-9, 'Чтение', '90')
        put(top, 3, 'Оглавление', '1')
        put(top, side-1, 'T', '90')
        for i, chapter in enumerate(CHAPTERS):
            r = top + 3 + 2*i
            put(r, 3, f'{i+1:02}', '90')
            put(r, 6, ('▸ ' if i == 1 else '  ') + chapter, '1;36' if i == 1 else '0')
        put(bottom-1, 3, 'T  К разделам', '90')
        # Short marks at the outside edges suggest a spread without boxing it in.
        for row, mark_left, mark_right in [(top, '┌──', '──┐'), (bottom, '└──', '──┘')]:
            put(row, start-1, mark_left, '2;90')
            put(row, gutter+page_width-1, mark_right, '2;90')
        for col, png in zip((start, gutter+1), imgs):
            put(top+1, col, '')
            out.write(png)
        for col, num in [(start, first), (gutter+1, last)]:
            num = str(num)
            put(bottom, col + (page_width-len(num))//2, ' '+num+' ', '90')
        track = 27
        fill = round(percent * track/100)
        put(rows-2, left, '━'*fill, '36')
        put(rows-2, left+fill, '─'*(track-fill), '90')
        put(rows-2, left+track+2, f'{percent}%   {first}–{last} / {total}', '1')
        put(rows-2, right-23, '← →  Листать   ?  Клавиши', '90')
        out.flush()
        while os.read(0, 1) not in (b'q', b'Q', b'\x03', b'\x1b', b''):
            pass
    finally:
        out.write(b'\x1b[?7h\x1b[?25h\x1b[?1049l'); out.flush(); out.close()
        termios.tcsetattr(0, termios.TCSANOW, original)


if __name__ == '__main__':
    main()
