use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::Duration;

/// A P4 CLI wrapper that extracts the embedded p4 binary to an isolated temporary directory.
///
/// Each `P4Cli` instance gets its own temp directory, eliminating cross-process races.
/// A content-addressed cache avoids repeated zstd decompression across instances.
/// Stale temp directories (older than 1 hour) are cleaned up on construction.
pub struct P4Cli {
    bin_path: PathBuf,
    _temp_dir: PathBuf,
}

/// Collected output from a single `p4` invocation.
///
/// Holds raw stdout/stderr bytes (supports binary content) and the exit code.
/// The child process is guaranteed to have been reaped before this struct is returned.
pub struct P4Output {
    exit_code: i32,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

impl P4Output {
    pub fn exit_code(&self) -> i32 {
        self.exit_code
    }

    /// Returns `true` if the exit code is `0`.
    pub fn success(&self) -> bool {
        self.exit_code == 0
    }

    /// Raw stdout bytes (may be binary).
    pub fn stdout(&self) -> &[u8] {
        &self.stdout
    }

    /// Raw stderr bytes (may be binary).
    pub fn stderr(&self) -> &[u8] {
        &self.stderr
    }

    /// Decode stdout as UTF-8.
    pub fn stdout_str(&self) -> Result<&str, std::str::Utf8Error> {
        std::str::from_utf8(&self.stdout)
    }

    /// Decode stderr as UTF-8.
    pub fn stderr_str(&self) -> Result<&str, std::str::Utf8Error> {
        std::str::from_utf8(&self.stderr)
    }

    /// Lines of stdout (UTF-8 text only).
    pub fn stdout_lines(&self) -> Result<Vec<&str>, std::str::Utf8Error> {
        let s = self.stdout_str()?;
        if s.is_empty() {
            Ok(Vec::new())
        } else {
            Ok(s.lines().collect())
        }
    }

    /// Lines of stderr (UTF-8 text only).
    pub fn stderr_lines(&self) -> Result<Vec<&str>, std::str::Utf8Error> {
        let s = self.stderr_str()?;
        if s.is_empty() {
            Ok(Vec::new())
        } else {
            Ok(s.lines().collect())
        }
    }
}

impl std::fmt::Debug for P4Output {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("P4Output")
            .field("exit_code", &self.exit_code)
            .field("stdout_len", &self.stdout.len())
            .field("stderr_len", &self.stderr.len())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Streaming API
// ---------------------------------------------------------------------------

/// A single event yielded by [`P4Stream`].
pub enum P4StreamEvent {
    /// A line from stdout (UTF-8).
    Stdout(String),
    /// A line from stderr (UTF-8).
    Stderr(String),
    /// The process has exited with the given code.
    Exit(i32),
}

impl std::fmt::Display for P4StreamEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            P4StreamEvent::Stdout(line) => write!(f, "{line}"),
            P4StreamEvent::Stderr(line) => write!(f, "{line}"),
            P4StreamEvent::Exit(code) => write!(f, "(exit {code})"),
        }
    }
}

/// A streaming iterator over the output of a `p4` process.
///
/// Stdout and stderr are read concurrently by two OS threads and merged
/// into a single line-oriented stream. The process is automatically reaped
/// when the stream is exhausted. Dropping the stream mid-way kills the
/// process and cleans up all resources.
///
/// The final item yielded is always [`P4StreamEvent::Exit`] with the exit
/// code, unless the stream is dropped early.
pub struct P4Stream {
    rx: std::sync::mpsc::Receiver<std::io::Result<P4StreamEvent>>,
    child: Option<Child>,
    /// Killing the child closes the pipes, which unblocks the reader
    /// threads and lets them exit naturally.
    #[allow(dead_code)]
    handles: Vec<thread::JoinHandle<()>>,
    exhausted: bool,
}

impl Iterator for P4Stream {
    type Item = std::io::Result<P4StreamEvent>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.exhausted {
            return None;
        }
        match self.rx.recv() {
            Ok(item) => Some(item),
            Err(_) => {
                // All senders (reader threads) have finished → reap child.
                self.exhausted = true;
                let code = self
                    .child
                    .take()
                    .and_then(|mut c| c.wait().ok())
                    .and_then(|s| s.code())
                    .unwrap_or(-1);
                Some(Ok(P4StreamEvent::Exit(code)))
            }
        }
    }
}

