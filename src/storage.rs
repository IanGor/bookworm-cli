use anyhow::{Context, Result, bail, ensure};
use bookworm_core::model::Position;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    env,
    fmt::Write as _,
    fs::{self, DirBuilder, File, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

#[link(name = "crypto")]
unsafe extern "C" {
    fn SHA256(data: *const u8, len: usize, digest: *mut u8) -> *mut u8;
}

pub fn content_id(bytes: &[u8]) -> String {
    let mut digest = [0; 32];
    // OpenSSL writes SHA256_DIGEST_LENGTH bytes to the caller's buffer.
    unsafe {
        SHA256(bytes.as_ptr(), bytes.len(), digest.as_mut_ptr());
    }
    let mut result = String::with_capacity(64);
    for byte in digest {
        write!(result, "{byte:02x}").unwrap();
    }
    result
}

pub fn now() -> String {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let seconds = elapsed.as_secs() as libc::time_t;
    let mut date = std::mem::MaybeUninit::uninit();
    // gmtime_r initializes the caller-owned result for current system time.
    assert!(!unsafe { libc::gmtime_r(&seconds, date.as_mut_ptr()) }.is_null());
    let t = unsafe { date.assume_init() };
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:09}Z",
        t.tm_year + 1900,
        t.tm_mon + 1,
        t.tm_mday,
        t.tm_hour,
        t.tm_min,
        t.tm_sec,
        elapsed.subsec_nanos()
    )
}

pub fn parse_utc(t: &str) -> Option<(i64, u32)> {
    if !t.ends_with('Z') {
        return None;
    }
    let dt: toml::value::Datetime = t.parse().ok()?;
    let (d, time) = (dt.date?, dt.time?);
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    tm.tm_year = i32::from(d.year) - 1900;
    tm.tm_mon = i32::from(d.month) - 1;
    tm.tm_mday = i32::from(d.day);
    tm.tm_hour = i32::from(time.hour);
    tm.tm_min = i32::from(time.minute);
    tm.tm_sec = i32::from(time.second);
    let secs = unsafe { libc::timegm(&mut tm) };
    Some((secs as i64, time.nanosecond))
}

#[derive(Default, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Anchor {
    pub block: u32,
    pub offset: u32,
}
impl From<Position> for Anchor {
    fn from(p: Position) -> Self {
        Self {
            block: p.block,
            offset: p.offset,
        }
    }
}
impl From<Anchor> for Position {
    fn from(p: Anchor) -> Self {
        Self {
            block: p.block,
            offset: p.offset,
        }
    }
}

