//! A tiny terminal spinner, the equivalent of `rich`'s `console.status(...)`.
//!
//! The spinner is drawn on **stderr** and only when stderr is a terminal, so
//! piped output, `--json` consumers and CI logs are never polluted. It is also
//! disabled with `--debug`, where log lines would interleave with it.

use std::io::{self, IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::report::{paint, stderr_color, Style};

const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const INTERVAL: Duration = Duration::from_millis(80);

/// A running spinner. It stops and erases itself when dropped.
#[must_use = "the spinner stops as soon as it is dropped"]
pub struct Spinner {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    width: usize,
}

impl Spinner {
    /// Starts a spinner with `message`, or does nothing when stderr is not a
    /// terminal or debug logging is on.
    pub fn start(message: &str) -> Spinner {
        let stop = Arc::new(AtomicBool::new(false));
        let enabled = io::stderr().is_terminal()
            && !log::log_enabled!(log::Level::Debug)
            && !matches!(std::env::var("TERM").as_deref(), Ok("dumb"));
        if !enabled {
            return Spinner {
                stop,
                handle: None,
                width: 0,
            };
        }

        let flag = Arc::clone(&stop);
        let text = message.to_string();
        let width = text.chars().count() + 2;
        let colored = stderr_color();
        let handle = thread::spawn(move || {
            let mut index = 0usize;
            while !flag.load(Ordering::Relaxed) {
                let frame = paint(FRAMES[index % FRAMES.len()], Style::Green, colored);
                {
                    let mut err = io::stderr().lock();
                    let _ = write!(err, "\r{frame} {text}");
                    let _ = err.flush();
                }
                index += 1;
                thread::park_timeout(INTERVAL);
            }
        });
        Spinner {
            stop,
            handle: Some(handle),
            width,
        }
    }
}

impl Drop for Spinner {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            handle.thread().unpark();
            let _ = handle.join();
            let mut err = io::stderr().lock();
            let _ = write!(err, "\r{}\r", " ".repeat(self.width));
            let _ = err.flush();
        }
    }
}
