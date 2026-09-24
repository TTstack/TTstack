//! Bounded execution of synchronous host commands, off the HTTP executor.
use std::io::{Read, Seek, SeekFrom};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

pub trait CommandExt {
    fn bounded_output(&mut self) -> std::io::Result<Output> {
        self.output_timeout(Duration::from_secs(300))
    }
    fn output_timeout(&mut self, timeout: Duration) -> std::io::Result<Output>;
}

impl CommandExt for Command {
    fn output_timeout(&mut self, timeout: Duration) -> std::io::Result<Output> {
        // Files avoid pipe deadlocks and reader threads lingering after a timeout.
        let mut stdout = tempfile::tempfile()?;
        let mut stderr = tempfile::tempfile()?;
        self.stdout(Stdio::from(stdout.try_clone()?));
        self.stderr(Stdio::from(stderr.try_clone()?));
        let mut child = self.spawn()?;
        let start = Instant::now();
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if start.elapsed() >= timeout {
                let _ = child.kill();
                let _ = child.wait();
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!(
                        "{} timed out after {}s",
                        self.get_program().to_string_lossy(),
                        timeout.as_secs()
                    ),
                ));
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        stdout.seek(SeekFrom::Start(0))?;
        stderr.seek(SeekFrom::Start(0))?;
        let mut out = Vec::new();
        let mut err = Vec::new();
        stdout.take(4 * 1024 * 1024).read_to_end(&mut out)?;
        stderr.take(4 * 1024 * 1024).read_to_end(&mut err)?;
        Ok(Output {
            status,
            stdout: out,
            stderr: err,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn command_timeout_and_output() {
        let err = Command::new("sleep")
            .arg("10")
            .output_timeout(Duration::from_millis(50))
            .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
        let out = Command::new("printf")
            .arg("hello")
            .bounded_output()
            .unwrap();
        assert!(out.status.success());
        assert_eq!(out.stdout, b"hello");
    }
}
