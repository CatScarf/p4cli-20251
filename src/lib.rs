use std::io::{BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::Duration;

/// Per-instance temp dir via `tempfile`, cleaned up on Drop.
///
/// ```rust
/// use p4cli_20251::P4Cli;
/// fn main() -> std::io::Result<()> {
///     let p4: P4Cli = P4Cli::new()?;
///     let output: p4cli_20251::P4Output = p4.run(&["--help"])?;
///     println!("exit: {}", output.exit_code());
///     println!("stdout: {}", output.stdout_str()?);
///     Ok(())
/// }
/// ```
pub struct P4Cli {
    bin_path: PathBuf,
    _temp_dir: tempfile::TempDir,
}

/// Raw stdout/stderr bytes, exit code and timeout flag from a p4 invocation.
///
/// For large outputs (e.g. `p4 sync`, `p4 fstat`), use [`P4Cli::stream`]
/// instead of [`P4Cli::run`] to avoid buffering all output in memory.
pub struct P4Output {
    exit_code: i32,
    timed_out: bool,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

impl P4Output {
    pub fn exit_code(&self) -> i32 {
        self.exit_code
    }

    /// Returns `true` when the process was killed by a timeout.
    pub fn timed_out(&self) -> bool {
        self.timed_out
    }

    /// Returns `true` when the process exited with code 0 and was not timed out.
    pub fn success(&self) -> bool {
        !self.timed_out && self.exit_code == 0
    }

    pub fn stdout(&self) -> &[u8] {
        &self.stdout
    }

    pub fn stderr(&self) -> &[u8] {
        &self.stderr
    }

    pub fn stdout_str(&self) -> std::io::Result<&str> {
        std::str::from_utf8(&self.stdout).map_err(std::io::Error::other)
    }

    pub fn stderr_str(&self) -> std::io::Result<&str> {
        std::str::from_utf8(&self.stderr).map_err(std::io::Error::other)
    }

    pub fn stdout_lines(&self) -> std::io::Result<Vec<&str>> {
        let s = self.stdout_str()?;
        if s.is_empty() {
            Ok(Vec::new())
        } else {
            Ok(s.lines().collect())
        }
    }

    pub fn stderr_lines(&self) -> std::io::Result<Vec<&str>> {
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
            .field("timed_out", &self.timed_out)
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
    Stdout(Vec<u8>),
    Stderr(Vec<u8>),
    Exit(i32),
}

impl P4StreamEvent {
    /// Try to decode this event's payload as UTF-8.
    pub fn as_utf8(&self) -> Option<&str> {
        match self {
            P4StreamEvent::Stdout(data) | P4StreamEvent::Stderr(data) => {
                std::str::from_utf8(data).ok()
            }
            P4StreamEvent::Exit(_) => None,
        }
    }
}

impl std::fmt::Display for P4StreamEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            P4StreamEvent::Stdout(data) | P4StreamEvent::Stderr(data) => {
                if let Ok(text) = std::str::from_utf8(data) {
                    write!(f, "{text}")
                } else {
                    write!(f, "<{} bytes>", data.len())
                }
            }
            P4StreamEvent::Exit(code) => write!(f, "(exit {code})"),
        }
    }
}

/// Merged stdout/stderr byte chunks (~64 KB each) as a single iterator.
///
/// Uses a bounded channel (64 slots) for backpressure. The final item is
/// always [`P4StreamEvent::Exit`]. Drop mid-way to kill.
///
/// ```rust
/// use p4cli_20251::{P4Cli, P4StreamEvent};
/// fn main() -> std::io::Result<()> {
///     let p4: P4Cli = P4Cli::new()?;
///     for event in p4.stream(&["--help"])? {
///         match event? {
///             P4StreamEvent::Stdout(chunk) => {
///                 if let Ok(text) = std::str::from_utf8(&chunk) {
///                     print!("{text}");
///                 }
///             }
///             P4StreamEvent::Stderr(chunk) => {
///                 if let Ok(text) = std::str::from_utf8(&chunk) {
///                     eprint!("{text}");
///                 }
///             }
///             P4StreamEvent::Exit(code) => println!("exit {code}"),
///         }
///     }
///     Ok(())
/// }
/// ```
pub struct P4Stream {
    rx: std::sync::mpsc::Receiver<std::io::Result<P4StreamEvent>>,
    child: Option<Child>,
    exhausted: bool,
}

