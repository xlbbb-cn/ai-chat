//! Background conversion jobs.
//!
//! The desktop app cannot block its UI thread on a 400-page paper, and a batch
//! run cannot start 200 parses at once. This module is the thin layer between
//! "the user pressed Convert" and [`crate::pipeline`]:
//!
//! * **submit** — queue a file, get a [`JobHandle`] immediately;
//! * **bounded concurrency** — at most `max_concurrency` conversions run at a
//!   time, the rest wait in `Queued`;
//! * **progress** — every pipeline stage and page updates the job record;
//! * **cancel** — an [`AtomicBool`] the pipeline polls between pages, so a
//!   cancelled job stops at the next boundary and never writes a half file;
//! * **wait** — a condition variable, for a batch runner and for tests.
//!
//! It is deliberately built on `std::thread` (one thread per submitted job,
//! gated by a semaphore) instead of pulling a runtime in: the library stays
//! usable from a synchronous context, and the app can await [`JobManager::wait`]
//! on a blocking thread or map [`JobStatus`] to its own event stream.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::error::{Error, Result};
use crate::options::Options;
use crate::pipeline::{self, Control, Progress, Stage};

/// Identifier of a submitted job (`job-0001`, …).
pub type JobId = String;

/// Lifecycle of a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobState {
    /// Waiting for a concurrency slot.
    Queued,
    /// Extraction/parsing in progress.
    Running,
    /// Finished successfully; the Markdown file exists.
    Done,
    /// Finished with an error.
    Failed,
    /// Stopped on request.
    Cancelled,
}

impl JobState {
    pub fn as_str(self) -> &'static str {
        match self {
            JobState::Queued => "queued",
            JobState::Running => "running",
            JobState::Done => "done",
            JobState::Failed => "failed",
            JobState::Cancelled => "cancelled",
        }
    }

    /// `true` for the three terminal states.
    pub fn is_finished(self) -> bool {
        matches!(self, JobState::Done | JobState::Failed | JobState::Cancelled)
    }
}

/// A snapshot of one job. Cheap to clone; the UI polls or caches it.
#[derive(Debug, Clone)]
pub struct JobStatus {
    pub id: JobId,
    pub state: JobState,
    pub input: PathBuf,
    pub output: PathBuf,
    pub stage: Stage,
    pub page: usize,
    pub total: usize,
    /// 0..=100, from [`Progress::percent`].
    pub percent: u8,
    /// Failure message, when `state == Failed`.
    pub error: Option<String>,
    /// Images written, when the job finished.
    pub assets: usize,
    /// Content blocks in the IR, when the job finished.
    pub blocks: usize,
    pub elapsed_ms: u128,
}

impl JobStatus {
    /// `true` when a cancel request would still have an effect.
    pub fn is_cancellable(&self) -> bool {
        !self.state.is_finished()
    }
}

/// A handle returned by [`JobManager::submit`].
#[derive(Debug, Clone)]
pub struct JobHandle {
    pub id: JobId,
    cancel: Arc<AtomicBool>,
}

impl JobHandle {
    /// The job id.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Ask the conversion to stop at the next page boundary.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// `true` when a cancel was requested.
    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

struct Token {
    id: JobId,
    cancel: Arc<AtomicBool>,
}

impl Control for Token {
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    fn job(&self) -> &str {
        &self.id
    }
}

#[derive(Debug)]
struct Record {
    status: JobStatus,
    cancel: Arc<AtomicBool>,
    started: Option<Instant>,
}

impl Record {
    fn snapshot(&self) -> JobStatus {
        let mut status = self.status.clone();
        if let Some(started) = self.started {
            status.elapsed_ms = started.elapsed().as_millis();
        }
        status
    }
}

#[derive(Debug, Default)]
struct Inner {
    jobs: HashMap<JobId, Record>,
    /// Completion counter, so `wait` can be woken without scanning.
    finished: usize,
}

/// A semaphore with `std` primitives (no dependency, no runtime).
#[derive(Debug)]
struct Semaphore {
    slots: Mutex<usize>,
    condvar: Condvar,
}

impl Semaphore {
    fn new(slots: usize) -> Self {
        Semaphore { slots: Mutex::new(slots), condvar: Condvar::new() }
    }

    /// Block until a slot is free.
    fn acquire(&self) {
        let mut slots = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        while *slots == 0 {
            slots = self.condvar.wait(slots).unwrap_or_else(|e| e.into_inner());
        }
        *slots -= 1;
    }

    fn release(&self) {
        let mut slots = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        *slots += 1;
        self.condvar.notify_one();
    }
}

/// Queue and run conversions in the background.
#[derive(Debug, Clone)]
pub struct JobManager {
    inner: Arc<(Mutex<Inner>, Condvar)>,
    semaphore: Arc<Semaphore>,
    counter: Arc<AtomicU64>,
    max_concurrency: usize,
}

impl Default for JobManager {
    fn default() -> Self {
        JobManager::new(4)
    }
}

impl JobManager {
    /// Create a manager running at most `max_concurrency` conversions at once.
    /// Values below 1 are raised to 1.
    pub fn new(max_concurrency: usize) -> Self {
        JobManager {
            inner: Arc::new((Mutex::new(Inner::default()), Condvar::new())),
            semaphore: Arc::new(Semaphore::new(max_concurrency.max(1))),
            counter: Arc::new(AtomicU64::new(0)),
            max_concurrency: max_concurrency.max(1),
        }
    }

