//! Shell log for builds that have no console.
//!
//! Debug `cargo run` still prints to stderr. A Windows release is a GUI
//! program, so the same lines are also appended to `data/log/gpng.log`.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::sync::{Arc, Mutex, MutexGuard};

use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::EnvFilter;

use crate::paths;

struct ShellLog {
    file: Arc<Mutex<File>>,
}

struct ShellWrite<'a> {
    file: MutexGuard<'a, File>,
}

impl Write for ShellWrite<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let _ = io::stderr().write_all(buf);
        self.file.write_all(buf)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        let _ = io::stderr().flush();
        self.file.flush()
    }
}

impl<'a> MakeWriter<'a> for ShellLog {
    type Writer = ShellWrite<'a>;

    fn make_writer(&'a self) -> Self::Writer {
        ShellWrite {
            file: self.file.lock().unwrap_or_else(|e| e.into_inner()),
        }
    }
}

/// `info` for libraries, `debug` for this crate, unless `RUST_LOG` is set.
fn filter() -> EnvFilter {
    EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,graphic_packet=debug"))
}

/// Open `data/log/gpng.log` and install the global tracing subscriber.
pub fn init() {
    let dir = paths::data_dir().join("log");
    let path = dir.join("gpng.log");
    let file = fs::create_dir_all(&dir).ok().and_then(|_| {
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .ok()
    });
    let filter = filter();
    match file {
        Some(file) => {
            let log = ShellLog {
                file: Arc::new(Mutex::new(file)),
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
}