impl std::fmt::Debug for P4Stream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("P4Stream")
            .field("exhausted", &self.exhausted)
            .finish()
    }
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
                self.exhausted = true;
                // Wait for the child to be reaped.
                match self.child.take() {
                    Some(mut c) => match c.wait() {
                        Ok(status) => Some(Ok(P4StreamEvent::Exit(status.code().unwrap_or(-1)))),
                        Err(e) => Some(Err(e)),
                    },
                    None => None,
                }
            }
        }
    }
}

impl Drop for P4Stream {
    fn drop(&mut self) {
        // Drop rx so sender threads break out of send().
        // The handles field is consumed by join, but dropping JoinHandle
        // simply detaches the thread — the threads exit on their own once
        // the child is killed and pipes close.
        if let Some(ref mut child) = self.child {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

// ---------------------------------------------------------------------------
// Binary extraction
// ---------------------------------------------------------------------------

fn write_p4_cli_to_disk() -> std::io::Result<(PathBuf, tempfile::TempDir)> {
    let zst_data = get_p4_cli_zst();
    let binary_data = decompress_zst(&zst_data)?;

    let temp_dir = create_temp_dir()?;
    let bin_path = temp_dir.path().join("p4_binary");
    let tmp_path = temp_dir.path().join(".tmp");

    {
        let mut file = std::fs::File::create(&tmp_path)?;
        file.write_all(&binary_data)?;
        file.sync_all()?;
    }
    std::fs::rename(&tmp_path, &bin_path)?;
    set_executable_perms(&bin_path)?;

    Ok((bin_path, temp_dir))
}

fn create_temp_dir() -> std::io::Result<tempfile::TempDir> {
    let mut builder = tempfile::Builder::new();
    builder.prefix("p4cli-20251");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    builder.tempdir()
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

    #[cfg(all(
        target_os = "linux",
        target_arch = "x86_64",
        any(target_env = "gnu", target_env = "musl")
    ))]
    {
        use p4cli_20251_linux_x64::get_p4_cli_zst;
        get_p4_cli_zst()
    }

    #[cfg(all(
        target_os = "linux",
        target_arch = "aarch64",
        any(target_env = "gnu", target_env = "musl")
    ))]
    {
        use p4cli_20251_linux_arm64::get_p4_cli_zst;
        get_p4_cli_zst()
    }

    #[cfg(not(any(
        all(target_os = "windows", target_arch = "x86_64"),
        all(target_os = "macos", target_arch = "aarch64"),
        all(target_os = "macos", target_arch = "x86_64"),
        all(
            target_os = "linux",
            target_arch = "x86_64",
            any(target_env = "gnu", target_env = "musl")
        ),
        all(
            target_os = "linux",
            target_arch = "aarch64",
            any(target_env = "gnu", target_env = "musl")
        ),
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
// P4Command builder
// ---------------------------------------------------------------------------

