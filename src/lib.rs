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
    pub fn new() -> std::io::Result<Self> {
        let path = Self::write_p4_cli_to_disk()?;
        Ok(Self { bin_path: path })
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
        compile_error!(format!(
            "Unsupported platform: {}-{}",
            std::env::consts::OS,
            std::env::consts::ARCH
        ));
    }

    fn get_p4_cli_binary() -> std::io::Result<Vec<u8>> {
        let zst_data = Self::get_p4_cli_zst();
        let mut decoder = zstd::stream::Decoder::new(&zst_data[..])?;
        let mut decompressed_data = Vec::new();
        std::io::copy(&mut decoder, &mut decompressed_data)?;
        Ok(decompressed_data)
    }

    fn write_p4_cli_to_disk() -> std::io::Result<String> {
        let binary_data = Self::get_p4_cli_binary()?;
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

        Ok(file_path
            .to_str()
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, "path is not valid UTF-8")
            })?
            .to_string())
    }

    pub fn run<'a, I, S>(
        &'a self,
        args: I,
    ) -> std::io::Result<impl Iterator<Item = std::io::Result<P4CliResult>> + 'a>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::os_str::OsStr>,
    {
        let process = Command::new(&self.bin_path)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        let stdout = process
            .stdout
            .ok_or_else(|| std::io::Error::other("stdout was not captured"))?;
        let stderr = process
            .stderr
            .ok_or_else(|| std::io::Error::other("stderr was not captured"))?;

        let stdout_reader = std::io::BufReader::new(stdout).lines();
        let stderr_reader = std::io::BufReader::new(stderr).lines();

        let stdout_lines = Arc::new(Mutex::new(stdout_reader));
        let stderr_lines = Arc::new(Mutex::new(stderr_reader));

        Ok(std::iter::from_fn(move || {
            (|| -> std::io::Result<Option<P4CliResult>> {
                let mut stdout_lines = stdout_lines
                    .lock()
                    .map_err(|e| std::io::Error::other(e.to_string()))?;
                let mut stderr_lines = stderr_lines
                    .lock()
                    .map_err(|e| std::io::Error::other(e.to_string()))?;

                if let Some(line) = stdout_lines.next().transpose()? {
                    return Ok(Some(P4CliResult::Out(line)));
                }

                if let Some(line) = stderr_lines.next().transpose()? {
                    return Ok(Some(P4CliResult::Err(line)));
                }

                Ok(None)
            })()
            .transpose()
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_run() -> std::io::Result<()> {
        let p4 = P4Cli::new()?;
        let results: Vec<_> = p4
            .run(vec!["--help".to_string()])?
            .collect::<std::io::Result<Vec<_>>>()?;

        assert!(!results.is_empty());

        if let Some(P4CliResult::Out(line)) = results.first() {
            assert!(line.contains("Usage:"));
        } else {
            panic!("Expected the first line to be an output with usage information.");
        }

        Ok(())
    }
}
