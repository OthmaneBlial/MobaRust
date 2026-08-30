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
        "check-fuzz" => check_fuzz(),
        "check-rdp-helper" => check_rdp_helper(),
        "check-vnc-helper" => check_vnc_helper(),
        "benchmark" => benchmark(),
        "package-check" => package_check(),
        "stage-helpers" => stage_helpers(),
        "help" | "--help" | "-h" => {
            println!("cargo xtask check    Run Rust and frontend validation locally");
            println!("cargo xtask check-fuzz    Compile and format-check isolated fuzz targets");
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
        let mut command = Command::new("cargo");
        sanitize_process_environment(&mut command);
        let isolated_home = create_sanitized_test_home()?;
        apply_isolated_home(&mut command, &isolated_home);
        let status = command
            .args(["build", "--locked", "--release", "--manifest-path"])
            .arg(&manifest_path)
            .current_dir(&repository_root)
            .status()
            .map_err(|error| format!("could not build {binary}: {error}"));
        let cleanup = fs::remove_dir_all(&isolated_home)
            .map_err(|error| format!("could not remove temporary helper home: {error}"));
        cleanup?;
        let status = status?;
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
        verify_unbundled_helper(&bundle, "mobarust-rdp-helper")?;
        println!("verified macOS app bundle and the shippable VNC helper resource");
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

fn verify_unbundled_helper(bundle: &std::path::Path, helper: &str) -> Result<(), String> {
    let path = bundle.join("Contents/Resources/helpers").join(helper);
    match fs::symlink_metadata(&path) {
        Ok(_) => Err(format!(
            "unshippable helper was included in the bundle: {}",
            path.display()
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "could not verify unshippable helper absence {}: {error}",
            path.display()
        )),
    }
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
    run("pnpm", ["run", "test:unit"], Some("apps/desktop"))?;
    run("pnpm", ["run", "check"], Some("apps/desktop"))?;
    run("pnpm", ["run", "lint"], Some("apps/desktop"))?;
    run("pnpm", ["run", "build"], Some("apps/desktop"))?;
    check_rdp_helper()?;
    check_vnc_helper()?;
    check_fuzz()?;
    Ok(())
}

fn check_fuzz() -> Result<(), String> {
    run(
        "cargo",
        ["fmt", "--manifest-path", "fuzz/Cargo.toml", "--", "--check"],
        None,
    )?;
    run(
        "cargo",
        ["check", "--locked", "--manifest-path", "fuzz/Cargo.toml"],
        None,
    )
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
    let isolated_home = create_sanitized_test_home()?;
    let mut command = Command::new(program);
    sanitize_process_environment(&mut command);
    apply_isolated_home(&mut command, &isolated_home);
    apply_project_tool_configuration(&mut command, program, directory);
    command.args(args);
    if let Some(directory) = directory {
        command.current_dir(directory);
    }
    let status = command
        .status()
        .map_err(|error| format!("could not start {program}: {error}"));
    let cleanup = fs::remove_dir_all(&isolated_home)
        .map_err(|error| format!("could not remove temporary command home: {error}"));
    cleanup?;
    let status: ExitStatus = status?;
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
    sanitize_process_environment(&mut command);
    command.args(args);
    if let Some(directory) = directory {
        command.current_dir(directory);
    }
    apply_isolated_home(&mut command, &test_home);

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

/// Keep every validation child process away from the caller's shell and user
/// configuration. Compiler and package caches remain inherited explicitly;
/// only home-directory configuration/data/cache lookup is redirected.
fn apply_isolated_home(command: &mut Command, isolated_home: &Path) {
    command.env("CI", "1");
    command.env("HOME", isolated_home);
    command.env("XDG_CONFIG_HOME", isolated_home.join("config"));
    command.env("XDG_DATA_HOME", isolated_home.join("data"));
    command.env("XDG_CACHE_HOME", isolated_home.join("cache"));
    #[cfg(unix)]
    command.env("SHELL", "/bin/sh");
    #[cfg(windows)]
    {
        command.env("ComSpec", "cmd.exe");
        command.env("USERPROFILE", isolated_home);
        command.env("APPDATA", isolated_home.join("data"));
        command.env("LOCALAPPDATA", isolated_home.join("cache"));
    }
}

/// Reuse only the package manager store already recorded by the repository's
/// own ignored install metadata. This prevents pnpm from treating the isolated
/// HOME as a new store and asking to delete/reinstall `node_modules`, while it
/// still cannot read a personal npm configuration or registry credential.
fn apply_project_tool_configuration(command: &mut Command, program: &str, directory: Option<&str>) {
    if program != "pnpm" || directory != Some("apps/desktop") {
        return;
    }

    let repository_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf);
    if let Some(repository_root) = repository_root
        && let Some(store_dir) = project_pnpm_store_dir(&repository_root)
    {
        command.env("npm_config_store_dir", store_dir);
    }
}

fn project_pnpm_store_dir(repository_root: &Path) -> Option<PathBuf> {
    let metadata =
        fs::read_to_string(repository_root.join("apps/desktop/node_modules/.modules.yaml")).ok()?;
    metadata
        .lines()
        .find_map(|line| line.strip_prefix("storeDir: "))
        .map(PathBuf::from)
}

/// Keep build, packaging, and tests from inheriting ambient SSH credentials or
/// user-selected Git transports. This does not replace the OS sandbox and it
/// deliberately leaves compiler/package caches available for reproducibility.
fn sanitize_process_environment(command: &mut Command) {
    for (variable, _) in std::env::vars_os() {
        if should_remove_process_variable(&variable) {
            command.env_remove(variable);
        }
    }
    let null_device = if cfg!(windows) { "NUL" } else { "/dev/null" };
    // npm/pnpm otherwise search the user's home for a registry token or a
    // private-registry configuration. Public lockfiles remain usable while
    // user authentication material stays out of validation subprocesses.
    command.env("NPM_CONFIG_USERCONFIG", null_device);
    command.env("NPM_CONFIG_GLOBALCONFIG", null_device);
    command.env("GIT_CONFIG_NOSYSTEM", "1");
    command.env(
        "GIT_CONFIG_GLOBAL",
        if cfg!(windows) { "NUL" } else { "/dev/null" },
    );
}

fn should_remove_process_variable(variable: &std::ffi::OsStr) -> bool {
    let variable = variable.to_string_lossy().to_ascii_uppercase();
    matches!(
        variable.as_str(),
        "SSH_AUTH_SOCK"
            | "SSH_AGENT_PID"
            | "GIT_SSH_COMMAND"
            | "GIT_SSH"
            | "GIT_SSH_VARIANT"
            | "GIT_ASKPASS"
            | "SSH_ASKPASS"
            | "CARGO_NET_GIT_FETCH_WITH_CLI"
            | "CARGO_NET_GIT_SSH"
            | "CARGO_NET_GIT_MIRROR"
            | "NETRC"
            | "SSLKEYLOGFILE"
            | "SSL_CERT_FILE"
            | "SSL_CERT_DIR"
            // Prevent test helpers and shells from loading user-selected
            // startup files even when HOME is redirected to the disposable
            // test home.
            | "BASH_ENV"
            | "ENV"
            | "ZDOTDIR"
            | "PYTHONSTARTUP"
            | "PERL5OPT"
            | "RUBYOPT"
            | "NODE_OPTIONS"
            | "GITHUB_TOKEN"
            | "GH_TOKEN"
            | "NPM_TOKEN"
            | "NODE_AUTH_TOKEN"
            | "LD_PRELOAD"
            | "LD_LIBRARY_PATH"
            | "DYLD_INSERT_LIBRARIES"
            | "DYLD_LIBRARY_PATH"
            | "DYLD_FALLBACK_LIBRARY_PATH"
            | "PYTHONPATH"
            | "PERL5LIB"
            | "RUBYLIB"
            | "NODE_PATH"
            | "RUSTC_WRAPPER"
            | "RUSTC_WORKSPACE_WRAPPER"
    ) || variable == "GIT_CONFIG"
        || variable.starts_with("GIT_CONFIG_")
        || variable.starts_with("NPM_CONFIG_")
        || variable.starts_with("CARGO_REGISTRIES_")
        || variable.starts_with("CARGO_NET_GIT_")
        || variable.starts_with("CARGO_TARGET_")
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

    #[test]
    fn process_environment_filter_covers_user_configuration_and_tool_hooks() {
        for variable in [
            "GIT_CONFIG_GLOBAL",
            "npm_config_//registry.example/:_authToken",
            "CARGO_REGISTRIES_PRIVATE_TOKEN",
            "CARGO_NET_GIT_FETCH_WITH_CLI",
            "SSH_AUTH_SOCK",
            "NETRC",
            "SSLKEYLOGFILE",
            "SSL_CERT_FILE",
            "SSL_CERT_DIR",
            "CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUNNER",
        ] {
            assert!(should_remove_process_variable(std::ffi::OsStr::new(
                variable
            )));
        }
        assert!(!should_remove_process_variable(std::ffi::OsStr::new(
            "PATH"
        )));
        assert!(!should_remove_process_variable(std::ffi::OsStr::new(
            "RUSTUP_HOME"
        )));
    }

    #[test]
    fn project_pnpm_store_dir_reads_only_ignored_install_metadata() {
        let root = std::env::temp_dir().join(format!(
            "mobarust-xtask-pnpm-metadata-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock")
                .as_nanos()
        ));
        let metadata = root.join("apps/desktop/node_modules");
        fs::create_dir_all(&metadata).expect("create metadata directory");
        fs::write(
            metadata.join(".modules.yaml"),
            "storeDir: /fixture/cache/pnpm\n",
        )
        .expect("write fixture metadata");

        assert_eq!(
            project_pnpm_store_dir(&root),
            Some(PathBuf::from("/fixture/cache/pnpm"))
        );
        fs::remove_dir_all(root).expect("remove metadata fixture");
    }

    #[test]
    fn package_verifier_rejects_the_unshippable_rdp_helper() {
        let root = std::env::temp_dir().join(format!(
            "mobarust-xtask-bundle-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock")
                .as_nanos()
        ));
        let helpers = root.join("Contents/Resources/helpers");
        fs::create_dir_all(&helpers).expect("create helper directory");
        verify_unbundled_helper(&root, "mobarust-rdp-helper")
            .expect("RDP helper should be absent initially");
        fs::write(helpers.join("mobarust-rdp-helper"), b"candidate")
            .expect("write forbidden helper");
        let error = verify_unbundled_helper(&root, "mobarust-rdp-helper").unwrap_err();
        assert!(error.contains("unshippable helper"));
        fs::remove_dir_all(root).expect("remove test bundle");
    }
}