/// Builder for a single p4 invocation (timeout, cwd, env, stdin).
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

    pub fn arg(&mut self, arg: impl AsRef<std::ffi::OsStr>) -> &mut Self {
        self.args.push(arg.as_ref().to_os_string());
        self
    }

    pub fn args(&mut self, args: &[impl AsRef<std::ffi::OsStr>]) -> &mut Self {
        self.args
            .extend(args.iter().map(|a| a.as_ref().to_os_string()));
        self
    }

    /// Maximum wall-clock time. Kills the direct child on timeout (not process tree).
    ///
    /// When the process is killed by timeout, [`P4Output::timed_out`] returns `true`.
    /// Not supported with [`stream()`](Self::stream) — use [`run()`](Self::run) instead.
    pub fn timeout(&mut self, timeout: Duration) -> &mut Self {
        self.timeout = Some(timeout);
        self
    }

    pub fn cwd(&mut self, path: impl Into<PathBuf>) -> &mut Self {
        self.cwd = Some(path.into());
        self
    }

    pub fn env(
        &mut self,
        key: impl Into<std::ffi::OsString>,
        val: impl Into<std::ffi::OsString>,
    ) -> &mut Self {
        self.envs.push((key.into(), val.into()));
        self
    }

    pub fn stdin(&mut self, data: impl Into<Vec<u8>>) -> &mut Self {
        self.stdin_data = Some(data.into());
        self
    }

    /// Block until the process exits, returning collected output.
    ///
    /// For large output, consider [`stream()`](Self::stream) instead to
    /// avoid buffering everything in memory.
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

        let stdin_handle = self.stdin_data.take().and_then(|data| {
            child.stdin.take().map(|mut stdin| {
                thread::spawn(move || {
                    if let Err(e) = stdin.write_all(&data) {
                        Err(e)
                    } else {
                        Ok(())
                    }
                })
            })
        });

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| std::io::Error::other("stdout was not captured"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| std::io::Error::other("stderr was not captured"))?;

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

        let (exit_status, timed_out) = wait_process(&mut child, self.timeout)?;

        // Surface stdin write errors.
        if let Some(handle) = stdin_handle {
            handle
                .join()
                .map_err(|_| std::io::Error::other("stdin thread panicked"))?
                .map_err(|e| std::io::Error::other(format!("stdin write failed: {e}")))?;
        }

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
            timed_out,
            stdout: stdout_buf,
            stderr: stderr_buf,
        })
    }

    /// Streaming iterator over stdout/stderr byte chunks.
    ///
    /// Uses a bounded channel (64 slots) for backpressure. The final event
    /// is [`P4StreamEvent::Exit`]. Drop mid-way to cancel.
    ///
    /// **Note**: [`timeout`](Self::timeout) is not supported — use
    /// [`run()`](Self::run) instead.
    pub fn stream(&mut self) -> std::io::Result<P4Stream> {
        if self.timeout.is_some() {
            return Err(std::io::Error::other(
                "timeout is not supported on stream(); use run() instead",
            ));
        }

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

        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| std::io::Error::other("stdout was not captured"))?;
        let mut stderr = child
            .stderr
            .take()
            .ok_or_else(|| std::io::Error::other("stderr was not captured"))?;

        // Bounded channel for backpressure (64 slots).
        let (tx, rx) = std::sync::mpsc::sync_channel(64);

        let tx_out = tx.clone();
        thread::spawn(move || {
            let mut buf = vec![0u8; 65536];
            loop {
                let n = match stdout.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(e) => {
                        let _ = tx_out.send(Err(e));
                        break;
                    }
                };
                if tx_out
                    .send(Ok(P4StreamEvent::Stdout(buf[..n].to_vec())))
                    .is_err()
                {
                    break;
                }
            }
        });

        let tx_err = tx.clone();
        thread::spawn(move || {
            let mut buf = vec![0u8; 65536];
            loop {
                let n = match stderr.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(e) => {
                        let _ = tx_err.send(Err(e));
                        break;
                    }
                };
                if tx_err
                    .send(Ok(P4StreamEvent::Stderr(buf[..n].to_vec())))
                    .is_err()
                {
                    break;
                }
            }
        });

        Ok(P4Stream {
            rx,
            child: Some(child),
            exhausted: false,
        })
    }
}

// ---------------------------------------------------------------------------
// Process helpers
// ---------------------------------------------------------------------------

fn wait_process(
    child: &mut Child,
    timeout: Option<Duration>,
) -> std::io::Result<(ExitStatus, bool)> {
    match timeout {
        None => Ok((child.wait()?, false)),
        Some(t) => wait_with_timeout(child, t),
    }
}

fn wait_with_timeout(child: &mut Child, timeout: Duration) -> std::io::Result<(ExitStatus, bool)> {
    let start = std::time::Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok((status, false));
        }
        if start.elapsed() >= timeout {
            child.kill()?;
            return Ok((child.wait()?, true));
        }
        thread::sleep(Duration::from_millis(50));
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

impl P4Cli {
    /// Decompress embedded p4 binary to a `tempfile`-managed temp directory.
    pub fn new() -> std::io::Result<Self> {
        let (bin_path, temp_dir) = write_p4_cli_to_disk()?;
        Ok(Self {
            bin_path,
            _temp_dir: temp_dir,
        })
    }

    /// Equivalent to `self.command().args(args).run()`.
    pub fn run<S: AsRef<std::ffi::OsStr>>(&self, args: &[S]) -> std::io::Result<P4Output> {
        self.command().args(args).run()
    }

    /// Equivalent to `self.command().args(args).stream()`.
    pub fn stream<S: AsRef<std::ffi::OsStr>>(&self, args: &[S]) -> std::io::Result<P4Stream> {
        self.command().args(args).stream()
    }

    /// Obtain a [`P4Command`] builder.
    ///
    /// ```rust
    /// use p4cli_20251::P4Cli;
    /// use std::time::Duration;
    /// fn main() -> std::io::Result<()> {
    ///     let p4: P4Cli = P4Cli::new()?;
    ///     let output: p4cli_20251::P4Output = p4
    ///         .command()
    ///         .arg("--help")
    ///         .timeout(Duration::from_secs(10))
    ///         .run()?;
    ///     if output.success() {
    ///         println!("{}", output.stdout_str()?);
    ///     }
    ///     Ok(())
    /// }
    /// ```
    pub fn command(&self) -> P4Command<'_> {
        P4Command::new(self)
    }
}
