use crate::error::{ComponentError, Result};
use std::{
    ffi::{OsStr, OsString},
    io::Read,
    path::Path,
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};

const OUTPUT_TAIL_BYTES: usize = 8 * 1024;
const STDOUT_LIMIT_BYTES: usize = 1024 * 1024;

struct CapturedStdout {
    text: String,
    exceeded_limit: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ProcessOutput {
    pub success: bool,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

/// Narrow process boundary. Callers pass only paths and pre-built whitelisted
/// argument vectors; it is deliberately not a generic shell executor.
pub trait ProcessRunner: Send + Sync {
    fn run(
        &self,
        executable: &Path,
        args: &[OsString],
        cancelled: &AtomicBool,
    ) -> Result<ProcessOutput>;

    /// Run a short health check with a hard deadline. Test runners can rely on
    /// the default implementation; the system runner enforces the timeout.
    fn run_with_timeout(
        &self,
        executable: &Path,
        args: &[OsString],
        cancelled: &AtomicBool,
        _timeout: Duration,
    ) -> Result<ProcessOutput> {
        self.run(executable, args, cancelled)
    }
}

#[derive(Debug, Default)]
pub struct SystemProcessRunner;

impl ProcessRunner for SystemProcessRunner {
    fn run(
        &self,
        executable: &Path,
        args: &[OsString],
        cancelled: &AtomicBool,
    ) -> Result<ProcessOutput> {
        run_process(executable, args, cancelled, None)
    }

    fn run_with_timeout(
        &self,
        executable: &Path,
        args: &[OsString],
        cancelled: &AtomicBool,
        timeout: Duration,
    ) -> Result<ProcessOutput> {
        run_process(executable, args, cancelled, Some(timeout))
    }
}

fn run_process(
    executable: &Path,
    args: &[OsString],
    cancelled: &AtomicBool,
    timeout: Option<Duration>,
) -> Result<ProcessOutput> {
    if cancelled.load(Ordering::Acquire) {
        return Err(ComponentError::Cancelled);
    }
    let started = Instant::now();
    let mut child = Command::new(executable)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(ComponentError::Io)?;
    // Drain both pipes while the child is running. Waiting for the process
    // before reading can deadlock once FFmpeg fills either OS pipe buffer.
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ComponentError::State("child stdout pipe was unavailable".into()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| ComponentError::State("child stderr pipe was unavailable".into()))?;
    let stdout_reader = thread::spawn(move || read_stdout_bounded(stdout));
    let stderr_reader = thread::spawn(move || read_tail(stderr));
    loop {
        let was_cancelled = cancelled.load(Ordering::Acquire);
        let timed_out = timeout.is_some_and(|limit| started.elapsed() >= limit);
        if was_cancelled || timed_out {
            let _ = child.kill();
            for _ in 0..20 {
                if child.try_wait().ok().flatten().is_some() {
                    break;
                }
                thread::sleep(Duration::from_millis(10));
            }
            // Do not block on pipe-reader joins here. A hostile executable can
            // pass inherited pipe handles to a descendant; detaching the
            // readers keeps cancellation and the health-check deadline hard.
            drop(stdout_reader);
            drop(stderr_reader);
            return if was_cancelled {
                Err(ComponentError::Cancelled)
            } else {
                Err(ComponentError::Process {
                    message: format!(
                        "process health check timed out after {:.1} seconds",
                        timeout.unwrap_or_default().as_secs_f32()
                    ),
                    stderr: String::new(),
                })
            };
        }
        if child.try_wait().map_err(ComponentError::Io)?.is_some() {
            break;
        }
        thread::sleep(Duration::from_millis(25));
    }
    let status = child.wait().map_err(ComponentError::Io)?;
    let stdout = join_stdout(stdout_reader)?;
    let stderr = join_reader(stderr_reader)?;
    if stdout.exceeded_limit {
        return Err(ComponentError::Process {
            message: format!(
                "process stdout exceeded the {} byte structured-output limit",
                STDOUT_LIMIT_BYTES
            ),
            stderr,
        });
    }
    Ok(ProcessOutput {
        success: status.success(),
        exit_code: status.code(),
        stdout: stdout.text,
        stderr,
    })
}

fn read_stdout_bounded(mut reader: impl Read) -> std::io::Result<CapturedStdout> {
    let mut output = Vec::with_capacity(STDOUT_LIMIT_BYTES.min(64 * 1024));
    let mut exceeded_limit = false;
    let mut buffer = [0_u8; 4096];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        let remaining = STDOUT_LIMIT_BYTES.saturating_sub(output.len());
        let keep = remaining.min(read);
        output.extend_from_slice(&buffer[..keep]);
        exceeded_limit |= keep < read;
    }
    Ok(CapturedStdout {
        text: String::from_utf8_lossy(&output).into_owned(),
        exceeded_limit,
    })
}

fn read_tail(mut reader: impl Read) -> std::io::Result<String> {
    let mut tail = Vec::with_capacity(OUTPUT_TAIL_BYTES);
    let mut buffer = [0_u8; 4096];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        if read >= OUTPUT_TAIL_BYTES {
            tail.clear();
            tail.extend_from_slice(&buffer[read - OUTPUT_TAIL_BYTES..read]);
        } else {
            let excess = tail
                .len()
                .saturating_add(read)
                .saturating_sub(OUTPUT_TAIL_BYTES);
            if excess > 0 {
                tail.drain(..excess);
            }
            tail.extend_from_slice(&buffer[..read]);
        }
    }
    Ok(String::from_utf8_lossy(&tail).into_owned())
}

