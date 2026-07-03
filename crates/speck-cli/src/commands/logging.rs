use std::{
    fs::{File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

use anyhow::Context as _;
use tracing_subscriber::EnvFilter;

// ──────────────────────────────────────────────────────────────────────────────
// SizeRotatingFileAppender
// ──────────────────────────────────────────────────────────────────────────────

pub struct SizeRotatingFileAppender {
    dir: PathBuf,
    filename: String,
    max_bytes: u64,
    max_files: usize,
    current: Option<File>,
    bytes_written: u64,
}

impl SizeRotatingFileAppender {
    pub fn new(
        dir: PathBuf,
        filename: &str,
        max_bytes: u64,
        max_files: usize,
    ) -> io::Result<Self> {
        let mut appender = SizeRotatingFileAppender {
            dir,
            filename: filename.to_string(),
            max_bytes,
            max_files,
            current: None,
            bytes_written: 0,
        };
        appender.open_current()?;
        Ok(appender)
    }

    fn open_current(&mut self) -> io::Result<()> {
        let path = self.dir.join(&self.filename);
        let f = OpenOptions::new().create(true).append(true).open(&path)?;
        self.bytes_written = f.metadata()?.len();
        self.current = Some(f);
        Ok(())
    }

    fn rotate(&mut self) -> io::Result<()> {
        // Shift .log.{i} → .log.{i+1} from highest index down to 1,
        // removing the oldest file when it would exceed max_files.
        for i in (1..self.max_files).rev() {
            let src = self.dir.join(format!("{}.{}", self.filename, i));
            let dst = self.dir.join(format!("{}.{}", self.filename, i + 1));
            if src.exists() {
                if dst.exists() {
                    std::fs::remove_file(&dst)?;
                }
                std::fs::rename(&src, &dst)?;
            }
        }

        // Rename the current log file to .log.1
        let current_path = self.dir.join(&self.filename);
        let first_backup = self.dir.join(format!("{}.1", self.filename));
        if current_path.exists() {
            if first_backup.exists() {
                std::fs::remove_file(&first_backup)?;
            }
            std::fs::rename(&current_path, &first_backup)?;
        }

        self.open_current()
    }
}

impl Write for SizeRotatingFileAppender {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.bytes_written + buf.len() as u64 >= self.max_bytes {
            self.rotate()?;
        }
        let n = self.current.as_mut().unwrap().write(buf)?;
        self.bytes_written += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.current
            .as_mut()
            .map(|f| f.flush())
            .unwrap_or(Ok(()))
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// init_daemon_logging
// ──────────────────────────────────────────────────────────────────────────────

/// Initialise structured file logging for the daemon process.
///
/// Creates a [`SizeRotatingFileAppender`] at `speck_home/speck.log` (10 MiB cap,
/// 5 backup files), wraps it in a `tracing-appender` non-blocking writer, and
/// installs it as the global tracing subscriber.
///
/// The caller **must** hold the returned [`WorkerGuard`] for the entire process
/// lifetime. Dropping it early will flush and shut down the background writer
/// thread, silencing all subsequent log output.
pub fn init_daemon_logging(
    speck_home: &Path,
    log_level: &str,
) -> anyhow::Result<tracing_appender::non_blocking::WorkerGuard> {
    let appender =
        SizeRotatingFileAppender::new(speck_home.to_owned(), "speck.log", 10 * 1024 * 1024, 5)
            .context("failed to create log appender")?;
    let (non_blocking, guard) = tracing_appender::non_blocking(appender);
    tracing_subscriber::fmt()
        .with_writer(non_blocking)
        .with_env_filter(EnvFilter::try_new(log_level)?)
        .try_init()
        .map_err(|e| anyhow::anyhow!(e))?;
    Ok(guard)
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a unique temporary directory for the test and return its path.
    fn temp_log_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("speck-logging-{name}-{}", std::process::id()));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).unwrap();
        }
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn test_size_rotating_appender_rotates() {
        let dir = temp_log_dir("rotates");
        let mut appender =
            SizeRotatingFileAppender::new(dir.clone(), "speck.log", 10, 5).unwrap();
        // 15 bytes triggers rotation (0 + 15 >= 10)
        appender.write_all(b"hello, rotation").unwrap();
        assert!(dir.join("speck.log").exists(), "speck.log should exist after rotation");
        assert!(
            dir.join("speck.log.1").exists(),
            "speck.log.1 should exist — old log was renamed"
        );
    }

    #[test]
    fn test_size_rotating_appender_max_files_enforced() {
        let dir = temp_log_dir("maxfiles");
        // max_bytes=1: any write of 2 bytes triggers rotation
        // max_files=3: keep at most .log.1 + .log.2 + .log.3
        let mut appender =
            SizeRotatingFileAppender::new(dir.clone(), "speck.log", 1, 3).unwrap();
        for _ in 0..6 {
            appender.write_all(b"AB").unwrap();
        }
        let count = std::fs::read_dir(&dir).unwrap().count();
        assert!(
            count <= 4,
            "expected at most 4 files (speck.log + .log.1 + .log.2 + .log.3), found {count}"
        );
    }
}