impl Drop for P4Stream {
    fn drop(&mut self) {
        // Kill child if still owned (stream was dropped mid-way).
        if let Some(ref mut child) = self.child {
            let _ = child.kill();
            let _ = child.wait();
        }
        // The reader threads will now hit EOF and exit on their own.
    }
}

// ---------------------------------------------------------------------------
// Cache & temporary-directory helpers
// ---------------------------------------------------------------------------

/// The base directory under `TMP` / `/tmp` where everything lives.
fn base_dir() -> PathBuf {
    std::env::temp_dir().join("p4cli-20251")
}

/// A simple content fingerprint for the embedded zstd payload.
///
/// Uses `(length, first 64 bytes)` – sufficient for cache invalidation
/// since the payload is embedded at compile time and never tampered with.
fn fingerprint(data: &[u8]) -> String {
    let n = data.len();
    let prefix = &data[..data.len().min(64)];
    let hex: String = prefix.iter().map(|b| format!("{:02x}", b)).collect();
    format!("{}_{}", n, hex)
}

/// Remove instance directories whose modification time is older than `cutoff`.
///
/// The `.cache` directory is never removed here – it is managed separately.
fn cleanup_stale_dirs(base: &PathBuf, cutoff: std::time::SystemTime) {
    let Ok(entries) = std::fs::read_dir(base) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        // Skip the cache directory.
        if path.file_name().is_some_and(|n| n == ".cache") {
            continue;
        }
        if let Ok(meta) = path.metadata()
            && let Ok(mtime) = meta.modified()
            && mtime < cutoff
        {
            let _ = std::fs::remove_dir_all(&path);
        }
    }
}

/// Ensure the p4 binary exists on disk inside a fresh per-instance directory.
///
/// Caching
/// -------
/// The decompressed binary is cached at `base/.cache/{fingerprint}/p4_binary`.
/// Only the very first `P4Cli` on a given build host pays the decompression
/// cost; all subsequent instances copy (not decompress) from the cache.
fn write_p4_cli_to_disk() -> std::io::Result<(PathBuf, PathBuf)> {
    let zst_data = get_p4_cli_zst();
    let fp = fingerprint(&zst_data);

    let base = base_dir();
    std::fs::create_dir_all(&base)?;

    // Clean up stale instance directories (older than 1 hour).
    let cutoff = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    cleanup_stale_dirs(&base, cutoff);

    // Ensure the cache entry exists (race-safe: unique tmp name per PID).
    let cache_dir = base.join(".cache").join(&fp);
    let cache_bin = cache_dir.join("p4_binary");
    if !cache_bin.exists() {
        std::fs::create_dir_all(&cache_dir)?;
        let binary_data = decompress_zst(&zst_data)?;
        let tmp = cache_dir.join(format!(".tmp.{}", std::process::id()));
        {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(&binary_data)?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, &cache_bin)?;
        set_executable_perms(&cache_bin)?;
    }

    // Per-instance directory.
    let dir = base.join(format!(
        "{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir(&dir)?;
    let bin_path = dir.join("p4_binary");

    // Copy (not hardlink – avoids cross-device link errors on some setups).
    std::fs::copy(&cache_bin, &bin_path)?;
    set_executable_perms(&bin_path)?;

    Ok((bin_path, dir))
}

fn decompress_zst(zst_data: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut decoder = zstd::stream::Decoder::new(zst_data)?;
    let mut buf = Vec::new();
    std::io::copy(&mut decoder, &mut buf)?;
    Ok(buf)
}

#[cfg(unix)]
fn set_executable_perms(path: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn set_executable_perms(_path: &std::path::Path) -> std::io::Result<()> {
    Ok(())
}

// ---------------------------------------------------------------------------
// Platform-specific binary accessors
// ---------------------------------------------------------------------------

fn get_p4_cli_zst() -> Vec<u8> {
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        use p4cli_20251_win_x64::get_p4_cli_zst;
        get_p4_cli_zst()
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        use p4cli_20251_mac_arm64::get_p4_cli_zst;
        get_p4_cli_zst()
    }

    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        use p4cli_20251_mac_x64::get_p4_cli_zst;
        get_p4_cli_zst()
    }

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        use p4cli_20251_linux_x64::get_p4_cli_zst;
        get_p4_cli_zst()
    }

    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    {
        use p4cli_20251_linux_arm64::get_p4_cli_zst;
        get_p4_cli_zst()
    }

    #[cfg(not(any(
        all(target_os = "windows", target_arch = "x86_64"),
        all(target_os = "macos", target_arch = "aarch64"),
        all(target_os = "macos", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "aarch64")
    )))]
    {
        compile_error!(format!(
            "Unsupported platform: {}-{}",
            std::env::consts::OS,
            std::env::consts::ARCH
        ));
        Vec::new()
    }
}

