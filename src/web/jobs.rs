//! Job registry: long-running operations (rip, burn, recover, verify) run as child
//! processes of the `rustydisc` binary itself. Their `--progress-json` output is
//! translated into a replayable event log that browsers follow over SSE.

use std::{
    process::Stdio,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{SystemTime, UNIX_EPOCH},
};

use serde::Serialize;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt, BufReader, AsyncBufReadExt},
    process::{ChildStdin, Command},
    sync::{watch, Notify},
};

use crate::error::DiscError;

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Running,
    AwaitingInput,
    Done,
    Failed,
    Cancelled,
}

impl Status {
    pub fn is_terminal(self) -> bool {
        matches!(self, Status::Done | Status::Failed | Status::Cancelled)
    }
}

#[derive(Serialize, Clone, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    Progress { pct: f32 },
    Step { msg: String },
    Log { msg: String },
    /// The process is waiting for the user (e.g. "insert the next blank disc").
    Input { msg: String },
    Status { status: Status },
    Error { error: String, message: String, recoverable: bool },
}

struct Inner {
    events: Vec<Event>,
    status: Status,
    pct: f32,
    step: String,
    error: Option<DiscError>,
    finished: Option<u64>,
}

pub struct Job {
    pub id: u64,
    pub kind: String,
    pub title: String,
    /// Exclusive jobs need the optical drive; only one may run at a time.
    pub exclusive: bool,
    pub started: u64,
    inner: Mutex<Inner>,
    tx: watch::Sender<usize>,
    stdin: tokio::sync::Mutex<Option<ChildStdin>>,
    cancel: Notify,
}

#[derive(Serialize)]
pub struct JobSummary {
    pub id: u64,
    pub kind: String,
    pub title: String,
    pub exclusive: bool,
    pub status: Status,
    pub pct: f32,
    pub step: String,
    pub error: Option<DiscError>,
    pub started: u64,
    pub finished: Option<u64>,
    pub events: usize,
}

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl Job {
    pub fn push(&self, ev: Event) {
        let mut g = self.inner.lock().unwrap();
        match &ev {
            Event::Progress { pct } => g.pct = *pct,
            Event::Step { msg } => g.step = msg.clone(),
            Event::Status { status } => {
                g.status = *status;
                if status.is_terminal() {
                    g.finished = Some(now());
                }
            }
            Event::Error { error, message, recoverable } => {
                g.error = Some(DiscError {
                    error: error.clone(),
                    message: message.clone(),
                    recoverable: *recoverable,
                });
            }
            _ => {}
        }
        g.events.push(ev);
        let len = g.events.len();
        drop(g);
        self.tx.send_replace(len);
    }

    pub fn status(&self) -> Status {
        self.inner.lock().unwrap().status
    }

    pub fn summary(&self) -> JobSummary {
        let g = self.inner.lock().unwrap();
        JobSummary {
            id: self.id,
            kind: self.kind.clone(),
            title: self.title.clone(),
            exclusive: self.exclusive,
            status: g.status,
            pct: g.pct,
            step: g.step.clone(),
            error: g.error.clone(),
            started: self.started,
            finished: g.finished,
            events: g.events.len(),
        }
    }

    /// Events from `from` onward, plus whether the job has reached a terminal state.
    pub fn events_since(&self, from: usize) -> (Vec<Event>, bool) {
        let g = self.inner.lock().unwrap();
        let evs = g.events.get(from..).map(|s| s.to_vec()).unwrap_or_default();
        (evs, g.status.is_terminal())
    }

    pub fn subscribe(&self) -> watch::Receiver<usize> {
        self.tx.subscribe()
    }

    pub fn request_cancel(&self) {
        self.cancel.notify_one();
    }

    /// Answer an `Input` prompt (sends ENTER to the child's stdin).
    pub async fn resume(&self) -> bool {
        if self.status() != Status::AwaitingInput {
            return false;
        }
        let mut guard = self.stdin.lock().await;
        if let Some(stdin) = guard.as_mut() {
            if stdin.write_all(b"\n").await.is_ok() && stdin.flush().await.is_ok() {
                drop(guard);
                self.push(Event::Status { status: Status::Running });
                return true;
            }
        }
        false
    }

    pub fn cancel_notified(&self) -> &Notify {
        &self.cancel
    }
}

#[derive(Default)]
pub struct Jobs {
    list: Mutex<Vec<Arc<Job>>>,
    next: AtomicU64,
}

impl Jobs {
    /// Register a new job. Exclusive jobs are refused while another one holds the drive.
    pub fn create(&self, kind: &str, title: &str, exclusive: bool) -> Result<Arc<Job>, Arc<Job>> {
        let mut list = self.list.lock().unwrap();
        if exclusive {
            if let Some(busy) = list.iter().find(|j| j.exclusive && !j.status().is_terminal()) {
                return Err(busy.clone());
            }
        }
        let (tx, _rx) = watch::channel(0usize);
        let job = Arc::new(Job {
            id: self.next.fetch_add(1, Ordering::SeqCst) + 1,
            kind: kind.to_string(),
            title: title.to_string(),
            exclusive,
            started: now(),
            inner: Mutex::new(Inner {
                events: vec![Event::Status { status: Status::Running }],
                status: Status::Running,
                pct: 0.0,
                step: String::new(),
                error: None,
                finished: None,
            }),
            tx,
            stdin: tokio::sync::Mutex::new(None),
            cancel: Notify::new(),
        });
        list.push(job.clone());
        Ok(job)
    }

