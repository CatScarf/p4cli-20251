# P4CLI-20251

A simple Rust library to interact with Perforce command-line tools.

## Usage

```toml
[dependencies]
p4cli-20251 = "0.1.0"
```

```rust
use p4cli_20251::P4Cli;

let p4 = P4Cli::new();
for line in p4.run(&["--help"]) {
    println!("{}", line);
}
```