    /// How many conversions may run at once.
    pub fn max_concurrency(&self) -> usize {
        self.max_concurrency
    }

    /// Queue `input` → `output` and return immediately.
    pub fn submit(
        &self,
        input: impl Into<PathBuf>,
        output: impl Into<PathBuf>,
        options: &Options,
    ) -> Result<JobHandle> {
        let input = input.into();
        let output = output.into();
        let id: JobId = format!("job-{:04}", self.counter.fetch_add(1, Ordering::Relaxed) + 1);
        let cancel = Arc::new(AtomicBool::new(false));
        let status = JobStatus {
            id: id.clone(),
            state: JobState::Queued,
            input: input.clone(),
            output: output.clone(),
            stage: Stage::Reading,
            page: 0,
            total: 0,
            percent: 0,
            error: None,
            assets: 0,
            blocks: 0,
            elapsed_ms: 0,
        };
        {
            let (lock, _) = &*self.inner;
            lock.lock()
                .unwrap_or_else(|e| e.into_inner())
                .jobs
                .insert(id.clone(), Record { status, cancel: cancel.clone(), started: None });
        }

        let manager = self.clone();
        let options = options.clone();
        let token = Token { id: id.clone(), cancel: cancel.clone() };
        std::thread::Builder::new()
            .name(format!("pdf2md-{id}"))
            .spawn(move || manager.work(input, output, options, token))
            .map_err(|e| Error::Other(format!("cannot start conversion thread: {e}")))?;

        Ok(JobHandle { id, cancel })
    }

    fn work(&self, input: PathBuf, output: PathBuf, options: Options, token: Token) {
        self.semaphore.acquire();
        let id = token.id.clone();
        let started = Instant::now();
        if token.cancelled() {
            self.finish(&id, JobState::Cancelled, None, 0, 0);
            self.semaphore.release();
            return;
        }
        self.update(&id, |status| {
            status.state = JobState::Running;
            status.stage = Stage::Reading;
        });
        {
            let (lock, _) = &*self.inner;
            if let Some(record) = lock.lock().unwrap_or_else(|e| e.into_inner()).jobs.get_mut(&id) {
                record.started = Some(started);
            }
        }

        let manager = self.clone();
        let mut progress = move |event: Progress| {
            manager.update(&id, |status| {
                status.stage = event.stage;
                status.page = event.page;
                status.total = event.total;
                status.percent = event.percent();
            });
        };

        let result = pipeline::convert_to_path_with(&input, &output, &options, &token, &mut progress);
        match result {
            Ok(conversion) => self.finish(
                &token.id,
                JobState::Done,
                None,
                conversion.assets.len(),
                conversion.document.content_block_count(),
            ),
            Err(Error::Cancelled { .. }) => {
                self.finish(&token.id, JobState::Cancelled, None, 0, 0)
            }
            Err(error) => self.finish(&token.id, JobState::Failed, Some(error.to_string()), 0, 0),
        }
        self.semaphore.release();
    }

    fn update(&self, id: &str, change: impl FnOnce(&mut JobStatus)) {
        let (lock, _) = &*self.inner;
        let mut inner = lock.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(record) = inner.jobs.get_mut(id) {
            change(&mut record.status);
        }
    }

    fn finish(&self, id: &str, state: JobState, error: Option<String>, assets: usize, blocks: usize) {
        let (lock, condvar) = &*self.inner;
        {
            let mut inner = lock.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(record) = inner.jobs.get_mut(id) {
                record.status.state = state;
                record.status.error = error;
                record.status.assets = assets;
                record.status.blocks = blocks;
                if state == JobState::Done {
                    record.status.percent = 100;
                }
                if let Some(started) = record.started {
                    record.status.elapsed_ms = started.elapsed().as_millis();
                }
            }
            inner.finished += 1;
        }
        condvar.notify_all();
    }

    /// Snapshot of one job.
    pub fn status(&self, id: &str) -> Option<JobStatus> {
        let (lock, _) = &*self.inner;
        lock.lock()
            .unwrap_or_else(|e| e.into_inner())
            .jobs
            .get(id)
            .map(Record::snapshot)
    }

    /// Every known job, oldest first.
    pub fn jobs(&self) -> Vec<JobStatus> {
        let (lock, _) = &*self.inner;
        let inner = lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut jobs: Vec<JobStatus> = inner.jobs.values().map(Record::snapshot).collect();
        jobs.sort_by(|a, b| a.id.cmp(&b.id));
        jobs
    }

    /// Jobs that reached a terminal state.
    pub fn finished(&self) -> Vec<JobStatus> {
        self.jobs().into_iter().filter(|job| job.state.is_finished()).collect()
    }

