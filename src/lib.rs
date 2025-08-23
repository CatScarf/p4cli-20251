use std::io::{BufRead, Write};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

pub struct P4Cli {
    bin_path: String,
}

#[derive(Debug)]
pub enum P4CliResult {
    Out(String),
    Err(String),
}

impl std::fmt::Display for P4CliResult {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            P4CliResult::Out(line) => write!(f, "{}", line),
            P4CliResult::Err(line) => write!(f, "{}", line),
        }
    }
}

impl P4Cli {
    pub fn new() -> Self {
        let path = Self::write_p4_cli_to_disk().unwrap();
        Self { bin_path: path }
    }

    fn get_p4_cli_zst() -> Vec<u8> {
        #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
        {
            use p4cli_20251_win_x64::get_p4_cli_zst;
            return get_p4_cli_zst();
        }

        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            use p4cli_20251_mac_arm64::get_p4_cli_zst;
            return get_p4_cli_zst();
        }

        #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
        {
            use p4cli_20251_mac_x64::get_p4_cli_zst;
            return get_p4_cli_zst();
        }

        #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
        {
            use p4cli_20251_linux_x64::get_p4_cli_zst;
            return get_p4_cli_zst();
        }

        #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
        {
            use p4cli_20251_linux_arm64::get_p4_cli_zst;
            return get_p4_cli_zst();
        }

        #[cfg(not(any(
            all(target_os = "windows", target_arch = "x86_64"),
            all(target_os = "macos", target_arch = "aarch64"),
            all(target_os = "macos", target_arch = "x86_64"),
            all(target_os = "linux", target_arch = "x86_64"),
            all(target_os = "linux", target_arch = "aarch64")
        )))]
        compile_error!(format!(
            "Unsupported platform: {}-{}",
            std::env::consts::OS,
            std::env::consts::ARCH
        ));
    }

    fn get_p4_cli_binary() -> Vec<u8> {
        let zst_data = Self::get_p4_cli_zst();
        let mut decoder = zstd::stream::Decoder::new(&zst_data[..]).unwrap();
        let mut decompressed_data = Vec::new();
        std::io::copy(&mut decoder, &mut decompressed_data).unwrap();
        decompressed_data
    }

    fn write_p4_cli_to_disk() -> std::io::Result<String> {
        let binary_data = Self::get_p4_cli_binary();
        let temp_dir = std::env::temp_dir();
        let file_path = temp_dir.join("p4_binary");

        let mut file = std::fs::File::create(&file_path)?;
        file.write_all(&binary_data)?;
        file.sync_all()?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&file_path, std::fs::Permissions::from_mode(0o755))?;
        }

        Ok(file_path.to_str().unwrap().to_string())
    }

    pub fn run<'a, I, S>(&'a self, args: I) -> impl Iterator<Item = P4CliResult> + 'a
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::os_str::OsStr>,
    {
        let process = Command::new(&self.bin_path)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();

        let stdout = process.stdout.unwrap();
        let stderr = process.stderr.unwrap();

        let stdout_reader = std::io::BufReader::new(stdout).lines();
        let stderr_reader = std::io::BufReader::new(stderr).lines();

        let stdout_lines = Arc::new(Mutex::new(stdout_reader));
        let stderr_lines = Arc::new(Mutex::new(stderr_reader));

        std::iter::from_fn(move || {
            let mut stdout_lines = stdout_lines.lock().unwrap();
            let mut stderr_lines = stderr_lines.lock().unwrap();

            if let Some(line) = stdout_lines.next().transpose().unwrap() {
                return Some(P4CliResult::Out(line));
            }

            if let Some(line) = stderr_lines.next().transpose().unwrap() {
                return Some(P4CliResult::Err(line));
            }

            None
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_run() {
        let p4 = P4Cli::new();
        let results: Vec<_> = p4.run(vec!["--help".to_string()]).collect();

        assert!(!results.is_empty());

        if let Some(P4CliResult::Out(line)) = results.get(0) {
            assert!(line.contains("Usage:"));
        } else {
            panic!("Expected the first line to be an output with usage information.");
        }
    }
}
