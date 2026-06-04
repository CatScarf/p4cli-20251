use std::path::PathBuf;

use crate::platform;

/// Try to locate a working `p4` binary on the system.
///
/// Checks `PATH` first, then common install directories.
/// Returns `None` if no working binary is found.
pub fn find_system_p4() -> Option<PathBuf> {
    // 1. Check PATH.
    if let Some(path) = search_path()
        && is_working_p4(&path)
    {
        return Some(path);
    }

    // 2. Check default install directories.
    for dir in platform::default_install_dirs() {
        let path = PathBuf::from(dir);
        if path.exists() && is_working_p4(&path) {
            return Some(path);
        }
    }

    None
}

/// Search PATH for the p4 binary.
fn search_path() -> Option<PathBuf> {
    let name = platform::binary_name();
    std::env::var_os("PATH").and_then(|paths| {
        for dir in std::env::split_paths(&paths) {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        None
    })
}

/// Quick check: run `p4` and verify it responds.
fn is_working_p4(path: &PathBuf) -> bool {
    std::process::Command::new(path)
        .arg("-h")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_system_p4_at_least_checks() {
        // This test verifies the function runs without error.
        // On CI with p4 installed, it returns Some; otherwise None.
        let result = find_system_p4();
        // We don't assert on the result — just ensure no panic/crash.
        let _ = result;
    }

    #[test]
    fn test_is_working_p4_with_bogus_path() {
        let bogus = PathBuf::from("/nonexistent/p4");
        assert!(!is_working_p4(&bogus));
    }
}
