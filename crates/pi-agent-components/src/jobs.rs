use crate::error::{ComponentError, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobStatus {
    pub id: u64,
    pub state: JobState,
    pub stage: String,
    pub progress: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JobError>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JobError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

impl JobError {
    fn from_component(error: &ComponentError) -> Self {
        Self {
            code: error.code().into(),
            message: error.to_string(),
            details: error.details(),
        }
    }
}

impl JobStatus {
    fn queued(id: u64) -> Self {
        Self {
            id,
            state: JobState::Queued,
            stage: "queued".into(),
            progress: 0.0,
            result: None,
            error: None,
        }
    }
}

#[derive(Clone)]
pub struct JobProgress {
    state: Arc<Mutex<JobStatus>>,
    cancelled: Arc<AtomicBool>,
}

impl JobProgress {
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
    /// Read-only cancellation token for component operations that poll during
    /// downloads or process execution. Callers cannot reset cancellation.
    pub fn cancellation_flag(&self) -> &AtomicBool {
        self.cancelled.as_ref()
    }
    pub fn check_cancelled(&self) -> Result<()> {
        if self.is_cancelled() {
            Err(ComponentError::Cancelled)
        } else {
            Ok(())
        }
    }
    pub fn set(&self, stage: impl Into<String>, progress: f32) {
        let mut state = self.state.lock().expect("job status lock poisoned");
        state.stage = stage.into();
        state.progress = progress.clamp(0.0, 1.0);
    }
}

pub struct JobHandle {
    state: Arc<Mutex<JobStatus>>,
    cancelled: Arc<AtomicBool>,
    join: Mutex<Option<JoinHandle<()>>>,
}

impl JobHandle {
    pub fn start<F>(id: u64, operation: F) -> Self
    where
        F: FnOnce(JobProgress) -> Result<Value> + Send + 'static,
    {
        let state = Arc::new(Mutex::new(JobStatus::queued(id)));
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_state = Arc::clone(&state);
        let worker_cancelled = Arc::clone(&cancelled);
        let join = thread::spawn(move || {
            {
                let mut status = worker_state.lock().expect("job status lock poisoned");
                status.state = JobState::Running;
                status.stage = "running".into();
            }
            let progress = JobProgress {
                state: Arc::clone(&worker_state),
                cancelled: Arc::clone(&worker_cancelled),
            };
            let outcome = operation(progress.clone());
            let mut status = worker_state.lock().expect("job status lock poisoned");
            match outcome {
                // Once an operation has crossed its commit point and returned
                // success, a late cancellation request must not erase the
                // result or claim the committed mutation was cancelled.
                Ok(value) => {
                    status.state = JobState::Succeeded;
                    status.stage = "complete".into();
                    status.progress = 1.0;
                    status.result = Some(value);
                }
                Err(ComponentError::Cancelled) => {
                    status.state = JobState::Cancelled;
                    status.stage = "cancelled".into();
                }
                Err(error) => {
                    status.state = JobState::Failed;
                    status.stage = "failed".into();
                    status.error = Some(JobError::from_component(&error));
                }
            }
        });
        Self {
            state,
            cancelled,
            join: Mutex::new(Some(join)),
        }
    }
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
    pub fn status(&self) -> JobStatus {
        self.state.lock().expect("job status lock poisoned").clone()
    }
    pub fn wait(&self) {
        if let Some(join) = self.join.lock().expect("job join lock poisoned").take() {
            let _ = join.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn job_exposes_structured_success() {
        let job = JobHandle::start(7, |p| {
            p.set("download", 0.5);
            Ok(serde_json::json!({"ok": true}))
        });
        job.wait();
        let status = job.status();
        assert_eq!(status.state, JobState::Succeeded);
        assert_eq!(status.result, Some(serde_json::json!({"ok": true})));
    }
    #[test]
    fn cancellation_becomes_terminal_cancelled() {
        let job = JobHandle::start(8, |p| {
            while !p.is_cancelled() {
                std::thread::yield_now();
            }
            p.check_cancelled()?;
            Ok(Value::Null)
        });
        job.cancel();
        job.wait();
        assert_eq!(job.status().state, JobState::Cancelled);
    }
    #[test]
    fn failures_have_stable_machine_code() {
        let job = JobHandle::start(9, |_p| Err(ComponentError::InvalidInput("bad path".into())));
        job.wait();
        let error = job.status().error.unwrap();
        assert_eq!(error.code, "invalid_input");
        assert_eq!(error.message, "invalid input: bad path");
    }
    #[test]
    fn progress_exposes_a_read_only_cancellation_flag() {
        let job = JobHandle::start(10, |p| {
            assert!(!p.cancellation_flag().load(Ordering::Acquire));
            Ok(Value::Null)
        });
        job.wait();
        assert_eq!(job.status().state, JobState::Succeeded);
    }

    #[test]
    fn cancellation_after_commit_does_not_erase_success() {
        let committed = Arc::new(AtomicBool::new(false));
        let may_return = Arc::new(AtomicBool::new(false));
        let worker_committed = Arc::clone(&committed);
        let worker_may_return = Arc::clone(&may_return);
        let job = JobHandle::start(11, move |_progress| {
            worker_committed.store(true, Ordering::Release);
            while !worker_may_return.load(Ordering::Acquire) {
                std::thread::yield_now();
            }
            Ok(serde_json::json!({"committed": true}))
        });
        while !committed.load(Ordering::Acquire) {
            std::thread::yield_now();
        }
        job.cancel();
        may_return.store(true, Ordering::Release);
        job.wait();
        assert_eq!(job.status().state, JobState::Succeeded);
    }
}
