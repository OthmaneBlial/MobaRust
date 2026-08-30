use std::process::{Command, ExitStatus};

fn main() {
    let command = std::env::args().nth(1).unwrap_or_else(|| "help".to_owned());
    let result = match command.as_str() {
        "check" => check(),
        "check-rdp-helper" => check_rdp_helper(),
        "check-vnc-helper" => check_vnc_helper(),
        "help" | "--help" | "-h" => {
            println!("cargo xtask check    Run Rust and frontend validation locally");
            println!("cargo xtask check-rdp-helper    Validate the isolated RDP helper locally");
            println!("cargo xtask check-vnc-helper    Validate the isolated VNC helper locally");
            Ok(())
        }
        other => Err(format!("unknown xtask command: {other}")),
    };

    if let Err(error) = result {
        eprintln!("xtask: {error}");
        std::process::exit(1);
    }
}

fn check() -> Result<(), String> {
    run("cargo", ["fmt", "--all", "--", "--check"], None)?;
    run("cargo", ["test", "--workspace"], None)?;
    run(
        "cargo",
        [
            "clippy",
            "--workspace",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ],
        None,
    )?;

    run(
        "pnpm",
        ["install", "--frozen-lockfile"],
        Some("apps/desktop"),
    )?;
    run("pnpm", ["run", "check"], Some("apps/desktop"))?;
    run("pnpm", ["run", "lint"], Some("apps/desktop"))?;
    run("pnpm", ["run", "build"], Some("apps/desktop"))?;
    check_rdp_helper()?;
    check_vnc_helper()?;
    Ok(())
}

fn check_rdp_helper() -> Result<(), String> {
    run(
        "cargo",
        [
            "fmt",
            "--manifest-path",
            "tools/rdp-helper/Cargo.toml",
            "--",
            "--check",
        ],
        None,
    )?;
    run(
        "cargo",
        ["test", "--manifest-path", "tools/rdp-helper/Cargo.toml"],
        None,
    )?;
    run(
        "cargo",
        [
            "clippy",
            "--manifest-path",
            "tools/rdp-helper/Cargo.toml",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ],
        None,
    )
}

fn check_vnc_helper() -> Result<(), String> {
    run(
        "cargo",
        [
            "fmt",
            "--manifest-path",
            "tools/vnc-helper/Cargo.toml",
            "--",
            "--check",
        ],
        None,
    )?;
    run(
        "cargo",
        ["test", "--manifest-path", "tools/vnc-helper/Cargo.toml"],
        None,
    )?;
    run(
        "cargo",
        [
            "clippy",
            "--manifest-path",
            "tools/vnc-helper/Cargo.toml",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ],
        None,
    )
}

fn run<const N: usize>(
    program: &str,
    args: [&str; N],
    directory: Option<&str>,
) -> Result<(), String> {
    let mut command = Command::new(program);
    command.args(args);
    if let Some(directory) = directory {
        command.current_dir(directory);
    }
    let status: ExitStatus = command
        .status()
        .map_err(|error| format!("could not start {program}: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{program} exited with {status}"))
    }
}
