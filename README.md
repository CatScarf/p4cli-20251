# P4CLI-20251

A simple Rust library for full-featured P4 (Perforce) CLI with zero pre-install. All errors are propagated via `Result` without any `unwrap`.

## Features

- **Zero pre-install** — the p4 binary is embedded as a zstd-compressed asset and extracted at runtime.
- **Secure** — each instance uses a random-isolated temp directory; binary is written atomically (`write + rename`); Unix permissions are `0o700`.
- **Concurrent-safe** — per-instance temp directories eliminate cross-process file clashes; a content-addressed disk cache avoids repeated decompression.
- **Clean exit** — child process is always reaped (`wait()`), no zombie left behind.
- **Binary-safe output** — raw `Vec<u8>` preserves any binary content; UTF-8 helpers provided for text.
- **Timeout & control** — `P4Command` builder provides `timeout`, `cwd`, `env`, and `stdin` control.
- **Platform-conditional dependencies** — only the current platform's binary crate is downloaded (~4–5 MB instead of ~23 MB).

## Usage

```toml
[dependencies]
p4cli-20251 = "0.5.0"
```

### Quick start

```rust
use p4cli_20251::P4Cli;

fn main() -> std::io::Result<()> {
    let p4 = P4Cli::new()?;
    let output = p4.run(&["--help"])?;
    println!("exit: {}", output.exit_code());
    println!("{}", output.stdout_str()?);
    Ok(())
}
```

### Builder API (timeout, cwd, env, stdin)

```rust
use p4cli_20251::P4Cli;
use std::time::Duration;

fn main() -> std::io::Result<()> {
    let p4 = P4Cli::new()?;

    let output = p4
        .command()
        .arg("sync")
        .args(&["-f", "//depot/..."])
        .cwd("/workspace")
        .env("P4PORT", "ssl:perforce:1666")
        .timeout(Duration::from_secs(60))
        .run()?;

    println!("synced, exit code: {}", output.exit_code());
    Ok(())
}
```

### P4Output inspection

```rust
let out = p4.run(&["info"])?;

// Raw bytes (binary-safe)
let stdout: &[u8] = out.stdout();
let stderr: &[u8] = out.stderr();

// UTF-8 text helpers
if let Ok(text) = out.stdout_str() {
    for line in text.lines() {
        println!("{line}");
    }
}

// Exit-code check
if out.success() {
    println!("command succeeded");
} else {
    eprintln!("failed with exit code {}", out.exit_code());
}
```

## Supported Platforms

| Target | Binary crate |
|--------|-------------|
| Windows x86\_64 | p4cli-20251-win-x64 |
| macOS ARM64 | p4cli-20251-mac-arm64 |
| macOS x86\_64 | p4cli-20251-mac-x64 |
| Linux x86\_64 | p4cli-20251-linux-x64 |
| Linux ARM64 | p4cli-20251-linux-arm64 |

Only the platform-specific binary crate is downloaded as a dependency — no unnecessary bloat.
