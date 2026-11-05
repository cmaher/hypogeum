use std::fs;
use std::io::Write;

use hypgm_logging::{
    LogFileSettings, LogFormat, RotatingFile, TestWriter, create_subscriber_with_file,
};

fn file_settings(dir: &tempfile::TempDir, max_bytes: u64, keep: usize) -> LogFileSettings {
    LogFileSettings {
        directory: dir.path().join("logs"),
        name: "app".to_string(),
        max_bytes,
        keep,
    }
}

fn read(path: std::path::PathBuf) -> String {
    fs::read_to_string(path).expect("read log file")
}

#[test]
fn rolls_at_cap_and_keeps_only_newest() {
    let dir = tempfile::tempdir().unwrap();
    let settings = file_settings(&dir, 10, 2);
    let mut file = RotatingFile::open(settings.clone()).unwrap();

    for line in ["aaaaaaa\n", "bbbbbbb\n", "ccccccc\n", "ddddddd\n"] {
        file.write_all(line.as_bytes()).unwrap();
    }

    assert_eq!(read(settings.path()), "ddddddd\n");
    assert_eq!(read(settings.rotated_path(1)), "ccccccc\n");
    assert_eq!(read(settings.rotated_path(2)), "bbbbbbb\n");
    assert!(!settings.rotated_path(3).exists());
}

#[test]
fn fills_up_to_cap_before_rolling() {
    let dir = tempfile::tempdir().unwrap();
    let settings = file_settings(&dir, 10, 1);
    let mut file = RotatingFile::open(settings.clone()).unwrap();

    file.write_all(b"abcd\n").unwrap();
    file.write_all(b"efgh\n").unwrap();
    assert!(!settings.rotated_path(1).exists());

    file.write_all(b"i\n").unwrap();
    assert_eq!(read(settings.rotated_path(1)), "abcd\nefgh\n");
    assert_eq!(read(settings.path()), "i\n");
}

#[test]
fn appends_to_existing_file_and_counts_its_size() {
    let dir = tempfile::tempdir().unwrap();
    let settings = file_settings(&dir, 10, 1);
    fs::create_dir_all(&settings.directory).unwrap();
    fs::write(settings.path(), "old-old\n").unwrap();

    let mut file = RotatingFile::open(settings.clone()).unwrap();
    file.write_all(b"n\n").unwrap();
    assert_eq!(read(settings.path()), "old-old\nn\n");

    file.write_all(b"new\n").unwrap();
    assert_eq!(read(settings.rotated_path(1)), "old-old\nn\n");
    assert_eq!(read(settings.path()), "new\n");
}

#[test]
fn keep_zero_truncates_in_place() {
    let dir = tempfile::tempdir().unwrap();
    let settings = file_settings(&dir, 10, 0);
    let mut file = RotatingFile::open(settings.clone()).unwrap();

    file.write_all(b"first-1\n").unwrap();
    file.write_all(b"second\n").unwrap();

    assert_eq!(read(settings.path()), "second\n");
    assert!(!settings.rotated_path(1).exists());
}

#[test]
fn oversized_event_is_written_whole_to_fresh_file() {
    let dir = tempfile::tempdir().unwrap();
    let settings = file_settings(&dir, 4, 1);
    let mut file = RotatingFile::open(settings.clone()).unwrap();

    file.write_all(b"ab\n").unwrap();
    file.write_all(b"much-too-long\n").unwrap();

    assert_eq!(read(settings.rotated_path(1)), "ab\n");
    assert_eq!(read(settings.path()), "much-too-long\n");
}

#[test]
fn subscriber_writes_filtered_events_to_file_without_ansi() {
    let dir = tempfile::tempdir().unwrap();
    let settings = file_settings(&dir, 1 << 20, 1);
    let writer = TestWriter::new();
    let subscriber =
        create_subscriber_with_file("warn", LogFormat::Json, writer.clone(), settings.clone())
            .unwrap();

    tracing::subscriber::with_default(subscriber, || {
        tracing::info!("filtered out");
        tracing::warn!(key = "value", "kept event");
    });

    let contents = read(settings.path());
    assert!(
        !contents.contains('\u{1b}'),
        "file must not contain ANSI escapes"
    );
    let lines: Vec<&str> = contents.lines().collect();
    assert_eq!(lines.len(), 1);
    let parsed: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(parsed["fields"]["message"], "kept event");
    assert_eq!(parsed["fields"]["key"], "value");
    assert!(writer.output_string().contains("kept event"));
    assert!(!writer.output_string().contains("filtered out"));

    // Kept in the same test: tracing callsite interest is process-global, and
    // parallel tests installing scoped subscribers can race on it.
    let dir = tempfile::tempdir().unwrap();
    let settings = file_settings(&dir, 1 << 20, 1);
    let subscriber =
        create_subscriber_with_file("info", LogFormat::Text, TestWriter::new(), settings.clone())
            .unwrap();

    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(key = "value", "plain text");
    });

    let contents = read(settings.path());
    assert!(contents.contains("plain text"));
    assert!(contents.contains("key=\"value\""));
    assert!(!contents.contains('\u{1b}'));
}
