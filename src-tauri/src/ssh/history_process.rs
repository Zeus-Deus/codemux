//! Cancellation-safe subprocesses for the bounded workspace history phase.

use std::io;
use std::process::{Output, Stdio};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::process::{Child, Command};

pub(crate) fn ssh_command(target: &str) -> Command {
    super::exec::verified_ssh_command(target, 10)
}

pub(crate) async fn within_deadline<T>(
    budget: std::time::Duration,
    transfer: impl std::future::Future<Output = Result<T, String>>,
) -> Result<T, String> {
    tokio::time::timeout(budget, transfer)
        .await
        .map_err(|_| format!("workspace history sync exceeded {budget:?}"))?
}

struct ProcessGroup(Child);

impl ProcessGroup {
    fn spawn(command: &mut Command) -> io::Result<Self> {
        command
            .process_group(0)
            .kill_on_drop(true)
            .spawn()
            .map(Self)
    }
}

impl Drop for ProcessGroup {
    fn drop(&mut self) {
        if let Some(pid) = self
            .0
            .id()
            .and_then(|id| i32::try_from(id).ok())
            .filter(|pid| *pid > 1)
        {
            // SAFETY: The unreaped child is the leader of the process group
            // we created. The signal reaches its helpers, including rsync's SSH
            // process. Tokio also kills and reaps the direct child on drop.
            unsafe { libc::killpg(pid, libc::SIGKILL) };
        }
    }
}

pub(crate) async fn output(command: &mut Command, input: Option<&[u8]>) -> io::Result<Output> {
    command
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = ProcessGroup::spawn(command)?;
    let input_pipe = child.0.stdin.take();
    let mut output_pipe = child.0.stdout.take().expect("piped stdout");
    let mut error_pipe = child.0.stderr.take().expect("piped stderr");
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let write_input = async move {
        if let (Some(mut pipe), Some(data)) = (input_pipe, input) {
            pipe.write_all(data).await?;
        }
        Ok::<(), io::Error>(())
    };
    // Keep the leader unreaped while descendants can still hold a pipe open.
    // This keeps the group ID valid if the caller cancels during collection.
    tokio::try_join!(
        write_input,
        output_pipe.read_to_end(&mut stdout),
        error_pipe.read_to_end(&mut stderr),
    )?;
    let status = child.0.wait().await?;
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::io::{AsyncBufReadExt as _, BufReader};

    #[tokio::test]
    async fn history_process_group_closes_transport_pipes_on_cancellation() {
        let mut command = Command::new("sh");
        command
            .args(["-c", "sleep 60 & echo ready; wait"])
            .stdout(Stdio::piped());
        let mut child = ProcessGroup::spawn(&mut command).unwrap();
        let mut stdout = BufReader::new(child.0.stdout.take().unwrap());
        let mut line = String::new();
        tokio::time::timeout(Duration::from_secs(3), stdout.read_line(&mut line))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(line.trim(), "ready");

        drop(child);
        line.clear();
        let count = tokio::time::timeout(Duration::from_secs(3), stdout.read_line(&mut line))
            .await
            .expect("the shell and its child both released stdout")
            .unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn history_process_collects_both_streams_and_input() {
        let mut command = Command::new("sh");
        command.args(["-c", "cat; printf notice >&2"]);
        let out = output(&mut command, Some(b"history fixture"))
            .await
            .unwrap();
        assert!(out.status.success());
        assert_eq!(out.stdout, b"history fixture");
        assert_eq!(out.stderr, b"notice");
    }
}
