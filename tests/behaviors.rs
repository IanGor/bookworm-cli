use bookworm::{
    graphics, library,
    reader::{Frame, Reading, Window},
    storage::{Anchor, Record, Store, content_id},
};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        let p = std::env::temp_dir().join(format!(
            "bookworm-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../BookwormEngine/tests/corpus/synthetic")
        .join(name)
}

#[test]
fn snapshot_conflicts_and_invalid_toml_preserve_disk_bytes() {
    let d = Temp::new();
    let p = d.0.join("library.toml");
    let mut a = Store::open(&p).unwrap();
    let mut b = Store::open(&p).unwrap();
    a.db.single_page = true;
    a.save().unwrap();
    assert!(b.save().is_err());
    let invalid = b"missing = [";
    fs::write(&p, invalid).unwrap();
    assert!(a.save().is_err());
    assert!(Store::open(&p).is_err());
    assert_eq!(fs::read(&p).unwrap(), invalid);
}

#[test]
fn recursive_library_unifies_exact_copies_preserves_overrides_and_sorts_recent() {
    let d = Temp::new();
    let nested = d.0.join("раздел");
    fs::create_dir(&nested).unwrap();
    let first = nested.join("a.FB2");
    fs::copy(fixture("02_structure_ru.fb2"), &first).unwrap();
    fs::copy(&first, d.0.join("copy.fb2")).unwrap();
    fs::copy(fixture("06_english_epub3.epub"), nested.join("other.epub")).unwrap();
    let mut store = Store::open(&d.0.join("library.toml")).unwrap();
    let mut scan = library::scan(&d.0, &mut store.db).unwrap();
    assert_eq!(scan.entries.len(), 2);
    assert!(scan.warnings.is_empty());
    let id = content_id(&fs::read(&first).unwrap());
    let record = store.db.books.get_mut(&id).unwrap();
    record.title_override = Some("Моя книга".into());
    record.author_override = Some("Редактор".into());
    record.last_read = Some(bookworm::storage::now());
    record.anchor = Anchor {
        block: 7,
        offset: 11,
    };
    record.progress = 47;
    store.save().unwrap();
    fs::rename(&first, nested.join("moved.fb2")).unwrap();
    let mut reopened = Store::open(&store.path).unwrap();
    scan = library::scan(&d.0, &mut reopened.db).unwrap();
    scan.sort(&reopened.db);
    assert_eq!(scan.entries[0].id, id);
    let r = &reopened.db.books[&id];
    assert_eq!((r.title(), r.author()), ("Моя книга", "Редактор"));
    assert_eq!(
        (r.anchor, r.progress),
        (
            Anchor {
                block: 7,
                offset: 11
            },
            47
        )
    );
}

#[test]
fn native_spread_keeps_type_size_and_respects_single_page() {
    let window = Window {
        cols: 160,
        rows: 45,
        width: 1600,
        height: 900,
    };
    let spread = Frame::new(window, 24, false).unwrap();
    let single = Frame::new(window, 24, true).unwrap();
    assert_eq!((spread.pages, single.pages), (2, 1));
    assert_eq!(
        spread.geometry(24, 0.5).font_size,
        single.geometry(24, 0.5).font_size
    );
    assert!(spread.width * 2 <= window.width && spread.height < window.height);
    assert_eq!(
        Frame::new(
            Window {
                width: 760,
                cols: 76,
                ..window
            },
            24,
            false
        )
        .unwrap()
        .pages,
        1
    );
}

#[test]
fn last_spread_and_confirmation_are_both_required_each_time() {
    let mut r = Record::default();
    let first = Reading {
        page: 1,
        total: 4,
        pages: 2,
    };
    let end = Reading { page: 2, ..first };
    assert!(!first.finish(true, &mut r));
    assert!(!end.finish(false, &mut r));
    assert!(r.completions.is_empty());
    assert!(end.finish(true, &mut r));
    assert!(!end.finish(false, &mut r));
    assert!(end.finish(true, &mut r));
    assert_eq!(r.completions.len(), 2);
}

