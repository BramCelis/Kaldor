//! Worker pool for track analysis. Jobs go in over a channel; results come
//! back over another one, and the owner of the `Library` stores them. No
//! locks are shared with the workers.

use crate::model::TrackId;
use dj_analysis::{Analysis, AnalysisOptions, analyze_file};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::JoinHandle;

/// One analysis request.
#[derive(Clone, Debug)]
pub struct AnalysisJob {
    pub track: TrackId,
    pub path: PathBuf,
}

/// One analysis result.
#[derive(Debug)]
pub struct AnalysisDone {
    pub track: TrackId,
    pub result: Result<Box<Analysis>, String>,
}

/// A pool of analysis threads.
pub struct AnalysisQueue {
    tx: Option<crossbeam_channel::Sender<AnalysisJob>>,
    rx: crossbeam_channel::Receiver<AnalysisDone>,
    workers: Vec<JoinHandle<()>>,
    pending: Arc<AtomicUsize>,
}

impl AnalysisQueue {
    /// Starts `workers` threads (at least one).
    pub fn new(workers: usize, opts: AnalysisOptions) -> Self {
        let (job_tx, job_rx) = crossbeam_channel::unbounded::<AnalysisJob>();
        let (done_tx, done_rx) = crossbeam_channel::unbounded::<AnalysisDone>();
        let pending = Arc::new(AtomicUsize::new(0));
        let workers = (0..workers.max(1))
            .filter_map(|i| {
                let job_rx = job_rx.clone();
                let done_tx = done_tx.clone();
                std::thread::Builder::new()
                    .name(format!("kaldor-analysis-{i}"))
                    .spawn(move || {
                        for job in job_rx {
                            let result = analyze_file(&job.path, &opts)
                                .map(|(_, a)| Box::new(a))
                                .map_err(|e| e.to_string());
                            if done_tx
                                .send(AnalysisDone {
                                    track: job.track,
                                    result,
                                })
                                .is_err()
                            {
                                break;
                            }
                        }
                    })
                    .ok()
            })
            .collect();
        Self {
            tx: Some(job_tx),
            rx: done_rx,
            workers,
            pending,
        }
    }

    /// A sensible worker count: half the cores, at least 1, at most 4 (each
    /// worker holds one decoded track in memory).
    pub fn default_workers() -> usize {
        std::thread::available_parallelism()
            .map_or(2, |n| n.get() / 2)
            .clamp(1, 4)
    }

    pub fn submit(&self, job: AnalysisJob) {
        if let Some(tx) = &self.tx
            && tx.send(job).is_ok()
        {
            self.pending.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Next finished result, if any (non-blocking).
    pub fn try_recv(&self) -> Option<AnalysisDone> {
        let r = self.rx.try_recv().ok();
        if r.is_some() {
            self.pending.fetch_sub(1, Ordering::Relaxed);
        }
        r
    }

    /// Waits up to `timeout` for the next result.
    pub fn recv_timeout(&self, timeout: std::time::Duration) -> Option<AnalysisDone> {
        let r = self.rx.recv_timeout(timeout).ok();
        if r.is_some() {
            self.pending.fetch_sub(1, Ordering::Relaxed);
        }
        r
    }

    /// Jobs submitted but not yet received.
    pub fn pending(&self) -> usize {
        self.pending.load(Ordering::Relaxed)
    }
}

impl Drop for AnalysisQueue {
    fn drop(&mut self) {
        // Closing the job channel ends the workers after their current job.
        self.tx = None;
        while self.rx.try_recv().is_ok() {}
        for w in self.workers.drain(..) {
            let _ = w.join();
        }
    }
}
