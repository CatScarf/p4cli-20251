# P4CLI-20251

A simple Rust library for full-featured P4 (Perforce) CLI with zero pre-install, already battle-tested and mature in production environments, where all errors are propagated via Result without any unwrap.

```toml
[dependencies]
p4cli-20251 = "0.5.1"
```

## Synchronous API

Block until the process exits, then inspect the collected output.

```rust
use p4cli_20251::P4Cli;

fn main() -> std::io::Result<()> {
    let p4: P4Cli = P4Cli::new()?;
    let output: p4cli_20251::P4Output = p4.run(&["--help"])?;

    println!("exit: {}", output.exit_code());
    println!("{}", output.stdout_str()?);
    Ok(())
}
```

With timeout, working directory and environment variables via builder:

```rust
use p4cli_20251::P4Cli;
use std::time::Duration;

fn main() -> std::io::Result<()> {
    let p4: P4Cli = P4Cli::new()?;
    let output: p4cli_20251::P4Output = p4
        .command()
        .arg("info")
        .env("P4PORT", "ssl:perforce:1666")
        .timeout(Duration::from_secs(10))
        .run()?;

    if output.success() {
        println!("{}", output.stdout_str()?);
    }
    Ok(())
}
```

## Streaming API

Iterate over lines as they arrive. The stream ends with `P4StreamEvent::Exit`.

```rust
use p4cli_20251::{P4Cli, P4StreamEvent};

fn main() -> std::io::Result<()> {
    let p4: P4Cli = P4Cli::new()?;

    for event in p4.stream(&["sync", "//depot/..."])? {
        match event? {
            P4StreamEvent::Stdout(line) => println!("{line}"),
            P4StreamEvent::Stderr(line) => eprintln!("{line}"),
            P4StreamEvent::Exit(code) => println!("exit {code}"),
        }
    }
    Ok(())
}
```

Drop the stream mid-way to cancel the process.
