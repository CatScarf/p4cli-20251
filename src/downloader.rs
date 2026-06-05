use std::path::{Path, PathBuf};

use crate::platform;

/// Download p4 from the official Perforce filehost and cache it locally.
pub fn download_p4() -> std::io::Result<(PathBuf, tempfile::TempDir)> {
    let url =
        platform::download_url().ok_or_else(|| std::io::Error::other("unsupported platform"))?;

    let cache_dir = tempfile::Builder::new()
        .prefix("p4cli-20251-download")
        .tempdir()?;
    let bin_path = cache_dir.path().join(platform::binary_name());

    download_to(&url, &bin_path)?;
    set_perms(&bin_path)?;

    Ok((bin_path, cache_dir))
}

/// Download from `url` and write to `dest`.
fn download_to(url: &str, dest: &Path) -> std::io::Result<()> {
    let response = ureq::get(url)
        .call()
        .map_err(|e| std::io::Error::other(format!("download failed: {e}")))?;

    let status = response.status();
    if status != 200 {
        return Err(std::io::Error::other(format!(
            "download returned HTTP {status} for {url}"
        )));
    }

    let mut body = response.into_body();
    let data = body
        .read_to_vec()
        .map_err(|e| std::io::Error::other(format!("read body: {e}")))?;
    std::fs::write(dest, &data)?;

    Ok(())
}

#[cfg(unix)]
fn set_perms(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
}

#[cfg(not(unix))]
fn set_perms(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_download_url_is_some_on_supported_platforms() {
        let url = platform::download_url();
        // On supported platforms, URL must be present.
        // We just check it's not None (actual download tested in integration).
        #[cfg(any(
            all(target_os = "windows", target_arch = "x86_64"),
            all(target_os = "macos", target_arch = "aarch64"),
            all(target_os = "macos", target_arch = "x86_64"),
            all(target_os = "linux", target_arch = "x86_64"),
            all(target_os = "linux", target_arch = "aarch64"),
        ))]
        assert!(
            url.is_some(),
            "download_url() should return Some on this platform"
        );
    }

    #[test]
    fn test_download_bogus_url_fails_gracefully() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dest = tmp.path().join("p4");
        // Use a non-routable IP to avoid relying on external servers.
        let result = download_to("http://10.255.255.1/nonexistent", &dest);
        assert!(result.is_err(), "bogus URL should fail");
    }

    #[test]
    #[ignore = "requires network access to Perforce filehost"]
    fn test_download_p4_from_perforce() {
        let url = platform::download_url().expect("unsupported platform");
        let tmp = tempfile::TempDir::new().unwrap();
        let dest = tmp.path().join(platform::binary_name());

        let result = download_to(&url, &dest);
        assert!(result.is_ok(), "download from {url} failed: {result:?}");
        assert!(dest.exists(), "downloaded file should exist");
        assert!(
            dest.metadata().unwrap().len() > 1000,
            "downloaded file too small"
        );
    }
}