    /// Request cancellation. Returns `false` for an unknown id.
    pub fn cancel(&self, id: &str) -> bool {
        let (lock, condvar) = &*self.inner;
        let was_queued = {
            let mut inner = lock.lock().unwrap_or_else(|e| e.into_inner());
            if !inner.jobs.contains_key(id) {
                return false;
            }
            let queued = {
                let record = inner.jobs.get_mut(id).expect("checked above");
                record.cancel.store(true, Ordering::Relaxed);
                // A queued job never gets to see the flag if it is cancelled
                // before it starts, so its state is set here as well.
                if record.status.state == JobState::Queued {
                    record.status.state = JobState::Cancelled;
                    true
                } else {
                    false
                }
            };
            if queued {
                inner.finished += 1;
            }
            queued
        };
        if was_queued {
            condvar.notify_all();
        }
        true
    }

    /// Block until `id` finishes, or until `timeout` elapses.
    ///
    /// Returns the latest snapshot (which may still be `Running` on timeout), or
    /// `None` for an unknown id.
    pub fn wait(&self, id: &str, timeout: Duration) -> Option<JobStatus> {
        let (lock, condvar) = &*self.inner;
        let mut inner = lock.lock().unwrap_or_else(|e| e.into_inner());
        if !inner.jobs.contains_key(id) {
            return None;
        }
        let deadline = Instant::now() + timeout;
        loop {
            let finished = inner
                .jobs
                .get(id)
                .map(|record| record.status.state.is_finished())
                .unwrap_or(true);
            if finished {
                return inner.jobs.get(id).map(Record::snapshot);
            }
            let now = Instant::now();
            if now >= deadline {
                return inner.jobs.get(id).map(Record::snapshot);
            }
            let (guard, _) = condvar
                .wait_timeout(inner, deadline - now)
                .unwrap_or_else(|e| e.into_inner());
            inner = guard;
        }
    }

    /// Block until every job finishes (or the timeout elapses).
    pub fn wait_all(&self, timeout: Duration) -> Vec<JobStatus> {
        let (lock, condvar) = &*self.inner;
        let mut inner = lock.lock().unwrap_or_else(|e| e.into_inner());
        let deadline = Instant::now() + timeout;
        loop {
            if inner.jobs.values().all(|record| record.status.state.is_finished()) {
                break;
            }
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            let (guard, _) = condvar
                .wait_timeout(inner, deadline - now)
                .unwrap_or_else(|e| e.into_inner());
            inner = guard;
        }
        let mut jobs: Vec<JobStatus> = inner.jobs.values().map(Record::snapshot).collect();
        jobs.sort_by(|a, b| a.id.cmp(&b.id));
        jobs
    }

    /// Forget a finished job. Returns `false` when it is unknown or still active.
    pub fn remove(&self, id: &str) -> bool {
        let (lock, _) = &*self.inner;
        let mut inner = lock.lock().unwrap_or_else(|e| e.into_inner());
        let finished = inner
            .jobs
            .get(id)
            .map(|record| record.status.state.is_finished())
            .unwrap_or(false);
        if finished {
            inner.jobs.remove(id);
            true
        } else {
            false
        }
    }
}

/// Convenience: convert one file synchronously, reporting progress to `on_progress`.
pub fn convert_now(
    input: impl AsRef<Path>,
    output: impl AsRef<Path>,
    options: &Options,
    on_progress: &mut dyn FnMut(Progress),
) -> Result<crate::pipeline::Conversion> {
    pipeline::convert_to_path(input, output, options, on_progress)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::options::Options;

    fn temp_path(name: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!("pdf2md-jobs-{}-{}", std::process::id(), name));
        path
    }

    #[test]
    fn missing_input_fails_the_job() {
        let manager = JobManager::new(2);
        let handle = manager
            .submit(temp_path("missing.pdf"), temp_path("missing.md"), &Options::plain_text())
            .unwrap();
        let status = manager.wait(handle.id(), Duration::from_secs(10)).unwrap();
        assert_eq!(status.state, JobState::Failed);
        assert!(status.error.unwrap().contains("i/o error"));
    }

    #[test]
    fn a_queued_job_can_be_cancelled() {
        let manager = JobManager::new(1);
        // Occupy the single slot with a job that fails quickly.
        let blocker = manager
            .submit(temp_path("blocker.pdf"), temp_path("blocker.md"), &Options::plain_text())
            .unwrap();
        let input = temp_path("queued.pdf");
        std::fs::write(&input, b"not a pdf").unwrap();
        let handle = manager
            .submit(&input, temp_path("queued.md"), &Options::plain_text())
            .unwrap();
        assert!(manager.cancel(handle.id()));
        manager.wait(blocker.id(), Duration::from_secs(10));
        let status = manager.wait(handle.id(), Duration::from_secs(10)).unwrap();
        assert_eq!(status.state, JobState::Cancelled);
        let _ = std::fs::remove_file(&input);
    }

    #[test]
    fn unknown_jobs_are_reported() {
        let manager = JobManager::new(1);
        assert!(manager.status("nope").is_none());
        assert!(!manager.cancel("nope"));
        assert!(manager.wait("nope", Duration::from_millis(10)).is_none());
    }
}