    pub fn get(&self, id: u64) -> Option<Arc<Job>> {
        self.list.lock().unwrap().iter().find(|j| j.id == id).cloned()
    }

    /// Newest first.
    pub fn all(&self) -> Vec<Arc<Job>> {
        self.list.lock().unwrap().iter().rev().cloned().collect()
    }

    /// The job currently holding the drive, if any.
    pub fn busy(&self) -> Option<Arc<Job>> {
        self.list.lock().unwrap().iter().find(|j| j.exclusive && !j.status().is_terminal()).cloned()
    }
}

// ── Child-process runner ─────────────────────────────────────────────────────

/// Run `rustydisc <args>` as a child process and feed its output into `job`.
pub async fn run_process(job: Arc<Job>, exe: std::path::PathBuf, args: Vec<String>) {
    let mut cmd = Command::new(exe);
    cmd.args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true);

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            fail(&job, "IO_ERROR", &format!("Could not start rustydisc: {e}"), false);
            return;
        }
    };
    let pid = child.id();
    *job.stdin.lock().await = child.stdin.take();

    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();

    let out_job = job.clone();
    let out_task = tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            handle_stdout_line(&out_job, &line);
        }
    });

    let err_job = job.clone();
    let err_task = tokio::spawn(async move { read_stderr(err_job, stderr).await });

    let mut cancelled = false;
    let exit = tokio::select! {
        r = child.wait() => r,
        _ = job.cancel_notified().notified() => {
            cancelled = true;
            kill_group(pid).await;
            child.wait().await
        }
    };

    let _ = out_task.await;
    let stderr_lines = err_task.await.unwrap_or_default();
    *job.stdin.lock().await = None;

    if cancelled {
        job.push(Event::Step { msg: "Cancelled".into() });
        job.push(Event::Status { status: Status::Cancelled });
        return;
    }

    match exit {
        Ok(status) if status.success() => {
            job.push(Event::Progress { pct: 100.0 });
            job.push(Event::Status { status: Status::Done });
        }
        Ok(_) => {
            let (code, msg, rec) = extract_error(&stderr_lines);
            fail(&job, &code, &msg, rec);
        }
        Err(e) => fail(&job, "IO_ERROR", &e.to_string(), false),
    }
}

pub fn fail(job: &Job, code: &str, msg: &str, recoverable: bool) {
    job.push(Event::Error {
        error: code.to_string(),
        message: msg.to_string(),
        recoverable,
    });
    job.push(Event::Status { status: Status::Failed });
}

async fn kill_group(pid: Option<u32>) {
    if let Some(pid) = pid {
        // Negative pid = whole process group (rustydisc plus cdparanoia/cdrdao/xorriso children).
        let _ = Command::new("kill")
            .args(["-TERM", "--", &format!("-{pid}")])
            .status()
            .await;
    }
}

fn handle_stdout_line(job: &Job, line: &str) {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return;
    }
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
        match v.get("type").and_then(|t| t.as_str()) {
            Some("progress") => {
                if let Some(p) = v.get("pct").and_then(|p| p.as_f64()) {
                    job.push(Event::Progress { pct: p as f32 });
                }
                return;
            }
            Some("step") => {
                let msg = v.get("msg").and_then(|m| m.as_str()).unwrap_or("").to_string();
                job.push(Event::Step { msg });
                return;
            }
            Some("done") => return,
            _ => {}
        }
    }
    job.push(Event::Log { msg: trimmed.to_string() });
}

/// stderr is read as raw chunks (not lines) because interactive prompts such as
/// "Insert blank disc 2 … press ENTER" are written without a trailing newline.
async fn read_stderr(job: Arc<Job>, mut stderr: tokio::process::ChildStderr) -> Vec<String> {
    let mut all = Vec::new();
    let mut buf = [0u8; 4096];
    let mut pending = String::new();

    let mut flush_lines = |pending: &mut String, all: &mut Vec<String>, job: &Job, force: bool| {
        while let Some(pos) = pending.find(['\n', '\r']) {
            let line: String = pending.drain(..=pos).collect();
            let line = line.trim().to_string();
            if !line.is_empty() {
                all.push(line.clone());
                job.push(Event::Log { msg: line });
            }
        }
        if force || pending.contains("ENTER") {
            let line = pending.trim().to_string();
            pending.clear();
            if !line.is_empty() {
                all.push(line.clone());
                if line.contains("ENTER") {
                    job.push(Event::Input { msg: line.trim_end_matches("...").trim().to_string() });
                    job.push(Event::Status { status: Status::AwaitingInput });
                } else {
                    job.push(Event::Log { msg: line });
                }
            }
        }
    };

    loop {
        match stderr.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                pending.push_str(&String::from_utf8_lossy(&buf[..n]));
                flush_lines(&mut pending, &mut all, &job, false);
            }
        }
    }
    flush_lines(&mut pending, &mut all, &job, true);
    all
}

/// `main` prints failures as a pretty-printed `DiscError` JSON object on stderr.
fn extract_error(lines: &[String]) -> (String, String, bool) {
    if let Some(start) = lines.iter().rposition(|l| l == "{") {
        let json = lines[start..].join("\n");
        if let Ok(e) = serde_json::from_str::<DiscError>(&json) {
            return (e.error, e.message, e.recoverable);
        }
    }
    let tail: Vec<&str> = lines.iter().rev().take(5).map(|s| s.as_str()).collect();
    let msg = if tail.is_empty() {
        "Process exited with an error".to_string()
    } else {
        tail.into_iter().rev().collect::<Vec<_>>().join("\n")
    };
    ("BACKEND_ERROR".into(), msg, true)
}