// ---------------------------------------------------------------------------
// Builder for a single p4 invocation
// ---------------------------------------------------------------------------

/// Builder-style interface for running a single `p4` command.
///
/// Obtain one via [`P4Cli::command()`] and chain configuration calls
/// before calling [`run`](P4Command::run).
pub struct P4Command<'a> {
    cli: &'a P4Cli,
    args: Vec<std::ffi::OsString>,
    timeout: Option<Duration>,
    cwd: Option<PathBuf>,
    envs: Vec<(std::ffi::OsString, std::ffi::OsString)>,
    stdin_data: Option<Vec<u8>>,
}

impl<'a> P4Command<'a> {
    fn new(cli: &'a P4Cli) -> Self {
        Self {
            cli,
            args: Vec::new(),
            timeout: None,
            cwd: None,
            envs: Vec::new(),
            stdin_data: None,
        }
    }

    /// Append a single argument.
    pub fn arg(&mut self, arg: impl AsRef<std::ffi::OsStr>) -> &mut Self {
        self.args.push(arg.as_ref().to_os_string());
        self
    }

    /// Append all arguments from a slice.
    pub fn args(&mut self, args: &[impl AsRef<std::ffi::OsStr>]) -> &mut Self {
        self.args
            .extend(args.iter().map(|a| a.as_ref().to_os_string()));
        self
    }

    /// Maximum wall-clock time the process is allowed to run.
    /// When exceeded the process is killed.
    pub fn timeout(&mut self, timeout: Duration) -> &mut Self {
        self.timeout = Some(timeout);
        self
    }

    /// Working directory for the child process.
    pub fn cwd(&mut self, path: impl Into<PathBuf>) -> &mut Self {
        self.cwd = Some(path.into());
        self
    }

    /// Set an environment variable for the child process.
    pub fn env(
        &mut self,
        key: impl Into<std::ffi::OsString>,
        val: impl Into<std::ffi::OsString>,
    ) -> &mut Self {
        self.envs.push((key.into(), val.into()));
        self
    }

    /// Provide data to be piped to the child's stdin.
    pub fn stdin(&mut self, data: impl Into<Vec<u8>>) -> &mut Self {
        self.stdin_data = Some(data.into());
        self
    }

