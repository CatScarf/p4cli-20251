# P4CLI-20251

A simple Rust library for full-featured P4 CLI with zero pre-install, where all errors are propagated via `Result` without any `unwrap`.

## Usage

```toml
[dependencies]
p4cli-20251 = "0.2.0"
```

```rust
use p4cli_20251::P4Cli;

fn main() -> std::io::Result<()> {
    let p4 = P4Cli::new()?;
    for line in p4.run(&["--help"])? {
        println!("{}", line?);
    }
    Ok(())
}
```