#[test]
fn graphics_chunks_and_cleanup_are_scoped_to_owned_images() {
    let mut out = Vec::new();
    graphics::send(&mut out, 42, &vec![0x61; 6145]).unwrap();
    let stream = String::from_utf8(out).unwrap();
    let parts: Vec<_> = stream.split("\x1b_G").skip(1).collect();
    assert_eq!(parts.len(), 3);
    for (i, chunk) in parts.iter().enumerate() {
        let (params, data) = chunk.trim_end_matches("\x1b\\").split_once(';').unwrap();
        assert!(params.contains(if i == 2 { "m=0" } else { "m=1" }));
        assert_eq!(data.len(), if i == 2 { 4 } else { 4096 });
        if i == 0 {
            for key in ["a=T", "f=100", "t=d", "i=42", "C=1"] {
                assert!(params.contains(key));
            }
        } else {
            assert!(!params.contains("a="));
        }
    }
    assert!(stream.ends_with("YQ==\x1b\\"));
    let mut del = Vec::new();
    graphics::delete(&mut del, 42).unwrap();
    assert!(String::from_utf8(del).unwrap().contains("d=I,i=42"));
    assert!(graphics::probe_ok(b"\x1b_Gi=31;OK\x1b\\", 31));
    assert!(!graphics::probe_ok(b"\x1b_Gi=310;OK\x1b\\", 31));
    assert!(!graphics::probe_ok(b"\x1b_Gi=31;EINVAL\x1b\\", 31));
}