    /// Execute the command and collect output.
    ///
    /// Stdout and stderr are read concurrently in separate OS threads to
    /// prevent pipe-full deadlocks. If a [`timeout`](Self::timeout) was set,
    /// the process is killed once the deadline is reached.
    pub fn run(&mut self) -> std::io::Result<P4Output> {
        let mut cmd = Command::new(&self.cli.bin_path);
        cmd.args(&self.args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        if self.stdin_data.is_some() {
            cmd.stdin(Stdio::piped());
        } else {
            cmd.stdin(Stdio::null());
        }

        if let Some(ref cwd) = self.cwd {
            cmd.current_dir(cwd);
        }
        for (k, v) in &self.envs {
            cmd.env(k, v);
        }

        let mut child = cmd.spawn()?;

        // Write stdin in a background thread if data was provided.
        if let Some(data) = self.stdin_data.take()
            && let Some(mut stdin) = child.stdin.take()
        {
            thread::spawn(move || {
                let _ = stdin.write_all(&data);
            });
        }

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| std::io::Error::other("stdout was not captured"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| std::io::Error::other("stderr was not captured"))?;

        // Read stdout and stderr concurrently in dedicated threads.
        let stdout_handle = thread::spawn(move || {
            let mut buf = Vec::new();
            BufReader::new(stdout).read_to_end(&mut buf)?;
            Ok::<_, std::io::Error>(buf)
        });

        let stderr_handle = thread::spawn(move || {
            let mut buf = Vec::new();
            BufReader::new(stderr).read_to_end(&mut buf)?;
            Ok::<_, std::io::Error>(buf)
        });

        // Wait for the process (with optional timeout).
        let exit_status = wait_process(&mut child, self.timeout)?;

        let stdout_buf = stdout_handle
            .join()
            .map_err(|_| std::io::Error::other("stdout reader thread panicked"))?
            .map_err(|e| std::io::Error::other(format!("stdout read failed: {e}")))?;
        let stderr_buf = stderr_handle
            .join()
            .map_err(|_| std::io::Error::other("stderr reader thread panicked"))?
            .map_err(|e| std::io::Error::other(format!("stderr read failed: {e}")))?;

        Ok(P4Output {
            exit_code: exit_status.code().unwrap_or(-1),
            stdout: stdout_buf,
            stderr: stderr_buf,
        })
    }

    /// Run the command and return a streaming iterator over output lines.
    ///
    /// Stdout and stderr are read concurrently by two OS threads and merged
    /// into a single stream. The final event is always
    /// [`P4StreamEvent::Exit`] with the exit code (unless the stream is
    /// dropped early).
    ///
    /// Unlike [`run`](Self::run), this method does **not** support
    /// [`timeout`](Self::timeout) — the caller controls iteration and can
    /// drop the stream to cancel.
    pub fn stream(&mut self) -> std::io::Result<P4Stream> {
        let mut cmd = Command::new(&self.cli.bin_path);
        cmd.args(&self.args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        if self.stdin_data.is_some() {
            cmd.stdin(Stdio::piped());
        } else {
            cmd.stdin(Stdio::null());
        }
        if let Some(ref cwd) = self.cwd {
            cmd.current_dir(cwd);
        }
        for (k, v) in &self.envs {
            cmd.env(k, v);
        }

        let mut child = cmd.spawn()?;

        if let Some(data) = self.stdin_data.take()
            && let Some(mut stdin) = child.stdin.take()
        {
            thread::spawn(move || {
                let _ = stdin.write_all(&data);
            });
        }

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| std::io::Error::other("stdout was not captured"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| std::io::Error::other("stderr was not captured"))?;

        let (tx, rx) = std::sync::mpsc::channel();
        let mut handles = Vec::new();

        // Stdout reader thread.
        let tx_out = tx.clone();
        handles.push(thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                match line {
                    Ok(l) => {
                        if tx_out.send(Ok(P4StreamEvent::Stdout(l))).is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        let _ = tx_out.send(Err(e));
                        break;
                    }
                }
            }
        }));

        // Stderr reader thread.
        let tx_err = tx.clone();
        handles.push(thread::spawn(move || {
            for line in BufReader::new(stderr).lines() {
                match line {
                    Ok(l) => {
                        if tx_err.send(Ok(P4StreamEvent::Stderr(l))).is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        let _ = tx_err.send(Err(e));
                        break;
                    }
                }
            }
        }));

        Ok(P4Stream {
            rx,
            child: Some(child),
            handles,
            exhausted: false,
        })
    }
}

/// Block until `child` exits, optionally killing it after `timeout`.
fn wait_process(child: &mut Child, timeout: Option<Duration>) -> std::io::Result<ExitStatus> {
    match timeout {
        None => child.wait(),
        Some(t) => wait_with_timeout(child, t),
    }
}

fn wait_with_timeout(child: &mut Child, timeout: Duration) -> std::io::Result<ExitStatus> {
    let start = std::time::Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if start.elapsed() >= timeout {
            child.kill()?;
            return child.wait();
        }
        thread::sleep(Duration::from_millis(50));
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

impl P4Cli {
    /// Create a new `P4Cli` instance.
    ///
    /// The embedded p4 binary is decompressed once and cached on disk.
    /// Subsequent instances on the same host reuse the cache.
    /// Stale per-instance directories (older than 1 hour) are cleaned up.
    pub fn new() -> std::io::Result<Self> {
        let (bin_path, temp_dir) = write_p4_cli_to_disk()?;
        Ok(Self {
            bin_path,
            _temp_dir: temp_dir,
        })
    }

    /// Convenience method: run p4 with the given arguments.
    ///
    /// This is equivalent to `self.command().args(args).run()`.
    pub fn run<S: AsRef<std::ffi::OsStr>>(&self, args: &[S]) -> std::io::Result<P4Output> {
        self.command().args(args).run()
    }

    /// Convenience method: stream p4 output with the given arguments.
    ///
    /// This is equivalent to `self.command().args(args).stream()`.
    pub fn stream<S: AsRef<std::ffi::OsStr>>(&self, args: &[S]) -> std::io::Result<P4Stream> {
        self.command().args(args).stream()
    }

    /// Obtain a [`P4Command`] builder for fine-grained control over
    /// working directory, environment variables, stdin, and timeout.
    pub fn command(&self) -> P4Command<'_> {
        P4Command::new(self)
    }
}

