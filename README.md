# P4CLI-20251

A simple Rust library for full-featured P4 (Perforce) CLI with zero pre-install. All errors are propagated via `Result` without any `unwrap`.

The `p4` binary is **not** bundled. On first use, it locates a system installation or downloads it from Perforce's official filehost.

```toml
[dependencies]
p4cli-20251 = "0.6.0"
```

## Synchronous API

Block until the process exits, then inspect the collected output.

```rust
use p4cli_20251::P4Cli;

fn main() -> std::io::Result<()> {
    let p4: P4Cli = P4Cli::new()?;
    let output: p4cli_20251::P4Output = p4.run(&["--help"])?;

    println!("exit: {}", output.exit_code());
    println!("stdout: {}", output.stdout_str()?);
    Ok(())
}
```

With timeout detection:

```rust
use p4cli_20251::P4Cli;
use std::time::Duration;

fn main() -> std::io::Result<()> {
    let p4: P4Cli = P4Cli::new()?;
    let output: p4cli_20251::P4Output = p4
        .command()
        .arg("sync")
        .timeout(Duration::from_secs(30))
        .run()?;

    if output.timed_out() {
        eprintln!("p4 timed out");
    } else if output.success() {
        println!("{}", output.stdout_str()?);
    } else {
        eprintln!("p4 failed (exit {})", output.exit_code());
    }
    Ok(())
}
```

## Streaming API

Iterate over raw byte chunks as they arrive. The stream ends with `P4StreamEvent::Exit`.

```rust
use p4cli_20251::{P4Cli, P4StreamEvent};

fn main() -> std::io::Result<()> {
    let p4: P4Cli = P4Cli::new()?;

    for event in p4.stream(&["info"])? {
        match event? {
            P4StreamEvent::Stdout(chunk) => {
                if let Ok(text) = std::str::from_utf8(&chunk) {
                    print!("{text}");
                }
            }
            P4StreamEvent::Stderr(chunk) => {
                if let Ok(text) = std::str::from_utf8(&chunk) {
                    eprint!("{text}");
                }
            }
            P4StreamEvent::Exit(code) => println!("exit {code}"),
        }
    }
    Ok(())
}
```

Drop the stream mid-way to cancel the process.

## Supported Platforms

| Target | Download |
|--------|----------|
| Windows x86\_64 | `bin.ntx64/p4.exe` |
| macOS ARM64 | `bin.macosx12arm64/p4` |
| macOS x86\_64 | `bin.macosx1015x86_64/p4` |
| Linux x86\_64 (glibc/musl) | `bin.linux26x86_64/p4` |
| Linux ARM64 (glibc/musl) | `bin.linux26aarch64/p4` |

## License

This project is licensed under the MIT License — see the [LICENSE](LICENSE) file for details.

The Perforce (`p4`) binary is proprietary software owned by Perforce Software, Inc.
Use is subject to Perforce's [Master Terms & Conditions](https://www.perforce.com/legal).
This crate does **not** bundle the binary — it either uses a system-installed copy or
downloads it directly from Perforce's official filehost.