#[derive(Default, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Record {
    pub source_title: String,
    pub source_author: String,
    pub title_override: Option<String>,
    pub author_override: Option<String>,
    pub anchor: Anchor,
    pub progress: u8,
    pub last_read: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub rereading: bool,
    pub completions: Vec<String>,
}
impl Record {
    pub fn title(&self) -> &str {
        self.title_override.as_deref().unwrap_or(&self.source_title)
    }
    pub fn author(&self) -> &str {
        self.author_override
            .as_deref()
            .unwrap_or(&self.source_author)
    }
    pub fn completed_at(&self) -> Option<&str> {
        if self.rereading {
            None
        } else {
            self.completions
                .iter()
                .max_by_key(|t| parse_utc(t))
                .map(String::as_str)
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Database {
    version: u32,
    pub folder: Option<PathBuf>,
    pub font_folder: Option<PathBuf>,
    pub font: String,
    pub size: u16,
    pub single_page: bool,
    #[serde(default)]
    pub bionic: bool,
    #[serde(default = "crate::theme::default_id")]
    pub theme: String,
    #[serde(default)]
    pub books: BTreeMap<String, Record>,
}
impl Default for Database {
    fn default() -> Self {
        Self {
            version: 1,
            folder: None,
            font_folder: None,
            font: "PT Serif".into(),
            size: 24,
            single_page: false,
            bionic: false,
            theme: crate::theme::default_id(),
            books: BTreeMap::new(),
        }
    }
}
impl Database {
    fn validate(&self) -> Result<()> {
        ensure!(self.version == 1, "Неизвестная версия базы");
        ensure!(
            (12..=72).contains(&self.size),
            "Размер шрифта должен быть от 12 до 72 пикселей"
        );
        for (id, r) in &self.books {
            ensure!(
                id.len() == 64
                    && id
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
                "Неверный SHA-256 книги"
            );
            ensure!(r.progress <= 100, "Прогресс вне диапазона 0–100");
            for t in r.last_read.iter().chain(&r.completions) {
                let d: toml::value::Datetime = t.parse().context("Некорректная дата чтения")?;
                ensure!(
                    d.date.is_some() && d.time.is_some() && t.ends_with('Z'),
                    "Нужна дата и время UTC с суффиксом Z"
                );
            }
        }
        Ok(())
    }
}

pub fn data_path() -> Result<PathBuf> {
    let root = env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| {
            env::var_os("HOME")
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .map(|p| p.join(".local/share"))
        })
        .context("Не заданы HOME и XDG_DATA_HOME")?;
    Ok(root.join("bookworm/library.toml"))
}

#[derive(PartialEq)]
pub(crate) struct Snapshot {
    dev: u64,
    ino: u64,
    pub bytes: Vec<u8>,
}
pub(crate) fn snapshot(path: &Path) -> Result<Option<Snapshot>> {
    let mut file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let meta = file.metadata()?;
    ensure!(meta.is_file(), "База должна быть обычным файлом");
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(Some(Snapshot {
        dev: meta.dev(),
        ino: meta.ino(),
        bytes,
    }))
}
const CONFLICT: &str =
    "Файл базы изменён извне. Сохранение остановлено. Откройте библиотеку заново.";

pub(crate) fn publish(
    path: &Path,
    original: &mut Option<Snapshot>,
    bytes: Vec<u8>,
    dir_mode: Option<u32>,
    conflict: &str,
) -> Result<()> {
    let parent = path.parent().context("Путь базы без каталога")?;
    if let Some(mode) = dir_mode {
        DirBuilder::new()
            .recursive(true)
            .mode(mode)
            .create(parent)?;
    } else {
        fs::create_dir_all(parent)?;
    }
    let dir = File::open(parent)?;
    dir.lock()?; // Serializes compare + publish across reader processes, even on first save.
    ensure!(snapshot(path)?.as_ref() == original.as_ref(), "{conflict}");
    if original.as_ref().is_some_and(|s| s.bytes == bytes) {
        return Ok(());
    }
    let tmp = parent.join(format!(".bookworm-{}-{}.tmp", std::process::id(), now()));
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)?;
    let mut remove_tmp = true;
    let result = (|| -> Result<()> {
        f.write_all(&bytes)?;
        f.sync_all()?;
        let meta = f.metadata()?;
        ensure!(snapshot(path)?.as_ref() == original.as_ref(), "{conflict}");
        if original.is_none() {
            fs::hard_link(&tmp, path).with_context(|| conflict.to_owned())?;
        } else {
            // Atomic exchange keeps the displaced inode until we have checked it.
            exchange(&tmp, path)?;
            remove_tmp = false;
            if !snapshot(&tmp).is_ok_and(|displaced| displaced.as_ref() == original.as_ref()) {
                // If another writer also replaced our publication, preserve the
                // displaced file for recovery instead of clobbering that writer.
                if fs::symlink_metadata(path)
                    .is_ok_and(|m| m.ino() == meta.ino() && m.dev() == meta.dev())
                    && snapshot(path)?.is_some_and(|s| s.bytes == bytes)
                {
                    exchange(&tmp, path)?;
                    remove_tmp = true;
                } else {
                    bail!("{conflict} Предыдущая версия в {}", tmp.display());
                }
                bail!("{conflict}");
            }
            remove_tmp = true;
        }
        *original = Some(Snapshot {
            dev: meta.dev(),
            ino: meta.ino(),
            bytes,
        });
        Ok(())
    })();
    if remove_tmp {
        fs::remove_file(&tmp)?;
    }
    result?;
    dir.sync_all()?;
    Ok(())
}

pub struct Store {
    pub path: PathBuf,
    pub db: Database,
    original: Option<Snapshot>,
}
impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let original = snapshot(path).with_context(|| format!("Чтение {}", path.display()))?;
        let db: Database = match &original {
            Some(s) => toml::from_str(
                std::str::from_utf8(&s.bytes)
                    .with_context(|| format!("База {} должна быть в UTF-8", path.display()))?,
            )
            .with_context(|| format!("Некорректная база {}", path.display()))?,
            None => Database::default(),
        };
        db.validate()
            .with_context(|| format!("Некорректная база {}", path.display()))?;
        Ok(Self {
            path: path.to_owned(),
            original,
            db,
        })
    }
    pub fn save(&mut self) -> Result<()> {
        self.db.validate()?;
        let bytes = toml::to_string_pretty(&self.db)?.into_bytes();
        publish(&self.path, &mut self.original, bytes, None, CONFLICT)
    }
}
fn exchange(a: &Path, b: &Path) -> io::Result<()> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    let a = CString::new(a.as_os_str().as_bytes())?;
    let b = CString::new(b.as_os_str().as_bytes())?;
    let r = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            a.as_ptr(),
            libc::AT_FDCWD,
            b.as_ptr(),
            libc::RENAME_EXCHANGE,
        )
    };
    if r == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bookworm-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn minimal_db(extra: &str) -> String {
        format!("version = 1\nfont = \"PT Serif\"\nsize = 24\nsingle_page = false\n{extra}")
    }

    #[test]
    fn store_roundtrips_changes_through_a_file() {
        let dir = temp("roundtrip");
        let path = dir.join("library.toml");
        {
            let mut store = Store::open(&path).unwrap();
            assert!(store.db.folder.is_none()); // Missing file → defaults.
            store.db.size = 30;
            store.db.books.insert(
                SHA.into(),
                Record {
                    progress: 40,
                    ..Default::default()
                },
            );
            store.save().unwrap();
        }
        let store = Store::open(&path).unwrap();
        assert_eq!(store.db.size, 30);
        assert_eq!(store.db.books[SHA].progress, 40);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn save_detects_external_modification() {
        let dir = temp("conflict");
        let path = dir.join("library.toml");
        let mut store = Store::open(&path).unwrap();
        store.db.size = 30;
        store.save().unwrap();

        let mut store = Store::open(&path).unwrap();
        fs::write(&path, b"tampered").unwrap();
        store.db.size = 31;
        let err = store.save().unwrap_err();
        assert!(err.to_string().contains("изменён извне"), "{err}");
        // The tampered file is preserved, not clobbered.
        assert_eq!(fs::read(&path).unwrap(), b"tampered");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn open_rejects_invalid_databases() {
        let cases = [
            (
                minimal_db(""),
                "version = 2\nfont = \"PT Serif\"\nsize = 24\nsingle_page = false\n",
                "версия",
            ),
            (
                minimal_db(""),
                "version = 1\nfont = \"PT Serif\"\nsize = 11\nsingle_page = false\n",
                "Размер шрифта",
            ),
            (
                minimal_db(""),
                "version = 1\nfont = \"PT Serif\"\nsize = 73\nsingle_page = false\n",
                "Размер шрифта",
            ),
            (
                minimal_db(""),
                &minimal_db("[books.xyz]\nprogress = 0\n"),
                "SHA-256",
            ),
            (
                minimal_db(""),
                &minimal_db(&format!("[books.{SHA}]\nprogress = 101\n")),
                "Прогресс",
            ),
            (
                minimal_db(""),
                &minimal_db(&format!("[books.{SHA}]\ncompletions = [\"вчера\"]\n")),
                "дата",
            ),
            (
                minimal_db(""),
                &minimal_db(&format!("[books.{SHA}]\ncompletions = [\"2026-11-19\"]\n")),
                "UTC",
            ),
            (
                minimal_db(""),
                &minimal_db(&format!(
                    "[books.{SHA}]\ncompletions = [\"2026-11-19T10:00:00\"]\n"
                )),
                "UTC",
            ),
        ];
        for (i, (_, toml_text, expected)) in cases.into_iter().enumerate() {
            let dir = temp(&format!("invalid-{i}"));
            let path = dir.join("library.toml");
            fs::write(&path, toml_text).unwrap();
            let err = match Store::open(&path) {
                Err(e) => format!("{:#}", e),
                Ok(_) => panic!("case {i}: expected an invalid database"),
            };
            assert!(err.contains(expected), "case {i}: {err}");
            fs::remove_dir_all(&dir).unwrap();
        }
    }

    #[test]
    fn open_accepts_a_valid_database() {
        let dir = temp("valid");
        let path = dir.join("library.toml");
        let toml_text = minimal_db(&format!(
            "\n[books.{SHA}]\nprogress = 40\nlast_read = \"2026-11-19T10:00:00Z\"\n\
             completions = [\"2026-11-19T09:00:00Z\"]\n"
        ));
        fs::write(&path, toml_text).unwrap();
        let store = Store::open(&path).unwrap();
        assert_eq!(store.db.books[SHA].progress, 40);
        fs::remove_dir_all(&dir).unwrap();
    }
}
