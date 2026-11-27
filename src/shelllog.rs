//! Shell log for builds that have no console.
//!
//! Debug `cargo run` still prints to stderr. A Windows release is a GUI
//! program, so the same lines are also appended to `data/LOG/gpng.log`.
//!
//! Each start renames a non-empty `gpng.log` to `lastlog.log` and begins a
//! new file, so a restart leaves the previous run beside the current one.
//! A process left up for days rolls the same way once the file reaches
//! [`ROTATE_AT`], which keeps the two files from growing without bound.

use std::fs::{self, File, OpenOptions};
use std::io::{self, ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::EnvFilter;

use crate::paths;

/// Roll the shell log once it reaches this size while the program is running.
const ROTATE_AT: u64 = 1024 * 1024;

const LAST_NAME: &str = "lastlog.log";

struct LogFile {
    file: Option<File>,
    path: PathBuf,
    bytes: u64,
}

struct ShellLog {
    inner: Arc<Mutex<LogFile>>,
}

struct ShellWrite<'a> {
    inner: MutexGuard<'a, LogFile>,
}

impl Write for ShellWrite<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let _ = io::stderr().write_all(buf);
        let log = &mut *self.inner;
        let file = log
            .file
            .as_mut()
            .ok_or_else(|| io::Error::new(ErrorKind::Other, "log closed"))?;
        file.write_all(buf)?;
        log.bytes = log.bytes.saturating_add(buf.len() as u64);
        if log.bytes >= ROTATE_AT {
            if let Err(err) = rotate(log) {
                let _ = writeln!(io::stderr(), "log rotate: {err}");
            }
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        let _ = io::stderr().flush();
        if let Some(file) = self.inner.file.as_mut() {
            file.flush()?;
        }
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for ShellLog {
    type Writer = ShellWrite<'a>;

    fn make_writer(&'a self) -> Self::Writer {
        ShellWrite {
            inner: self.inner.lock().unwrap_or_else(|e| e.into_inner()),
        }
    }
}

/// `info` for libraries, `debug` for this crate, unless `RUST_LOG` is set.
fn filter() -> EnvFilter {
    EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,graphic_packet=debug"))
}

/// Open `data/LOG/gpng.log` and install the global tracing subscriber.
///
/// A panic is appended to that same file. Release builds are a Windows GUI
/// program, so the default panic text never reaches a console or the tracing
/// log. The hook writes the file itself and does not take the tracing lock.
pub fn init() {
    let dir = paths::data_dir().join("LOG");
    let path = dir.join("gpng.log");
    let _ = paths::ensure_dir(&dir);
    let file = open_log(&path).ok();
    let filter = filter();
    match file {
        Some(file) => {
            let log = ShellLog {
                inner: Arc::new(Mutex::new(file)),
            };
            tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_writer(log)
                .with_ansi(false)
                .with_target(false)
                .init();
        }
        None => {
            tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_writer(io::stderr)
                .with_ansi(false)
                .with_target(false)
                .init();
        }
    }
    install_panic_log();
}

fn open_log(path: &Path) -> io::Result<LogFile> {
    roll_previous(path);
    let file = OpenOptions::new().create(true).append(true).open(path)?;
    let bytes = file.metadata().map(|meta| meta.len()).unwrap_or(0);
    Ok(LogFile {
        file: Some(file),
        path: path.to_path_buf(),
        bytes,
    })
}

/// Move a non-empty `gpng.log` aside to `lastlog.log`. An empty current file
/// leaves the previous `lastlog.log` in place. A failure keeps the current
/// file so logging can continue.
fn roll_previous(path: &Path) {
    let Ok(meta) = fs::metadata(path) else {
        return;
    };
    if meta.len() == 0 {
        return;
    }
    let Some(last) = last_log(path) else {
        return;
    };
    let _ = fs::remove_file(&last);
    let _ = fs::rename(path, last);
}

fn last_log(path: &Path) -> Option<PathBuf> {
    Some(path.parent()?.join(LAST_NAME))
}

/// Close the open file, rename it to `lastlog.log`, and start a new one.
fn rotate(log: &mut LogFile) -> io::Result<()> {
    if let Some(file) = log.file.as_mut() {
        file.flush()?;
    }
    // Windows will not rename a file that still has a handle.
    log.file.take();
    let Some(last) = last_log(&log.path) else {
        reopen(log)?;
        return Err(io::Error::new(ErrorKind::Other, "log has no directory"));
    };
    let _ = fs::remove_file(&last);
    if let Err(err) = fs::rename(&log.path, &last) {
        reopen(log)?;
        return Err(err);
    }
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&log.path)?;
    log.file = Some(file);
    log.bytes = 0;
    if let Some(file) = log.file.as_mut() {
        let note = "previous log renamed to lastlog.log\n";
        file.write_all(note.as_bytes())?;
        log.bytes = note.len() as u64;
    }
    Ok(())
}

fn reopen(log: &mut LogFile) -> io::Result<()> {
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log.path)?;
    log.bytes = file.metadata().map(|meta| meta.len()).unwrap_or(log.bytes);
    log.file = Some(file);
    Ok(())
}

fn install_panic_log() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let loc = info.location().map_or_else(
            || "unknown location".to_string(),
            |loc| format!("{}:{}:{}", loc.file(), loc.line(), loc.column()),
        );
        let payload = info.payload();
        let message = payload
            .downcast_ref::<&str>()
            .map(|text| (*text).to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "non-text panic".to_string());
        let path = paths::data_dir().join("LOG").join("gpng.log");
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(file, "panic at {loc}: {message}");
            let _ = file.flush();
        }
        previous(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "gpng-log-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn restart_keeps_the_previous_log_only() {
        let dir = scratch();
        let path = dir.join("gpng.log");
        let last = dir.join(LAST_NAME);
        fs::write(&path, b"session one\n").unwrap();
        roll_previous(&path);
        assert!(!path.exists());
        assert_eq!(fs::read(&last).unwrap(), b"session one\n");

        fs::write(&path, b"").unwrap();
        roll_previous(&path);
        assert_eq!(fs::read(&last).unwrap(), b"session one\n");
        assert_eq!(fs::read(&path).unwrap(), b"");

        fs::write(&path, b"session two\n").unwrap();
        roll_previous(&path);
        assert_eq!(fs::read(&last).unwrap(), b"session two\n");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_long_run_rolls_the_open_file() {
        let dir = scratch();
        let path = dir.join("gpng.log");
        let mut log = LogFile {
            file: Some(
                OpenOptions::new()
                    .create(true)
                    .write(true)
                    .truncate(true)
                    .open(&path)
                    .unwrap(),
            ),
            path: path.clone(),
            bytes: ROTATE_AT,
        };
        writeln!(log.file.as_mut().unwrap(), "still running").unwrap();
        rotate(&mut log).unwrap();
        assert_eq!(fs::read(dir.join(LAST_NAME)).unwrap(), b"still running\n");
        log.file.as_mut().unwrap().flush().unwrap();
        let current = fs::read(&path).unwrap();
        assert!(current.starts_with(b"previous log renamed to lastlog.log\n"));
        drop(log);
        let _ = fs::remove_dir_all(&dir);
    }
}
