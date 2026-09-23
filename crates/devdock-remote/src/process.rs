//! Bounded subprocess helpers for remote startup operations.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

/// No individual Coder/SSH setup command may hold a claimed session forever.
pub const REMOTE_SETUP_COMMAND_TIMEOUT: Duration = Duration::from_secs(120);

/// Capture command output without allowing an unbounded child process to block
/// remote startup. Temporary files avoid a full stdout/stderr pipe deadlock
/// while the parent waits for a slow Coder proxy command.
pub(crate) fn output_with_timeout(command: &mut Command, label: &str) -> std::io::Result<Output> {
    output_with_deadline(command, label, REMOTE_SETUP_COMMAND_TIMEOUT)
}

fn output_with_deadline(
    command: &mut Command,
    label: &str,
    timeout: Duration,
) -> std::io::Result<Output> {
    let mut stdout = tempfile::tempfile()?;
    let mut stderr = tempfile::tempfile()?;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout.try_clone()?))
        .stderr(Stdio::from(stderr.try_clone()?));
    let mut child = command.spawn()?;
    let started = Instant::now();

    let status = loop {
        match child.try_wait()? {
            Some(status) => break status,
            None if started.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!(
                        "{label} exceeded the {}s remote setup command deadline",
                        timeout.as_secs()
                    ),
                ));
            }
            None => std::thread::sleep(Duration::from_millis(100)),
        }
    };

    read_output_file(&mut stdout).and_then(|stdout| {
        read_output_file(&mut stderr).map(|stderr| Output { status, stdout, stderr })
    })
}

fn read_output_file(file: &mut File) -> std::io::Result<Vec<u8>> {
    file.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_setup_command_deadline_is_bounded() {
        assert_eq!(REMOTE_SETUP_COMMAND_TIMEOUT, Duration::from_secs(120));
    }

    #[test]
    fn deadline_kills_a_stalled_child() {
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 1"]);

        let error = output_with_deadline(&mut command, "test stalled command", Duration::ZERO)
            .expect_err("stalled child should be killed at the deadline");

        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        assert!(error.to_string().contains("test stalled command exceeded"));
    }
}
