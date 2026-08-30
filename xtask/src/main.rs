use std::fs;
use std::path::PathBuf;
use std::process::{Command, ExitStatus};

fn main() {
    let command = std::env::args().nth(1).unwrap_or_else(|| "help".to_owned());
    let result = match command.as_str() {
        "check" => check(),
        "check-rdp-helper" => check_rdp_helper(),
        "check-vnc-helper" => check_vnc_helper(),
        "package-check" => package_check(),
        "stage-helpers" => stage_helpers(),
        "help" | "--help" | "-h" => {
            println!("cargo xtask check    Run Rust and frontend validation locally");
            println!("cargo xtask check-rdp-helper    Validate the isolated RDP helper locally");
            println!("cargo xtask check-vnc-helper    Validate the isolated VNC helper locally");
            println!(
                "cargo xtask package-check    Build an unsigned current-platform Tauri app bundle"
            );
            println!(
                "cargo xtask stage-helpers    Build and stage ignored desktop helper resources"
            );
            Ok(())
        }
        other => Err(format!("unknown xtask command: {other}")),
    };

    if let Err(error) = result {
        eprintln!("xtask: {error}");
        std::process::exit(1);
    }
}

fn stage_helpers() -> Result<(), String> {
    let repository_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or_else(|| "xtask manifest has no repository parent".to_owned())?
        .to_path_buf();
    let staging_directory = repository_root.join("apps/desktop/src-tauri/helpers");
    fs::create_dir_all(&staging_directory)
        .map_err(|error| format!("could not create helper staging directory: {error}"))?;
    let executable_suffix = if cfg!(windows) { ".exe" } else { "" };
    let helpers = [
        ("tools/rdp-helper/Cargo.toml", "mobarust-rdp-helper"),
        ("tools/vnc-helper/Cargo.toml", "mobarust-vnc-helper"),
    ];

    for (manifest, binary) in helpers {
        let manifest_path = repository_root.join(manifest);
        let status = Command::new("cargo")
            .args(["build", "--release", "--manifest-path"])
            .arg(&manifest_path)
            .current_dir(&repository_root)
            .status()
            .map_err(|error| format!("could not build {binary}: {error}"))?;
        if !status.success() {
            return Err(format!(
                "cargo failed while building {binary} with {status}"
            ));
        }
        let source = manifest_path
            .parent()
            .ok_or_else(|| format!("helper manifest has no parent: {}", manifest_path.display()))?
            .join("target/release")
            .join(format!("{binary}{executable_suffix}"));
        let source_metadata = fs::symlink_metadata(&source).map_err(|error| {
            format!(
                "could not inspect built {binary} at {}: {error}",
                source.display()
            )
        })?;
        if !source_metadata.file_type().is_file() {
            return Err(format!(
                "built {binary} is not a regular file: {}",
                source.display()
            ));
        }
        let destination = staging_directory.join(format!("{binary}{executable_suffix}"));
        fs::copy(&source, &destination).map_err(|error| {
            format!(
                "could not stage {binary} from {} to {}: {error}",
                source.display(),
                destination.display()
            )
        })?;
        println!("staged {}", destination.display());
    }
    Ok(())
}

fn package_check() -> Result<(), String> {
    run(
        "pnpm",
        ["tauri", "build", "--debug", "--no-sign", "--bundles", "app"],
        Some("apps/desktop"),
    )
}

fn check() -> Result<(), String> {
    run("cargo", ["fmt", "--all", "--", "--check"], None)?;
    run_sanitized_test("cargo", ["test", "--workspace"], None)?;
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
    run_sanitized_test(
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
    run_sanitized_test(
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

/// Run tests without inheriting ambient SSH agents or Git SSH configuration.
///
/// This is intentionally narrower than changing HOME: Cargo and rustup still
/// need their normal toolchain/cache locations, while protocol fixtures must
/// never accidentally discover a user's agent or personal Git configuration.
fn run_sanitized_test<const N: usize>(
    program: &str,
    args: [&str; N],
    directory: Option<&str>,
) -> Result<(), String> {
    let mut command = Command::new(program);
    command.args(args);
    if let Some(directory) = directory {
        command.current_dir(directory);
    }

    for variable in [
        "SSH_AUTH_SOCK",
        "SSH_AGENT_PID",
        "GIT_SSH_COMMAND",
        "GIT_SSH",
        "GIT_ASKPASS",
        "SSH_ASKPASS",
    ] {
        command.env_remove(variable);
    }
    command.env_remove("GIT_CONFIG_GLOBAL");
    command.env_remove("GIT_CONFIG_SYSTEM");
    command.env("GIT_CONFIG_NOSYSTEM", "1");
    command.env(
        "GIT_CONFIG_GLOBAL",
        if cfg!(windows) { "NUL" } else { "/dev/null" },
    );

    let status: ExitStatus = command
        .status()
        .map_err(|error| format!("could not start {program}: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{program} exited with {status}"))
    }
}
