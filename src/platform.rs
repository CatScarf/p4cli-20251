/// Perforce download URL for the current platform, or None if unsupported.
pub fn download_url() -> Option<String> {
    // r25.2 is the latest stable release as of mid-2025.
    let base = "https://filehost.perforce.com/perforce/r25.2";
    let path = platform_path()?;
    Some(format!("{base}/{path}"))
}

/// Relative download path on Perforce filehost.
fn platform_path() -> Option<&'static str> {
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        Some("bin.ntx64/p4.exe")
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        Some("bin.mac20arm64/p4")
    }
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        Some("bin.macosx1015x86_64/p4")
    }
    #[cfg(all(
        target_os = "linux",
        target_arch = "x86_64",
        any(target_env = "gnu", target_env = "musl")
    ))]
    {
        Some("bin.linux26x86_64/p4")
    }
    #[cfg(all(
        target_os = "linux",
        target_arch = "aarch64",
        any(target_env = "gnu", target_env = "musl")
    ))]
    {
        Some("bin.linux26aarch64/p4")
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
        None
    }
}

/// Binary file name (`p4` or `p4.exe`).
pub fn binary_name() -> &'static str {
    #[cfg(windows)]
    {
        "p4.exe"
    }
    #[cfg(not(windows))]
    {
        "p4"
    }
}

/// Default install directories to check for system p4.
pub fn default_install_dirs() -> &'static [&'static str] {
    #[cfg(windows)]
    {
        &[
            r"C:\Program Files\Perforce\p4.exe",
            r"C:\Program Files (x86)\Perforce\p4.exe",
        ]
    }
    #[cfg(target_os = "macos")]
    {
        &["/Applications/Perforce/p4", "/usr/local/bin/p4"]
    }
    #[cfg(target_os = "linux")]
    {
        &["/usr/local/bin/p4", "/usr/bin/p4", "/opt/perforce/bin/p4"]
    }
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    {
        &[]
    }
}
