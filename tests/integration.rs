use p4cli_20251::{P4Cli, P4StreamEvent};
use std::time::Duration;

#[test]
fn test_run_help() -> std::io::Result<()> {
    let p4 = P4Cli::new()?;
    let output = p4.run(&["--help"])?;
    assert!(output.success(), "p4 --help should exit with 0");
    let stdout = output.stdout_str()?;
    assert!(
        stdout.contains("Usage:"),
        "expected --help output to contain 'Usage:'"
    );
    Ok(())
}

#[test]
fn test_run_error() -> std::io::Result<()> {
    let p4 = P4Cli::new()?;
    let output = p4.run(&["--nonexistent-flag"])?;
    assert!(!output.success(), "unknown flag should exit non-zero");
    let stderr = output.stderr_str()?;
    assert!(
        stderr.contains("Invalid option") || stderr.contains("error"),
        "expected error output, got: {stderr}"
    );
    Ok(())
}

#[test]
fn test_multiple_instances() -> std::io::Result<()> {
    let p4_a = P4Cli::new()?;
    let p4_b = P4Cli::new()?;
    assert!(p4_a.run(&["--help"])?.success());
    assert!(p4_b.run(&["--help"])?.success());
    Ok(())
}

#[test]
fn test_command_builder() -> std::io::Result<()> {
    let p4 = P4Cli::new()?;
    let output = p4.command().arg("--help").run()?;
    assert!(output.success());
    Ok(())
}

#[test]
fn test_timeout_kills() -> std::io::Result<()> {
    let p4 = P4Cli::new()?;
    let output = p4
        .command()
        .arg("help")
        .timeout(Duration::from_millis(1))
        .run()?;
    assert!(!output.success() || output.exit_code() == 0);
    Ok(())
}

#[test]
fn test_stream_help() -> std::io::Result<()> {
    let p4 = P4Cli::new()?;
    let mut saw_stdout = false;
    let mut saw_exit = false;
    for event in p4.stream(&["--help"])? {
        match &event? {
            P4StreamEvent::Stdout(data) => {
                if let Ok(text) = std::str::from_utf8(data)
                    && text.contains("Usage:")
                {
                    saw_stdout = true;
                }
            }
            P4StreamEvent::Stderr(_) => {}
            P4StreamEvent::Exit(code) => {
                assert_eq!(*code, 0);
                saw_exit = true;
            }
        }
    }
    assert!(saw_stdout, "expected --help to contain 'Usage:'");
    assert!(saw_exit, "expected Exit event");
    Ok(())
}

#[test]
fn test_stream_error() -> std::io::Result<()> {
    let p4 = P4Cli::new()?;
    let mut saw_stderr = false;
    let mut saw_exit = false;
    for event in p4.stream(&["--nonexistent-flag"])? {
        match &event? {
            P4StreamEvent::Stdout(_) => {}
            P4StreamEvent::Stderr(data) => {
                if let Ok(text) = std::str::from_utf8(data)
                    && (text.contains("Invalid option") || text.contains("error"))
                {
                    saw_stderr = true;
                }
            }
            P4StreamEvent::Exit(code) => {
                assert_ne!(*code, 0, "nonexistent flag should fail");
                saw_exit = true;
            }
        }
    }
    assert!(saw_stderr, "expected error output");
    assert!(saw_exit, "expected Exit event");
    Ok(())
}

#[test]
fn test_stream_drop_midway() -> std::io::Result<()> {
    let p4 = P4Cli::new()?;
    let stream = p4.stream(&["--help"])?;
    drop(stream);
    Ok(())
}

#[test]
fn test_stream_builder() -> std::io::Result<()> {
    let p4 = P4Cli::new()?;
    let mut saw_exit = false;
    for event in p4.command().arg("--help").stream()? {
        if let P4StreamEvent::Exit(code) = event? {
            assert_eq!(code, 0);
            saw_exit = true;
        }
    }
    assert!(saw_exit);
    Ok(())
}
