use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Settings for a size-capped log file written alongside stderr.
///
/// Logs go to `<directory>/<name>.log`. Before an event would push that file
/// past `max_bytes`, it is rolled to `<name>.log.1` (older files shift to
/// `.2`, `.3`, ...) and files beyond `keep` are deleted. With `keep == 0` the
/// file is truncated in place instead. While the settings stay unchanged,
/// total disk use therefore stays within `max_bytes * (keep + 1)`.
///
/// Known limits: a single event larger than `max_bytes` is still written
/// whole to a fresh file, so that one file can exceed the cap. Rotated files
/// beyond a since-reduced `keep`, and a pre-existing file already larger than
/// `max_bytes`, are not cleaned up beyond the normal rotation.
///
/// Callers resolve and validate these values from their own configuration:
/// `max_bytes` must be greater than zero, `keep` should be small (each
/// rotation does one rename per slot), and `name` must be a plain file name
/// without path separators.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogFileSettings {
    pub directory: PathBuf,
    pub name: String,
    pub max_bytes: u64,
    pub keep: usize,
}

impl LogFileSettings {
    /// Path of the active log file.
    pub fn path(&self) -> PathBuf {
        self.directory.join(format!("{}.log", self.name))
    }

    /// Path of the `index`-th rotated file (`1` is the newest).
    pub fn rotated_path(&self, index: usize) -> PathBuf {
        self.directory.join(format!("{}.log.{index}", self.name))
    }
}

/// A log file writer that rotates by size between writes.
///
/// Each `write` call is treated as one event: rotation happens only before a
/// write, never in the middle of one, so files split on event boundaries.
/// Wrap it in a `Mutex` to use it as a `tracing_subscriber` `MakeWriter`.
#[derive(Debug)]
pub struct RotatingFile {
    settings: LogFileSettings,
    file: File,
    size: u64,
}

impl RotatingFile {
    /// Opens (creating the directory if needed) and appends to the active
    /// log file, counting its existing size toward the cap.
    pub fn open(settings: LogFileSettings) -> io::Result<Self> {
        fs::create_dir_all(&settings.directory)?;
        let file = open_append(&settings.path())?;
        let size = file.metadata()?.len();
        Ok(Self {
            settings,
            file,
            size,
        })
    }

    fn rotate(&mut self) -> io::Result<()> {
        let keep = self.settings.keep;
        if keep == 0 {
            self.file.set_len(0)?;
            self.size = 0;
            return Ok(());
        }
        let path = self.settings.path();
        // A missing active file means a previous rotation already moved it
        // and then failed to reopen; only retry the open so retained files
        // are not shifted again.
        if path.exists() {
            remove_if_exists(&self.settings.rotated_path(keep))?;
            for index in (1..keep).rev() {
                rename_if_exists(
                    &self.settings.rotated_path(index),
                    &self.settings.rotated_path(index + 1),
                )?;
            }
            fs::rename(&path, self.settings.rotated_path(1))?;
        }
        self.file = open_append(&path)?;
        self.size = 0;
        Ok(())
    }
}

impl Write for RotatingFile {
    /// Writes `buf` whole, rotating first if it would exceed the cap. If
    /// rotation fails the event is dropped rather than growing the file.
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let len = buf.len() as u64;
        if self.size > 0 && self.size.saturating_add(len) > self.settings.max_bytes {
            self.rotate()?;
        }
        self.file.write_all(buf)?;
        self.size = self.size.saturating_add(len);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

fn open_append(path: &Path) -> io::Result<File> {
    OpenOptions::new().create(true).append(true).open(path)
}

fn remove_if_exists(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

fn rename_if_exists(from: &Path, to: &Path) -> io::Result<()> {
    match fs::rename(from, to) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}
