//! Kitty graphics protocol. One APC per 3072 PNG bytes (4096 base64 bytes).
use std::io::{self, Write};

pub fn query(w: &mut impl Write, id: u32) -> io::Result<()> {
    write!(w, "\x1b_Ga=q,t=d,f=24,s=1,v=1,i={id};AAAA\x1b\\")
}
pub fn probe_ok(response: &[u8], id: u32) -> bool {
    let expected = format!("\x1b_Gi={id};OK\x1b\\");
    response
        .windows(expected.len())
        .any(|s| s == expected.as_bytes())
}
pub fn delete(w: &mut impl Write, id: u32) -> io::Result<()> {
    write!(w, "\x1b_Ga=d,d=I,i={id},q=2\x1b\\")
}
pub fn write_base64(w: &mut impl Write, bytes: &[u8]) -> io::Result<()> {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    for chunk in bytes.chunks(3072) {
        let mut encoded = [0; 4096];
        for (group, dest) in chunk.chunks(3).zip(encoded.as_chunks_mut::<4>().0) {
            let a = group[0];
            let b = group.get(1).copied().unwrap_or(0);
            let c = group.get(2).copied().unwrap_or(0);
            dest[0] = ALPHABET[(a >> 2) as usize];
            dest[1] = ALPHABET[((a & 3) << 4 | b >> 4) as usize];
            dest[2] = if group.len() > 1 {
                ALPHABET[((b & 15) << 2 | c >> 6) as usize]
            } else {
                b'='
            };
            dest[3] = if group.len() > 2 {
                ALPHABET[(c & 63) as usize]
            } else {
                b'='
            };
        }
        w.write_all(&encoded[..chunk.len().div_ceil(3) * 4])?;
    }
    Ok(())
}

pub fn send(w: &mut impl Write, id: u32, png: &[u8]) -> io::Result<()> {
    let mut chunks = png.chunks(3072).enumerate().peekable();
    while let Some((i, chunk)) = chunks.next() {
        w.write_all(b"\x1b_G")?;
        if i == 0 {
            write!(w, "a=T,t=d,f=100,i={id},C=1,q=2,")?;
        }
        write!(w, "m={};", u8::from(chunks.peek().is_some()))?;
        write_base64(w, chunk)?;
        w.write_all(b"\x1b\\")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encoded(bytes: &[u8]) -> String {
        let mut out = Vec::new();
        write_base64(&mut out, bytes).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn write_base64_matches_rfc4648_vectors() {
        for (input, expected) in [
            (&b""[..], ""),
            (&b"f"[..], "Zg=="),
            (&b"fo"[..], "Zm8="),
            (&b"foo"[..], "Zm9v"),
            (&b"foob"[..], "Zm9vYg=="),
            (&b"fooba"[..], "Zm9vYmE="),
            (&b"foobar"[..], "Zm9vYmFy"),
        ] {
            assert_eq!(encoded(input), expected);
        }
    }

    #[test]
    fn send_wraps_a_small_image_in_one_apc() {
        let mut out = Vec::new();
        send(&mut out, 7, b"\x89PNG").unwrap();
        assert_eq!(out, b"\x1b_Ga=T,t=d,f=100,i=7,C=1,q=2,m=0;iVBORw==\x1b\\");
    }

    #[test]
    fn send_chunks_large_images_with_m_flags() {
        let png: Vec<u8> = (0..3073).map(|i| (i % 251) as u8).collect();
        let mut out = Vec::new();
        send(&mut out, 42, &png).unwrap();
        let s = String::from_utf8(out).unwrap();
        let parts: Vec<&str> = s.split("\x1b\\").filter(|p| !p.is_empty()).collect();
        assert_eq!(parts.len(), 2);
        assert!(parts[0].starts_with("\x1b_Ga=T,t=d,f=100,i=42,C=1,q=2,m=1;"));
        // 3072 bytes encode to exactly 4096 base64 characters.
        assert_eq!(parts[0].rsplit(';').next().unwrap().len(), 4096);
        assert_eq!(parts[1], "\x1b_Gm=0;PA==");
    }

    #[test]
    fn query_delete_and_probe_use_expected_frames() {
        let mut out = Vec::new();
        query(&mut out, 5).unwrap();
        assert_eq!(out, b"\x1b_Ga=q,t=d,f=24,s=1,v=1,i=5;AAAA\x1b\\");
        out.clear();
        delete(&mut out, 9).unwrap();
        assert_eq!(out, b"\x1b_Ga=d,d=I,i=9,q=2\x1b\\");
        let ok: &[u8] = b"\x1b_Gi=42;OK\x1b\\";
        assert!(probe_ok(ok, 42));
        assert!(!probe_ok(ok, 43));
        assert!(!probe_ok(b"\x1b_Gi=42;EBADF\x1b\\", 42));
    }
}
