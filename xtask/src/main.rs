use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

fn main() {
    let command = std::env::args().nth(1).unwrap_or_else(|| "help".to_owned());
    let result = match command.as_str() {
        "check" => check(),
        "check-rdp-helper" => check_rdp_helper(),
        "check-vnc-helper" => check_vnc_helper(),
        "benchmark" => benchmark(),
        "package-check" => package_check(),
        "stage-helpers" => stage_helpers(),
        "help" | "--help" | "-h" => {
            println!("cargo xtask check    Run Rust and frontend validation locally");
            println!("cargo xtask check-rdp-helper    Validate the isolated RDP helper locally");
            println!("cargo xtask check-vnc-helper    Validate the isolated VNC helper locally");
            println!("cargo xtask benchmark    Run synthetic local performance probes");
            println!(
                "cargo xtask package-check    Build and inspect an unsigned current-platform Tauri app bundle"
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
    // The isolated IronRDP candidate currently pulls rsa 0.10.0-rc.18 through
    // picky and fails the local RUSTSEC-2023-0071 audit. Keep it available for
    // isolated development checks, but never put that candidate in a normal
    // application bundle until its dependency chain is fixed and re-audited.
    let helpers = [("tools/vnc-helper/Cargo.toml", "mobarust-vnc-helper")];
    remove_unshippable_rdp_helper(&staging_directory, executable_suffix)?;

    for (manifest, binary) in helpers {
        let manifest_path = repository_root.join(manifest);
        let status = Command::new("cargo")
            .args(["build", "--locked", "--release", "--manifest-path"])
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

fn remove_unshippable_rdp_helper(
    staging_directory: &std::path::Path,
    executable_suffix: &str,
) -> Result<(), String> {
    let path = staging_directory.join(format!("mobarust-rdp-helper{executable_suffix}"));
    match fs::remove_file(&path) {
        Ok(()) => println!(
            "removed unshippable experimental RDP helper {}",
            path.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!(
                "could not remove unshippable experimental RDP helper {}: {error}",
                path.display()
            ));
        }
    }
    Ok(())
}

fn package_check() -> Result<(), String> {
    run(
        "pnpm",
        ["tauri", "build", "--debug", "--no-sign", "--bundles", "app"],
        Some("apps/desktop"),
    )?;
    verify_current_platform_bundle()
}

fn verify_current_platform_bundle() -> Result<(), String> {
    let repository_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or_else(|| "xtask manifest has no repository parent".to_owned())?
        .to_path_buf();
    let bundle_root = repository_root.join("target/debug/bundle");
    if !bundle_root.is_dir() {
        return Err(format!(
            "Tauri completed without a bundle directory: {}",
            bundle_root.display()
        ));
    }

    if cfg!(target_os = "macos") {
        let bundle = bundle_root.join("macos/MobaRust.app");
        let executable = bundle.join("Contents/MacOS/mobarust");
        if !executable.is_file() {
            return Err(format!(
                "macOS bundle executable is missing: {}",
                executable.display()
            ));
        }
        verify_bundled_helper(&bundle, "mobarust-vnc-helper")?;
        println!("verified macOS app bundle and native helper resources");
    } else {
        println!(
            "verified current-platform Tauri bundle directory: {}",
            bundle_root.display()
        );
    }

    let artifact_root = if cfg!(target_os = "macos") {
        bundle_root.join("macos/MobaRust.app")
    } else {
        bundle_root.clone()
    };
    let manifest_path = bundle_root.join("MobaRust.sha256");
    write_checksum_manifest(&bundle_root, &artifact_root, &manifest_path)?;
    verify_checksum_manifest(&bundle_root, &artifact_root, &manifest_path)?;
    println!(
        "wrote and verified artifact checksum manifest: {}",
        manifest_path.display()
    );
    Ok(())
}

/// Write a standard sha256sum-compatible manifest for the current artifact.
/// The manifest is kept beside the generated bundle, not inside the app, so it
/// cannot accidentally become part of the runtime resource set. Symlinks are
/// rejected to keep the checksum scope deterministic and prevent an artifact
/// entry from escaping the bundle root.
fn write_checksum_manifest(
    bundle_root: &Path,
    artifact_root: &Path,
    manifest_path: &Path,
) -> Result<(), String> {
    let contents = checksum_manifest_contents(bundle_root, artifact_root, manifest_path)?;
    fs::write(manifest_path, contents)
        .map_err(|error| format!("could not write checksum manifest: {error}"))
}

fn verify_checksum_manifest(
    bundle_root: &Path,
    artifact_root: &Path,
    manifest_path: &Path,
) -> Result<(), String> {
    let expected = checksum_manifest_contents(bundle_root, artifact_root, manifest_path)?;
    let actual = fs::read_to_string(manifest_path)
        .map_err(|error| format!("could not read checksum manifest: {error}"))?;
    if actual != expected {
        return Err(format!(
            "checksum manifest changed during verification: {}",
            manifest_path.display()
        ));
    }
    Ok(())
}

fn checksum_manifest_contents(
    bundle_root: &Path,
    artifact_root: &Path,
    manifest_path: &Path,
) -> Result<String, String> {
    let mut files = Vec::new();
    collect_artifact_files(artifact_root, manifest_path, &mut files)?;
    files.sort();

    let mut manifest = String::new();
    for path in files {
        let relative = path
            .strip_prefix(bundle_root)
            .map_err(|error| {
                format!(
                    "artifact file {} is outside bundle root {}: {error}",
                    path.display(),
                    bundle_root.display()
                )
            })?
            .to_str()
            .ok_or_else(|| format!("artifact path is not valid UTF-8: {}", path.display()))?
            .replace('\\', "/");
        let digest = sha256_file(&path)?;
        // Two spaces keep the manifest readable by both sha256sum and macOS
        // shasum without relying on a platform-specific binary marker.
        manifest.push_str(&format!("{digest}  {relative}\n"));
    }
    Ok(manifest)
}

fn collect_artifact_files(
    directory: &Path,
    manifest_path: &Path,
    files: &mut Vec<PathBuf>,
) -> Result<(), String> {
    let metadata = fs::symlink_metadata(directory).map_err(|error| {
        format!(
            "could not inspect artifact path {}: {error}",
            directory.display()
        )
    })?;
    if !metadata.is_dir() {
        return Err(format!(
            "artifact root is not a directory: {}",
            directory.display()
        ));
    }

    let mut children = fs::read_dir(directory)
        .map_err(|error| format!("could not enumerate artifact directory: {error}"))?
        .map(|entry| {
            entry
                .map(|entry| entry.path())
                .map_err(|error| format!("could not read artifact directory entry: {error}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    children.sort();

    for path in children {
        if path == manifest_path {
            continue;
        }
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            format!(
                "could not inspect artifact entry {}: {error}",
                path.display()
            )
        })?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "artifact contains unsupported symlink: {}",
                path.display()
            ));
        }
        if metadata.is_dir() {
            collect_artifact_files(&path, manifest_path, files)?;
        } else if metadata.is_file() {
            files.push(path);
        } else {
            return Err(format!(
                "artifact contains unsupported file type: {}",
                path.display()
            ));
        }
    }
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = fs::File::open(path)
        .map_err(|error| format!("could not open artifact file {}: {error}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("could not hash artifact file {}: {error}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let digest = hasher.finalize();
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn verify_bundled_helper(bundle: &std::path::Path, helper: &str) -> Result<(), String> {
    let path = bundle.join("Contents/Resources/helpers").join(helper);
    let metadata = fs::symlink_metadata(&path).map_err(|error| {
        format!(
            "could not inspect bundled helper {}: {error}",
            path.display()
        )
    })?;
    if !metadata.file_type().is_file() {
        return Err(format!(
            "bundled helper is not a regular file: {}",
            path.display()
        ));
    }
    Ok(())
}

fn benchmark() -> Result<(), String> {
    run(
        "cargo",
        [
            "run",
            "--release",
            "--manifest-path",
            "benchmarks/Cargo.toml",
            "--",
        ],
        None,
    )
}

fn check() -> Result<(), String> {
    run("cargo", ["fmt", "--all", "--", "--check"], None)?;
    run_sanitized_test("cargo", ["test", "--locked", "--workspace"], None)?;
    run(
        "cargo",
        [
            "clippy",
            "--locked",
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
        [
            "test",
            "--locked",
            "--manifest-path",
            "tools/rdp-helper/Cargo.toml",
        ],
        None,
    )?;
    run(
        "cargo",
        [
            "clippy",
            "--locked",
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
        [
            "test",
            "--locked",
            "--manifest-path",
            "tools/vnc-helper/Cargo.toml",
        ],
        None,
    )?;
    run(
        "cargo",
        [
            "clippy",
            "--locked",
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

/// Run tests without inheriting personal process, Git, or home-directory
/// configuration.
fn run_sanitized_test<const N: usize>(
    program: &str,
    args: [&str; N],
    directory: Option<&str>,
) -> Result<(), String> {
    let test_home = create_sanitized_test_home()?;
    let mut command = Command::new(program);
    command.args(args);
    if let Some(directory) = directory {
        command.current_dir(directory);
    }
    command.env("HOME", &test_home);
    command.env("XDG_CONFIG_HOME", test_home.join("config"));
    command.env("XDG_DATA_HOME", test_home.join("data"));
    command.env("XDG_CACHE_HOME", test_home.join("cache"));
    #[cfg(windows)]
    {
        command.env("USERPROFILE", &test_home);
        command.env("APPDATA", test_home.join("data"));
        command.env("LOCALAPPDATA", test_home.join("cache"));
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

    let status = command
        .status()
        .map_err(|error| format!("could not start {program}: {error}"));
    let cleanup = fs::remove_dir_all(&test_home)
        .map_err(|error| format!("could not remove temporary test home: {error}"));
    cleanup?;
    let status: ExitStatus = status?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{program} exited with {status}"))
    }
}

fn create_sanitized_test_home() -> Result<PathBuf, String> {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("system clock is before the Unix epoch: {error}"))?
        .as_nanos();
    let test_home = std::env::temp_dir().join(format!(
        "mobarust-safe-test-home-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir(&test_home).map_err(|error| {
        format!(
            "could not create isolated test home {}: {error}",
            test_home.display()
        )
    })?;
    for directory in ["config", "data", "cache"] {
        if let Err(error) = fs::create_dir(test_home.join(directory)) {
            let _ = fs::remove_dir_all(&test_home);
            return Err(format!(
                "could not create isolated test directory {directory}: {error}"
            ));
        }
    }
    Ok(test_home)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_manifest_is_deterministic_and_excludes_itself() {
        let root = std::env::temp_dir().join(format!(
            "mobarust-xtask-checksum-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock")
                .as_nanos()
        ));
        fs::create_dir_all(root.join("artifact/nested")).expect("create test artifact");
        fs::write(root.join("artifact/z.txt"), b"z").expect("write z");
        fs::write(root.join("artifact/nested/a.txt"), b"a").expect("write a");
        let manifest = root.join("MobaRust.sha256");

        write_checksum_manifest(&root, &root.join("artifact"), &manifest).expect("write manifest");
        verify_checksum_manifest(&root, &root.join("artifact"), &manifest)
            .expect("verify manifest");
        let contents = fs::read_to_string(&manifest).expect("read manifest");
        let lines = contents.lines().collect::<Vec<_>>();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].ends_with("artifact/nested/a.txt"));
        assert!(lines[1].ends_with("artifact/z.txt"));

        fs::remove_dir_all(root).expect("remove test artifact");
    }
}