#[test]
fn digest_matches_standard_sha256() {
    assert_eq!(
        content_id(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn shared_corpus_renders_epub_and_zipped_fb2_with_bundled_fonts() {
    use bookworm::{fonts, reader::Reader};
    use bookworm_core::{book::Book, render};
    let frame = Frame::new(
        Window {
            cols: 75,
            rows: 32,
            width: 750,
            height: 640,
        },
        24,
        false,
    )
    .unwrap();
    for (filename, family) in [
        ("06_english_epub3.epub", "Literata"),
        ("07_epub2_calibre.epub", "PT Sans"),
        ("04_zipped.fb2.zip", "Charis SIL"),
    ] {
        let bytes = fs::read(fixture(filename)).unwrap();
        let fonts = fonts::load(family, &[]).unwrap();
        let geometry = frame.geometry(24, fonts.faces[0].x_height);
        let book = Book::open(&bytes, fonts, geometry).unwrap();
        let reader = Reader::new(book, frame, Anchor::default()).unwrap();
        assert!(!reader.book.doc.title.is_empty());
        let png = render::page(&reader.book, reader.page, &render::DAY)
            .encode_png()
            .unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        assert!(reader.state().total > 1);
    }
}

#[test]
fn replacing_with_symlink_or_deleting_during_session_is_a_conflict() {
    use std::os::unix::fs::symlink;
    let d = Temp::new();
    let p = d.0.join("library.toml");
    let mut a = Store::open(&p).unwrap();
    a.save().unwrap();
    let backup = d.0.join("other");
    fs::rename(&p, &backup).unwrap();
    assert!(a.save().is_err());
    assert!(!p.exists());
    symlink(&backup, &p).unwrap();
    let original = fs::read(&backup).unwrap();
    assert!(a.save().is_err());
    assert!(Store::open(&p).is_err());
    assert_eq!(fs::read(backup).unwrap(), original);
    assert!(p.is_symlink());
}

#[test]
fn completed_books_move_to_bottom_by_latest_completion_and_rereading_restores_active() {
    let d = Temp::new();
    let p = d.0.join("library.toml");
    let mut store = Store::open(&p).unwrap();

    let id_unread = "0000000000000000000000000000000000000000000000000000000000000001";
    let id_active = "0000000000000000000000000000000000000000000000000000000000000002";
    let id_done_older = "0000000000000000000000000000000000000000000000000000000000000003";
    let id_done_newer = "0000000000000000000000000000000000000000000000000000000000000004";
    let id_done_subsec = "0000000000000000000000000000000000000000000000000000000000000005";

    store.db.books.insert(
        id_unread.into(),
        Record {
            source_title: "Анна Каренина".into(),
            ..Default::default()
        },
    );
    store.db.books.insert(
        id_active.into(),
        Record {
            source_title: "Солярис".into(),
            progress: 40,
            last_read: Some("2026-10-01T12:00:00Z".into()),
            ..Default::default()
        },
    );
    // Reopened mid-book with a newer last_read, yet still marked completed: stays at bottom.
    store.db.books.insert(
        id_done_older.into(),
        Record {
            source_title: "Мастер и Маргарита".into(),
            progress: 25,
            last_read: Some("2026-11-26T18:00:00Z".into()),
            completions: vec!["2026-09-15T12:00:00Z".into()],
            ..Default::default()
        },
    );
    // Multiple completions: grouped by the latest one.
    store.db.books.insert(
        id_done_newer.into(),
        Record {
            source_title: "Пикник на обочине".into(),
            progress: 100,
            last_read: Some("2026-11-19T12:00:00Z".into()),
            completions: vec!["2025-05-10T12:00:00Z".into(), "2026-11-19T12:00:00Z".into()],
            ..Default::default()
        },
    );
    // Fractional-second completion 0.5s after id_done_newer must sort ahead of it.
    store.db.books.insert(
        id_done_subsec.into(),
        Record {
            source_title: "Трудно быть богом".into(),
            progress: 100,
            last_read: Some("2026-11-19T12:00:01Z".into()),
            completions: vec!["2026-11-19T12:00:00.500000000Z".into()],
            ..Default::default()
        },
    );

    let mut scan = library::Scan {
        entries: [
            id_done_older,
            id_unread,
            id_done_newer,
            id_active,
            id_done_subsec,
        ]
        .into_iter()
        .map(|id| library::Entry {
            id: id.into(),
            path: PathBuf::from(id),
        })
        .collect(),
        warnings: Vec::new(),
    };
    scan.sort(&store.db);
    let ids: Vec<_> = scan.entries.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![
            id_active,
            id_unread,
            id_done_subsec,
            id_done_newer,
            id_done_older,
        ]
    );

    assert_eq!(
        library::completion_month(store.db.books[id_done_newer].completed_at().unwrap()),
        Some("Ноябрь 2026".into())
    );
    assert_eq!(
        library::completion_month(store.db.books[id_done_older].completed_at().unwrap()),
        Some("Сентябрь 2026".into())
    );

    // Unmarking completion preserves history and returns the book to the active top with its 25%.
    store.db.books.get_mut(id_done_older).unwrap().rereading = true;
    store.save().unwrap();
    let reopened = Store::open(&p).unwrap();
    let r = &reopened.db.books[id_done_older];
    assert!(r.rereading);
    assert_eq!(r.completed_at(), None);
    assert_eq!(r.completions, vec!["2026-09-15T12:00:00Z"]);
    assert_eq!(r.progress, 25);

    scan.sort(&reopened.db);
    assert_eq!(scan.entries[0].id, id_done_older);

    // Finishing again clears rereading, appends a new date, and omits `rereading = false` in TOML.
    let mut store = reopened;
    let end = Reading {
        page: 9,
        total: 10,
        pages: 1,
    };
    assert!(end.finish(true, store.db.books.get_mut(id_done_older).unwrap()));
    store.save().unwrap();
    let raw = fs::read_to_string(&p).unwrap();
    assert!(!raw.contains("rereading"));
    let final_store = Store::open(&p).unwrap();
    let r = &final_store.db.books[id_done_older];
    assert!(!r.rereading);
    assert_eq!(r.completions.len(), 2);
    assert!(r.completed_at().is_some());
}
