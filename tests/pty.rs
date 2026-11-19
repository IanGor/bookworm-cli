//! Drive the shipped binary through its normal stdin/stdout and Linux tty ioctls.
#[test]
fn terminal_session() {
    let status = std::process::Command::new("python3")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/session.py"))
        .arg(env!("CARGO_BIN_EXE_bookworm"))
        .status()
        .expect("python3 for PTY integration test");
    assert!(status.success());
}
