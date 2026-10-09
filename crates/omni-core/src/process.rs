//! Bounded subprocesses (ffmpeg, yt-dlp, pdfinfo, cupsfilter).

use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWriteExt as _};

/// Exit status and captured, capped output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Output {
    pub status: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

#[derive(Debug, thiserror::Error)]
pub enum ProcessError {
    #[error("failed to spawn process: {0}")]
    Spawn(#[source] std::io::Error),
    #[error("process I/O failed: {0}")]
    Io(#[source] std::io::Error),
    #[error("process timed out after {0:?}")]
    Timeout(Duration),
    #[error("process {stream} exceeded {limit} bytes")]
    OutputTooLarge { stream: &'static str, limit: usize },
    #[error("process terminated by signal")]
    Signaled,
}

/// Runs `cmd` with `kill_on_drop`, optional stdin, capped stdout/stderr and a
/// timeout; exceeding a cap or the timeout kills the child and errors.
///
/// A non-zero exit is not an error: callers inspect [`Output::status`]. A
/// child terminated by a signal yields [`ProcessError::Signaled`].
pub async fn run_bounded(
    mut cmd: tokio::process::Command,
    stdin: Option<Vec<u8>>,
    stdout_cap: usize,
    stderr_cap: usize,
    timeout: Duration,
) -> Result<Output, ProcessError> {
    cmd.kill_on_drop(true)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(ProcessError::Spawn)?;
    let child_stdin = child.stdin.take();
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    let io = async {
        let write_stdin = async {
            if let (Some(mut pipe), Some(bytes)) = (child_stdin, stdin) {
                match pipe.write_all(&bytes).await {
                    Ok(()) => {}
                    // The child may exit without reading all of stdin.
                    Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => {}
                    Err(e) => return Err(ProcessError::Io(e)),
                }
                drop(pipe);
            }
            Ok(())
        };
        let (_, out, err) = tokio::try_join!(
            write_stdin,
            read_capped(stdout, stdout_cap, "stdout"),
            read_capped(stderr, stderr_cap, "stderr"),
        )?;
        let status = child.wait().await.map_err(ProcessError::Io)?;
        Ok::<_, ProcessError>((status, out, err))
    };

    // On timeout or a cap error the future (and with it `child`) is dropped,
    // and `kill_on_drop` terminates the process.
    let (status, stdout, stderr) = tokio::time::timeout(timeout, io)
        .await
        .map_err(|_| ProcessError::Timeout(timeout))??;
    let status = status.code().ok_or(ProcessError::Signaled)?;
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

async fn read_capped<R: AsyncRead + Unpin>(
    pipe: Option<R>,
    cap: usize,
    stream: &'static str,
) -> Result<Vec<u8>, ProcessError> {
    let Some(pipe) = pipe else {
        return Ok(Vec::new());
    };
    let limit = u64::try_from(cap).unwrap_or(u64::MAX).saturating_add(1);
    let mut buffer = Vec::new();
    pipe.take(limit)
        .read_to_end(&mut buffer)
        .await
        .map_err(ProcessError::Io)?;
    if buffer.len() > cap {
        return Err(ProcessError::OutputTooLarge { stream, limit: cap });
    }
    Ok(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::process::Command;

    fn sh(script: &str) -> Command {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(script);
        cmd
    }

    #[tokio::test]
    async fn captures_output_and_status() {
        let out = run_bounded(
            sh("cat; echo err >&2; exit 3"),
            Some(b"hello".to_vec()),
            1024,
            1024,
            Duration::from_secs(10),
        )
        .await
        .expect("runs");
        assert_eq!(out.status, 3);
        assert_eq!(out.stdout, b"hello");
        assert_eq!(out.stderr, b"err\n");
    }

    #[tokio::test]
    async fn output_cap_is_an_error() {
        let res = run_bounded(
            sh("head -c 5000 /dev/zero"),
            None,
            100,
            100,
            Duration::from_secs(10),
        )
        .await;
        assert!(matches!(
            res,
            Err(ProcessError::OutputTooLarge {
                stream: "stdout",
                limit: 100
            })
        ));
    }

    #[tokio::test]
    async fn timeout_kills_the_child() {
        let res = run_bounded(sh("sleep 5"), None, 100, 100, Duration::from_millis(100)).await;
        assert!(matches!(res, Err(ProcessError::Timeout(_))));
    }

    #[tokio::test]
    async fn missing_binary_fails_to_spawn() {
        let res = run_bounded(
            Command::new("/nonexistent/omni-binary"),
            None,
            1,
            1,
            Duration::from_secs(1),
        )
        .await;
        assert!(matches!(res, Err(ProcessError::Spawn(_))));
    }
}