fn join_reader(reader: thread::JoinHandle<std::io::Result<String>>) -> Result<String> {
    reader
        .join()
        .map_err(|_| ComponentError::State("process output reader panicked".into()))?
        .map_err(ComponentError::Io)
}

fn join_stdout(
    reader: thread::JoinHandle<std::io::Result<CapturedStdout>>,
) -> Result<CapturedStdout> {
    reader
        .join()
        .map_err(|_| ComponentError::State("process stdout reader panicked".into()))?
        .map_err(ComponentError::Io)
}

pub(crate) fn os_args<I, S>(values: I) -> Vec<OsString>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    values.into_iter().map(|v| v.as_ref().to_owned()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tail_reader_keeps_only_the_last_eight_kib() {
        let input = vec![b'x'; OUTPUT_TAIL_BYTES + 17];
        assert_eq!(
            read_tail(std::io::Cursor::new(input)).unwrap().len(),
            OUTPUT_TAIL_BYTES
        );
    }

    #[test]
    fn stdout_reader_keeps_complete_bounded_output_and_marks_overflow() {
        let exact = vec![b'x'; STDOUT_LIMIT_BYTES];
        let captured = read_stdout_bounded(std::io::Cursor::new(exact)).unwrap();
        assert_eq!(captured.text.len(), STDOUT_LIMIT_BYTES);
        assert!(!captured.exceeded_limit);

        let oversized = vec![b'y'; STDOUT_LIMIT_BYTES + 1];
        let captured = read_stdout_bounded(std::io::Cursor::new(oversized)).unwrap();
        assert_eq!(captured.text.len(), STDOUT_LIMIT_BYTES);
        assert!(captured.exceeded_limit);
    }

    #[cfg(windows)]
    #[test]
    fn system_runner_enforces_health_check_timeout() {
        let runner = SystemProcessRunner;
        let result = runner.run_with_timeout(
            Path::new("powershell.exe"),
            &os_args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Start-Sleep -Seconds 30",
            ]),
            &AtomicBool::new(false),
            Duration::from_millis(100),
        );
        assert!(matches!(result, Err(ComponentError::Process { .. })));
    }
}
