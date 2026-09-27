//! File watcher for CodeYourPCB hot reload.
//!
//! Provides debounced file watching for .cypcb files, enabling instant feedback
//! when editing PCB designs.
//!
//! # Example
//!
//! ```no_run
//! use cypcb_watcher::{FileWatcher, WatchEvent};
//! use std::path::Path;
//!
//! let watcher = FileWatcher::new(Path::new("./examples")).unwrap();
//!
//! loop {
//!     match watcher.recv() {
//!         Ok(WatchEvent::Modified(path)) => {
//!             println!("File changed: {}", path);
//!         }
//!         Ok(WatchEvent::Error(err)) => {
//!             eprintln!("Watch error: {}", err);
//!         }
//!         Err(_) => break,
//!     }
//! }
//! ```

use notify_debouncer_full::{new_debouncer, notify::RecursiveMode, DebounceEventResult};
use std::path::Path;
use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;

/// Events from the file watcher.
#[derive(Debug, Clone)]
pub enum WatchEvent {
    /// A .cypcb file was modified. Contains the file path.
    Modified(String),
    /// An error occurred during watching.
    Error(String),
}

/// File watcher with debouncing for .cypcb files.
///
/// Uses 200ms debounce to handle editor save patterns (some editors do
/// multiple writes per save).
pub struct FileWatcher {
    /// The debouncer is held to keep watching active.
    /// Using Box<dyn Any> to avoid exposing internal types.
    _debouncer: Box<dyn std::any::Any + Send>,
    /// Channel for receiving watch events
    receiver: Receiver<WatchEvent>,
}

impl FileWatcher {
    /// Create a new file watcher for the given directory or file.
    ///
    /// If a directory is provided, watches for .cypcb files within it.
    /// Uses 200ms debounce to coalesce rapid file system events.
    ///
    /// # Errors
    ///
    /// Returns an error if the file watcher cannot be created or the
    /// path cannot be watched.
    pub fn new(path: &Path) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let (tx, rx) = channel();
        let path_owned = path.to_path_buf();

        let mut debouncer = new_debouncer(
            Duration::from_millis(200),
            None,
            move |result: DebounceEventResult| {
                match result {
                    Ok(events) => {
                        for event in events {
                            // Only care about .cypcb files
                            for path in &event.paths {
                                if path.extension().map(|e| e == "cypcb").unwrap_or(false) {
                                    let _ = tx.send(WatchEvent::Modified(
                                        path.to_string_lossy().to_string(),
                                    ));
                                }
                            }
                        }
                    }
                    Err(errs) => {
                        for err in errs {
                            let _ = tx.send(WatchEvent::Error(err.to_string()));
                        }
                    }
                }
            },
        )?;

        // Watch recursively to handle subdirectories
        debouncer.watch(&path_owned, RecursiveMode::Recursive)?;

        Ok(FileWatcher {
            _debouncer: Box::new(debouncer),
            receiver: rx,
        })
    }

    /// Try to receive a watch event without blocking.
    ///
    /// Returns `None` if no event is available.
    pub fn try_recv(&self) -> Option<WatchEvent> {
        self.receiver.try_recv().ok()
    }

    /// Receive a watch event, blocking until one is available.
    ///
    /// Returns an error if the sender has been dropped (watcher stopped).
    pub fn recv(&self) -> Result<WatchEvent, std::sync::mpsc::RecvError> {
        self.receiver.recv()
    }

    /// Receive a watch event with a timeout.
    ///
    /// Returns `None` if the timeout expires before an event is available.
    pub fn recv_timeout(&self, timeout: Duration) -> Option<WatchEvent> {
        self.receiver.recv_timeout(timeout).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_watcher_creation() {
        // Watching reads a directory and writes nothing, so this crate's own does.
        let watcher = FileWatcher::new(std::path::Path::new(env!("CARGO_MANIFEST_DIR")));
        assert!(watcher.is_ok());
    }

    /// How long a test waits for an event before it calls the event lost.
    /// Only a failing test waits this long; a passing one returns at its event.
    const EVENT_DEADLINE: Duration = Duration::from_secs(30);

    /// Waits for the watcher to report `name`, and returns every event that
    /// came before it. Panics when the deadline passes without it.
    fn wait_for(watcher: &FileWatcher, name: &str) -> Vec<WatchEvent> {
        let deadline = std::time::Instant::now() + EVENT_DEADLINE;
        let mut before = Vec::new();
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            match watcher.recv_timeout(left) {
                Some(WatchEvent::Modified(path)) if path.ends_with(name) => return before,
                Some(other) => before.push(other),
                None => panic!("no event for {name} within {EVENT_DEADLINE:?}; saw {before:?}"),
            }
        }
    }

    #[test]
    fn test_watcher_detects_change() {
        let temp = std::env::temp_dir().join(format!("cypcb_watcher_test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp);
        fs::create_dir_all(&temp).expect("Failed to create test dir");

        let watcher = FileWatcher::new(&temp).expect("Failed to create watcher");

        let test_file = temp.join("test.cypcb");
        fs::write(&test_file, "version 1\n").expect("Failed to write test file");

        let before = wait_for(&watcher, "test.cypcb");

        let _ = fs::remove_dir_all(&temp);

        assert!(
            before.iter().all(|e| matches!(e, WatchEvent::Modified(_))),
            "the watcher reported an error: {before:?}"
        );
    }

    #[test]
    fn test_ignores_non_cypcb_files() {
        let temp =
            std::env::temp_dir().join(format!("cypcb_watcher_test_ignore-{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp);
        fs::create_dir_all(&temp).expect("Failed to create test dir");

        let watcher = FileWatcher::new(&temp).expect("Failed to create watcher");

        // The .txt file is written first. The .cypcb file after it is the
        // control: once its event is in, the watcher has seen both writes, so
        // silence about the .txt file is a finding and not a slow machine.
        fs::write(temp.join("test.txt"), "hello\n").expect("Failed to write test file");
        fs::write(temp.join("control.cypcb"), "version 1\n").expect("Failed to write control");

        let mut seen = wait_for(&watcher, "control.cypcb");
        while let Some(event) = watcher.try_recv() {
            seen.push(event);
        }

        let _ = fs::remove_dir_all(&temp);

        let stray: Vec<_> = seen
            .iter()
            .filter(|e| !matches!(e, WatchEvent::Modified(p) if p.ends_with("control.cypcb")))
            .collect();
        assert!(
            stray.is_empty(),
            "Should not receive events for non-.cypcb files: {stray:?}"
        );
    }
}
