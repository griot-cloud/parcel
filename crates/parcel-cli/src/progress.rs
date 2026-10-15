//! Live progress on stderr: a board of lines, each task's line with its own bar, redrawn in
//! place while the tasks run, then erased. Nothing is drawn unless stderr is a terminal and
//! the board fits on the screen.

use std::io::{IsTerminal, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use crate::ui::{DIM, GREEN, paint};

/// How far one task has got: rows seen out of `total`, or just running when `total` is 0.
#[derive(Debug, Default)]
pub struct Tracker {
    pub seen: AtomicU64,
    pub total: u64,
    pub done: AtomicBool,
}

impl Tracker {
    pub fn new(total: u64) -> Arc<Tracker> {
        Arc::new(Tracker {
            total,
            ..Default::default()
        })
    }

    pub fn add(&self, n: u64) {
        self.seen.fetch_add(n, Ordering::Relaxed);
    }

    pub fn finish(&self) {
        self.done.store(true, Ordering::Relaxed);
    }
}

/// One line of the board: fixed text, then a tracker's bar when it has one.
pub struct Line {
    pub text: String,
    pub tracker: Option<Arc<Tracker>>,
}

impl Line {
    pub fn text(s: impl Into<String>) -> Line {
        Line {
            text: s.into(),
            tracker: None,
        }
    }

    pub fn task(s: impl Into<String>, t: &Arc<Tracker>) -> Line {
        Line {
            text: s.into(),
            tracker: Some(t.clone()),
        }
    }
}

const SPIN: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const BAR: usize = 24;

fn bar(t: &Tracker, frame: usize) -> String {
    let seen = t.seen.load(Ordering::Relaxed);
    if t.done.load(Ordering::Relaxed) {
        return format!("{} {}", paint(GREEN, "━".repeat(BAR)), paint(DIM, "100%"));
    }
    if t.total == 0 {
        let rows = if seen > 0 {
            format!(" {} rows", crate::ui::count(seen))
        } else {
            String::new()
        };
        return paint(DIM, format!("{}{rows}", SPIN[frame % SPIN.len()]));
    }
    let f = (seen as f64 / t.total as f64).clamp(0.0, 1.0);
    let full = (f * BAR as f64) as usize;
    let rest = BAR - full;
    let head = if rest > 0 { "╸" } else { "" };
    format!(
        "{}{}{} {}",
        "━".repeat(full),
        head,
        paint(DIM, "─".repeat(rest.saturating_sub(1))),
        paint(DIM, format!("{:>3.0}%", f * 100.0))
    )
}

/// The terminal's height, when stderr is a terminal.
fn height() -> Option<usize> {
    #[cfg(unix)]
    {
        // SAFETY: TIOCGWINSZ fills the zeroed winsize it is given and reads nothing else.
        let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
        if unsafe { libc::ioctl(2, libc::TIOCGWINSZ, &mut ws) } == 0 && ws.ws_row > 0 {
            return Some(ws.ws_row as usize);
        }
    }
    Some(40)
}

/// A board being drawn; [`Board::stop`] erases it.
pub struct Board {
    stop: Arc<AtomicBool>,
    handle: Option<tokio::task::JoinHandle<()>>,
}

impl Board {
    /// Draw `lines` every 80 ms until stopped. A board taller than the screen is drawn as
    /// one summary line instead.
    pub fn start(lines: Vec<Line>) -> Board {
        let stop = Arc::new(AtomicBool::new(false));
        if !std::io::stderr().is_terminal() || std::env::var_os("NO_PROGRESS").is_some() {
            return Board { stop, handle: None };
        }
        let fits = height().is_some_and(|h| lines.len() + 2 < h);
        let flag = stop.clone();
        let handle = tokio::spawn(async move {
            let trackers: Vec<Arc<Tracker>> =
                lines.iter().filter_map(|l| l.tracker.clone()).collect();
            let mut drawn = 0usize;
            let mut frame = 0usize;
            loop {
                let stopping = flag.load(Ordering::Relaxed);
                let mut out = String::new();
                if drawn > 0 {
                    out.push_str(&format!("\x1b[{drawn}A"));
                }
                if stopping {
                    out.push_str("\x1b[0J");
                    let mut err = std::io::stderr();
                    let _ = err.write_all(out.as_bytes());
                    let _ = err.flush();
                    return;
                }
                let rendered: Vec<String> = if fits {
                    lines
                        .iter()
                        .map(|l| match &l.tracker {
                            Some(t) => format!("{}{}", l.text, bar(t, frame)),
                            None => l.text.clone(),
                        })
                        .collect()
                } else {
                    let done = trackers
                        .iter()
                        .filter(|t| t.done.load(Ordering::Relaxed))
                        .count();
                    let all = Tracker {
                        seen: AtomicU64::new(done as u64),
                        total: trackers.len() as u64,
                        done: AtomicBool::new(false),
                    };
                    vec![format!(
                        "{} {} {}",
                        paint(DIM, SPIN[frame % SPIN.len()]),
                        bar(&all, frame),
                        paint(DIM, format!("{done}/{}", trackers.len()))
                    )]
                };
                for l in &rendered {
                    out.push_str("\x1b[2K");
                    out.push_str(l);
                    out.push('\n');
                }
                drawn = rendered.len();
                let mut err = std::io::stderr();
                let _ = err.write_all(out.as_bytes());
                let _ = err.flush();
                frame += 1;
                tokio::time::sleep(Duration::from_millis(80)).await;
            }
        });
        Board {
            stop,
            handle: Some(handle),
        }
    }

    pub async fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.await;
        }
    }
}