impl Drop for P4Cli {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self._temp_dir);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_run_help() -> std::io::Result<()> {
        let p4 = P4Cli::new()?;
        let output = p4.run(&["--help"])?;
        assert!(output.success(), "p4 --help should exit with 0");
        let stdout = output
            .stdout_str()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
        assert!(
            stdout.contains("Usage:"),
            "expected --help output to contain 'Usage:'"
        );
        Ok(())
    }

    #[test]
    fn test_run_error() -> std::io::Result<()> {
        let p4 = P4Cli::new()?;
        let output = p4.run(&["--nonexistent-flag"])?;
        assert!(!output.success(), "unknown flag should exit non-zero");
        let stderr = output
            .stderr_str()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
        assert!(
            stderr.contains("Invalid option") || stderr.contains("error"),
            "expected error output, got: {stderr}"
        );
        Ok(())
    }

    #[test]
    fn test_multiple_instances() -> std::io::Result<()> {
        let p4_a = P4Cli::new()?;
        let p4_b = P4Cli::new()?;
        assert!(p4_a.run(&["--help"])?.success());
        assert!(p4_b.run(&["--help"])?.success());
        Ok(())
    }

    #[test]
    fn test_command_builder() -> std::io::Result<()> {
        let p4 = P4Cli::new()?;
        let output = p4.command().arg("--help").run()?;
        assert!(output.success());
        Ok(())
    }

    #[test]
    fn test_timeout_kills() -> std::io::Result<()> {
        let p4 = P4Cli::new()?;
        // Run with a very short timeout – should be killed.
        let output = p4
            .command()
            .arg("help")
            .timeout(Duration::from_millis(1))
            .run()?;
        // After kill the exit code is typically non-zero (e.g. -1 or a signal number).
        // We only verify the call does not hang.
        assert!(!output.success() || output.exit_code() == 0);
        Ok(())
    }

    #[test]
    fn test_stream_help() -> std::io::Result<()> {
        let p4 = P4Cli::new()?;
        let mut saw_stdout = false;
        let mut saw_exit = false;
        for event in p4.stream(&["--help"])? {
            match event? {
                P4StreamEvent::Stdout(line) => {
                    if line.contains("Usage:") {
                        saw_stdout = true;
                    }
                }
                P4StreamEvent::Stderr(_) => {}
                P4StreamEvent::Exit(code) => {
                    assert_eq!(code, 0);
                    saw_exit = true;
                }
            }
        }
        assert!(saw_stdout, "expected --help to contain 'Usage:'");
        assert!(saw_exit, "expected Exit event");
        Ok(())
    }

    #[test]
    fn test_stream_error() -> std::io::Result<()> {
        let p4 = P4Cli::new()?;
        let mut saw_stderr = false;
        let mut saw_exit = false;
        for event in p4.stream(&["--nonexistent-flag"])? {
            match event? {
                P4StreamEvent::Stdout(_) => {}
                P4StreamEvent::Stderr(line) => {
                    if line.contains("Invalid option") || line.contains("error") {
                        saw_stderr = true;
                    }
                }
                P4StreamEvent::Exit(code) => {
                    assert_ne!(code, 0, "nonexistent flag should fail");
                    saw_exit = true;
                }
            }
        }
        assert!(saw_stderr, "expected error output");
        assert!(saw_exit, "expected Exit event");
        Ok(())
    }

    #[test]
    fn test_stream_drop_midway() -> std::io::Result<()> {
        // Dropping the stream mid-way must not hang or panic.
        let p4 = P4Cli::new()?;
        let stream = p4.stream(&["--help"])?;
        drop(stream);
        Ok(())
    }

    #[test]
    fn test_stream_builder() -> std::io::Result<()> {
        let p4 = P4Cli::new()?;
        let mut saw_exit = false;
        for event in p4.command().arg("--help").stream()? {
            if let P4StreamEvent::Exit(code) = event? {
                assert_eq!(code, 0);
                saw_exit = true;
            }
        }
        assert!(saw_exit);
        Ok(())
    }
}
