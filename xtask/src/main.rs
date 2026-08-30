use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

fn main() {
    let mut arguments = std::env::args().skip(1);
    let command = arguments.next().unwrap_or_else(|| "help".to_owned());
    let result = match command.as_str() {
        "check" => check(),
        "check-fuzz" => check_fuzz(),
        "check-rdp-helper" => check_rdp_helper(),
        "check-vnc-helper" => check_vnc_helper(),
        "benchmark" => benchmark(),
        "benchmark-app" => benchmark_app(arguments.collect()),
        "package-check" => package_check(),
        "package-layout-check" => package_layout_check(),
        "verify-platform-layout" => verify_platform_layout_command(arguments.collect()),
        "portable-check" => portable_check(),
        "stage-helpers" => stage_helpers(),
        "stage-rdp-helper" => stage_experimental_rdp_helper(),
        "pre-push-check" => pre_push_check(),
        "verify-checksum" => verify_checksum_command(arguments.collect()),
        "help" | "--help" | "-h" => {
            println!("cargo xtask check    Run Rust and frontend validation locally");
            println!("cargo xtask check-fuzz    Compile and format-check isolated fuzz targets");
            println!("cargo xtask check-rdp-helper    Validate the isolated RDP helper locally");
            println!("cargo xtask check-vnc-helper    Validate the isolated VNC helper locally");
            println!("cargo xtask benchmark    Run synthetic local performance probes");
            println!(
                "cargo xtask benchmark-app <path>    Measure the explicit local app --version startup path"
            );
            println!(
                "cargo xtask package-check    Build and inspect an unsigned current-platform Tauri app bundle"
            );
            println!(
                "cargo xtask package-layout-check    Validate the macOS, Windows, and Linux runtime package contract"
            );
            println!(
                "cargo xtask verify-platform-layout <macos|windows|linux> <app-root>    Verify an explicit unpacked app layout"
            );
            println!(
                "cargo xtask portable-check    Assemble and inspect an unsigned current-platform portable archive"
            );
            println!(
                "cargo xtask stage-helpers    Build and stage ignored desktop helper resources"
            );
            println!(
                "cargo xtask stage-rdp-helper    Explicitly stage the isolated RDP candidate for local development"
            );
            println!(
                "cargo xtask pre-push-check    Audit the local Git payload without network access"
            );
            println!(
                "cargo xtask verify-checksum <artifact-dir> <manifest>    Verify an explicit artifact checksum manifest"
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
        build_and_stage_helper(
            &repository_root,
            &staging_directory,
            manifest,
            binary,
            executable_suffix,
        )?;
    }
    Ok(())
}

/// Explicitly stage the isolated RDP candidate for a local development run.
///
/// Normal application preparation never calls this path: the candidate has a
/// separate dependency audit and is intentionally excluded from distributable
/// bundles until that gate and real interoperability evidence are complete.
fn stage_experimental_rdp_helper() -> Result<(), String> {
    let repository_root = repository_root()?;
    let staging_directory = repository_root.join("apps/desktop/src-tauri/helpers");
    fs::create_dir_all(&staging_directory)
        .map_err(|error| format!("could not create helper staging directory: {error}"))?;
    let executable_suffix = if cfg!(windows) { ".exe" } else { "" };
    build_and_stage_helper(
        &repository_root,
        &staging_directory,
        "tools/rdp-helper/Cargo.toml",
        "mobarust-rdp-helper",
        executable_suffix,
    )?;
    println!(
        "staged experimental RDP helper for local development only; normal bundles still exclude it"
    );
    Ok(())
}

fn build_and_stage_helper(
    repository_root: &Path,
    staging_directory: &Path,
    manifest: &str,
    binary: &str,
    executable_suffix: &str,
) -> Result<(), String> {
    let manifest_path = repository_root.join(manifest);
    let mut command = Command::new("cargo");
    sanitize_process_environment(&mut command);
    let isolated_home = create_sanitized_test_home()?;
    apply_isolated_home(&mut command, &isolated_home);
    let status = command
        .args(["build", "--locked", "--release", "--manifest-path"])
        .arg(&manifest_path)
        .current_dir(repository_root)
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PackagePlatform {
    Macos,
    Windows,
    Linux,
}

impl PackagePlatform {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "macos" => Ok(Self::Macos),
            "windows" => Ok(Self::Windows),
            "linux" => Ok(Self::Linux),
            _ => Err(format!(
                "unknown package platform {value}; expected macos, windows, or linux"
            )),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Macos => "macOS",
            Self::Windows => "Windows",
            Self::Linux => "Linux",
        }
    }

    fn executable(self) -> &'static str {
        match self {
            Self::Macos => "Contents/MacOS/mobarust",
            Self::Windows => "mobarust.exe",
            Self::Linux => "mobarust",
        }
    }

    fn vnc_helper(self) -> &'static str {
        match self {
            Self::Macos => "Contents/Resources/helpers/mobarust-vnc-helper",
            Self::Windows => "helpers/mobarust-vnc-helper.exe",
            Self::Linux => "helpers/mobarust-vnc-helper",
        }
    }

    fn rdp_helper(self) -> &'static str {
        match self {
            Self::Macos => "Contents/Resources/helpers/mobarust-rdp-helper",
            Self::Windows => "helpers/mobarust-rdp-helper.exe",
            Self::Linux => "helpers/mobarust-rdp-helper",
        }
    }
}

/// Validate the platform-specific runtime shape without building or signing
/// an artifact. `package_root` is the app root: the `.app` directory on
/// macOS, and the unpacked application directory on Windows/Linux. Keeping
/// this contract separate from the host build lets CI validate each target
/// later without weakening the current Mac-only smoke test.
fn verify_platform_package_layout(
    package_root: &Path,
    platform: PackagePlatform,
) -> Result<(), String> {
    let executable = package_root.join(platform.executable());
    require_regular_file(
        &executable,
        &format!("{} runtime executable", platform.label()),
    )?;

    let vnc_helper = package_root.join(platform.vnc_helper());
    require_regular_file(&vnc_helper, &format!("{} VNC helper", platform.label()))?;

    let rdp_helper = package_root.join(platform.rdp_helper());
    match fs::symlink_metadata(&rdp_helper) {
        Ok(_) => Err(format!(
            "{} package contains the unshippable RDP helper: {}",
            platform.label(),
            rdp_helper.display()
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "could not verify {} RDP helper absence {}: {error}",
            platform.label(),
            rdp_helper.display()
        )),
    }
}

fn require_regular_file(path: &Path, description: &str) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        format!(
            "could not inspect {description} {}: {error}",
            path.display()
        )
    })?;
    if metadata.file_type().is_file() {
        Ok(())
    } else {
        Err(format!(
            "{description} is not a regular file: {}",
            path.display()
        ))
    }
}

fn package_layout_check() -> Result<(), String> {
    let root = repository_root()?.join("target/debug/package-layout-check");
    remove_generated_path(&root)?;

    let result =
        (|| {
            for platform in [
                PackagePlatform::Macos,
                PackagePlatform::Windows,
                PackagePlatform::Linux,
            ] {
                let package = root.join(platform.label().to_ascii_lowercase());
                fs::create_dir_all(package.join(platform.vnc_helper()).parent().ok_or_else(
                    || format!("{} package helper path has no parent", platform.label()),
                )?)
                .map_err(|error| {
                    format!(
                        "could not create {} package fixture: {error}",
                        platform.label()
                    )
                })?;
                fs::create_dir_all(package.join(platform.executable()).parent().ok_or_else(
                    || format!("{} executable path has no parent", platform.label()),
                )?)
                .map_err(|error| {
                    format!(
                        "could not create {} executable fixture: {error}",
                        platform.label()
                    )
                })?;
                fs::write(package.join(platform.executable()), b"runtime").map_err(|error| {
                    format!(
                        "could not write {} runtime fixture: {error}",
                        platform.label()
                    )
                })?;
                fs::write(package.join(platform.vnc_helper()), b"vnc-helper").map_err(|error| {
                    format!("could not write {} VNC fixture: {error}", platform.label())
                })?;
                verify_platform_package_layout(&package, platform)?;
            }
            Ok(())
        })();
    let cleanup = remove_generated_path(&root);
    match (result, cleanup) {
        (Err(error), _) => Err(error),
        (Ok(()), Err(error)) => Err(error),
        (Ok(()), Ok(())) => {
            println!("verified unsigned runtime package layouts for macOS, Windows, and Linux");
            Ok(())
        }
    }
}

fn verify_platform_layout_command(arguments: Vec<String>) -> Result<(), String> {
    if arguments.len() != 2 {
        return Err(
            "usage: cargo xtask verify-platform-layout <macos|windows|linux> <app-root>".to_owned(),
        );
    }
    let platform = PackagePlatform::parse(&arguments[0])?;
    let package_root = PathBuf::from(&arguments[1]);
    verify_platform_package_layout(&package_root, platform)?;
    println!(
        "verified {} runtime package layout: {}",
        platform.label(),
        package_root.display()
    );
    Ok(())
}

fn portable_check() -> Result<(), String> {
    if !cfg!(target_os = "macos") {
        return Err(
            "portable-check currently requires a macOS host; cross-platform packaging evidence is pending"
                .to_owned(),
        );
    }

    package_check()?;

    let repository_root = repository_root()?;
    let bundle = repository_root.join("target/debug/bundle/macos/MobaRust.app");
    let portable_root = repository_root.join("target/debug/portable");
    let package_name = current_platform_package_name();
    let package_directory = portable_root.join(&package_name);
    let archive_name = format!("{package_name}.tar.gz");
    let archive = portable_root.join(&archive_name);
    let archive_manifest = portable_root.join(format!("{archive_name}.sha256"));

    remove_generated_path(&package_directory)?;
    remove_generated_path(&archive)?;
    remove_generated_path(&archive_manifest)?;
    fs::create_dir_all(&package_directory)
        .map_err(|error| format!("could not create portable package directory: {error}"))?;

    copy_regular_tree(&bundle, &package_directory.join("MobaRust.app"))?;
    copy_regular_tree(
        &repository_root.join("LICENSE"),
        &package_directory.join("LICENSE"),
    )?;
    copy_regular_tree(
        &repository_root.join("NOTICE"),
        &package_directory.join("NOTICE"),
    )?;
    fs::write(
        package_directory.join("MobaRust.app/Contents/MacOS/portable.flag"),
        b"",
    )
    .map_err(|error| format!("could not write portable mode marker: {error}"))?;
    fs::write(
        package_directory.join("PORTABLE-UNSIGNED.txt"),
        format!(
            "MobaRust {version} portable package\n\nThis local package is unsigned and intended for repository-scoped smoke testing only.\nIt is not notarized and does not establish cross-platform or interoperability evidence.\nThe package includes a portable.flag marker beside the macOS executable.\nPortable credentials remain in the separate encrypted vault and require an explicit unlock.\n",
            version = env!("CARGO_PKG_VERSION")
        ),
    )
    .map_err(|error| format!("could not write portable package notice: {error}"))?;

    let package_manifest = package_directory.join("MobaRust.sha256");
    write_checksum_manifest(&package_directory, &package_directory, &package_manifest)?;
    verify_checksum_manifest(&package_directory, &package_directory, &package_manifest)?;
    create_portable_archive(&portable_root, &package_name, &archive)?;
    verify_portable_archive(&portable_root, &package_name, &archive)?;
    write_archive_checksum_manifest(&portable_root, &archive, &archive_manifest)?;
    verify_archive_checksum_manifest(&portable_root, &archive, &archive_manifest)?;

    println!(
        "assembled and verified unsigned portable archive: {}",
        archive.display()
    );
    println!(
        "wrote and verified archive checksum manifest: {}",
        archive_manifest.display()
    );
    Ok(())
}

fn repository_root() -> Result<PathBuf, String> {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or_else(|| "xtask manifest has no repository parent".to_owned())
        .map(Path::to_path_buf)
}

fn current_platform_package_name() -> String {
    let architecture = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "x64",
        other => other,
    };
    format!("MobaRust-{}-{architecture}", std::env::consts::OS)
}

fn remove_generated_path(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || metadata.is_file() => {
            fs::remove_file(path).map_err(|error| {
                format!(
                    "could not remove generated file {}: {error}",
                    path.display()
                )
            })
        }
        Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(path).map_err(|error| {
            format!(
                "could not remove generated directory {}: {error}",
                path.display()
            )
        }),
        Ok(_) => Err(format!(
            "generated path has unsupported type: {}",
            path.display()
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "could not inspect generated path {}: {error}",
            path.display()
        )),
    }
}

/// Copy only regular files and directories into the generated package. A
/// bundle symlink is rejected so packaging cannot accidentally follow a path
/// outside the repository-owned artifact tree.
fn copy_regular_tree(source: &Path, destination: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(source).map_err(|error| {
        format!(
            "could not inspect portable source {}: {error}",
            source.display()
        )
    })?;
    if metadata.file_type().is_symlink() {
        return Err(format!(
            "portable source contains unsupported symlink: {}",
            source.display()
        ));
    }
    if metadata.is_file() {
        fs::copy(source, destination).map_err(|error| {
            format!(
                "could not copy portable file {} to {}: {error}",
                source.display(),
                destination.display()
            )
        })?;
        fs::set_permissions(destination, metadata.permissions()).map_err(|error| {
            format!(
                "could not preserve portable file permissions for {}: {error}",
                destination.display()
            )
        })?;
        return Ok(());
    }
    if !metadata.is_dir() {
        return Err(format!(
            "portable source has unsupported file type: {}",
            source.display()
        ));
    }

    fs::create_dir_all(destination).map_err(|error| {
        format!(
            "could not create portable destination {}: {error}",
            destination.display()
        )
    })?;
    let mut children = fs::read_dir(source)
        .map_err(|error| format!("could not enumerate portable source: {error}"))?
        .map(|entry| {
            entry
                .map(|entry| entry.path())
                .map_err(|error| format!("could not read portable source entry: {error}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    children.sort();
    for child in children {
        let name = child
            .file_name()
            .ok_or_else(|| format!("portable source entry has no name: {}", child.display()))?;
        copy_regular_tree(&child, &destination.join(name))?;
    }
    Ok(())
}

fn create_portable_archive(
    portable_root: &Path,
    package_name: &str,
    archive: &Path,
) -> Result<(), String> {
    let archive_name = archive
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            format!(
                "portable archive path is not valid UTF-8: {}",
                archive.display()
            )
        })?;
    let output = run_sanitized_output("tar", &["-czf", archive_name, package_name], portable_root)?;
    if !output.status.success() {
        return Err(format!(
            "tar could not create portable archive {} with {}",
            archive.display(),
            output.status
        ));
    }
    Ok(())
}

fn verify_portable_archive(
    portable_root: &Path,
    package_name: &str,
    archive: &Path,
) -> Result<(), String> {
    let archive_name = archive
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            format!(
                "portable archive path is not valid UTF-8: {}",
                archive.display()
            )
        })?;
    let output = run_sanitized_output("tar", &["-tzf", archive_name], portable_root)?;
    if !output.status.success() {
        return Err(format!(
            "tar could not inspect portable archive {} with {}",
            archive.display(),
            output.status
        ));
    }
    let listing = String::from_utf8_lossy(&output.stdout);
    validate_portable_archive_listing(&listing, package_name)?;

    let extraction_root = portable_root.join(format!("{package_name}-extracted"));
    remove_generated_path(&extraction_root)?;
    fs::create_dir_all(&extraction_root)
        .map_err(|error| format!("could not create archive extraction directory: {error}"))?;
    let extraction_path = extraction_root.to_str().ok_or_else(|| {
        format!(
            "portable archive extraction path is not valid UTF-8: {}",
            extraction_root.display()
        )
    })?;
    let extraction = run_sanitized_output(
        "tar",
        &["-xzf", archive_name, "-C", extraction_path],
        portable_root,
    )?;
    if !extraction.status.success() {
        return Err(format!(
            "tar could not extract portable archive {} with {}",
            archive.display(),
            extraction.status
        ));
    }

    let extracted_package = extraction_root.join(package_name);
    let extracted_manifest = extracted_package.join("MobaRust.sha256");
    let verification = verify_checksum_command(vec![
        extracted_package.display().to_string(),
        extracted_manifest.display().to_string(),
    ]);
    let cleanup = remove_generated_path(&extraction_root);
    match (verification, cleanup) {
        (Err(error), _) => Err(error),
        (Ok(()), Err(error)) => Err(error),
        (Ok(()), Ok(())) => Ok(()),
    }
}

fn validate_portable_archive_listing(listing: &str, package_name: &str) -> Result<(), String> {
    let entries = listing
        .lines()
        .map(|line| line.trim_end_matches('/'))
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    let package_prefix = format!("{package_name}/");
    for entry in &entries {
        if entry.starts_with('/')
            || entry.split('/').any(|component| component == "..")
            || (*entry != package_name && !entry.starts_with(&package_prefix))
        {
            return Err(format!("portable archive contains an unsafe path: {entry}"));
        }
    }

    for required in [
        package_name.to_owned(),
        format!("{package_name}/LICENSE"),
        format!("{package_name}/NOTICE"),
        format!("{package_name}/PORTABLE-UNSIGNED.txt"),
        format!("{package_name}/MobaRust.sha256"),
        format!("{package_name}/MobaRust.app/Contents/MacOS/mobarust"),
        format!("{package_name}/MobaRust.app/Contents/MacOS/portable.flag"),
        format!("{package_name}/MobaRust.app/Contents/Resources/helpers/mobarust-vnc-helper"),
    ] {
        if !entries.iter().any(|entry| *entry == required) {
            return Err(format!(
                "portable archive is missing required entry: {required}"
            ));
        }
    }
    Ok(())
}

fn write_archive_checksum_manifest(
    portable_root: &Path,
    archive: &Path,
    manifest: &Path,
) -> Result<(), String> {
    let contents = archive_checksum_manifest_contents(portable_root, archive)?;
    fs::write(manifest, contents)
        .map_err(|error| format!("could not write archive checksum manifest: {error}"))
}

fn verify_archive_checksum_manifest(
    portable_root: &Path,
    archive: &Path,
    manifest: &Path,
) -> Result<(), String> {
    let expected = archive_checksum_manifest_contents(portable_root, archive)?;
    let actual = fs::read_to_string(manifest)
        .map_err(|error| format!("could not read archive checksum manifest: {error}"))?;
    if actual != expected {
        return Err(format!(
            "archive checksum manifest changed during verification: {}",
            manifest.display()
        ));
    }
    Ok(())
}

fn archive_checksum_manifest_contents(
    portable_root: &Path,
    archive: &Path,
) -> Result<String, String> {
    let relative = archive
        .strip_prefix(portable_root)
        .map_err(|error| format!("portable archive is outside its output directory: {error}"))?
        .to_str()
        .ok_or_else(|| {
            format!(
                "portable archive path is not valid UTF-8: {}",
                archive.display()
            )
        })?
        .replace('\\', "/");
    Ok(format!("{}  {relative}\n", sha256_file(archive)?))
}

fn run_sanitized_output(
    program: &str,
    args: &[&str],
    directory: &Path,
) -> Result<std::process::Output, String> {
    let isolated_home = create_sanitized_test_home()?;
    let mut command = Command::new(program);
    sanitize_process_environment(&mut command);
    apply_isolated_home(&mut command, &isolated_home);
    command.args(args).current_dir(directory);
    let output = command
        .output()
        .map_err(|error| format!("could not start {program}: {error}"));
    let cleanup = fs::remove_dir_all(&isolated_home)
        .map_err(|error| format!("could not remove temporary command home: {error}"));
    cleanup?;
    output
}

fn pre_push_check() -> Result<(), String> {
    let branch = git_output(&["branch", "--show-current"])?;
    if !branch.status.success() || branch.stdout.trim() != "main" {
        return Err(format!(
            "push audit requires branch main, found {:?}",
            branch.stdout.trim()
        ));
    }

    for check in [
        &["diff", "--check"][..],
        &["diff", "--cached", "--check"][..],
    ] {
        let output = git_output(check)?;
        if !output.status.success() {
            return Err(format!(
                "Git whitespace check failed: {}",
                output.stderr.trim()
            ));
        }
    }

    let ignored = git_output(&["check-ignore", "--quiet", "base/"])?;
    if !ignored.status.success() {
        return Err("base/ is not ignored; refusing push audit".into());
    }

    if Path::new(".github/workflows").exists() {
        return Err(".github/workflows exists; refusing push audit".into());
    }

    let tracked = git_output(&["ls-files"])?;
    let untracked = git_output(&["ls-files", "--others", "--exclude-standard"])?;
    for path in tracked.stdout.lines().chain(untracked.stdout.lines()) {
        if suspicious_credential_path(path) {
            return Err(format!(
                "credential-like path is present in the local Git scope: {path}"
            ));
        }
    }

    let private_markers = git_output(&[
        "grep",
        "--cached",
        "-I",
        "-n",
        "-E",
        "BEGIN (RSA |EC |OPENSSH )?PRIVATE KEY",
        "--",
    ])?;
    if private_markers.status.success() {
        return Err("private-key marker found in the Git index; refusing push audit".into());
    }
    if private_markers.status.code() != Some(1) {
        return Err(format!(
            "could not inspect the Git index for private-key markers: {}",
            private_markers.stderr.trim()
        ));
    }

    let ahead = git_output(&["rev-list", "--left-right", "--count", "origin/main...HEAD"])?;
    if !ahead.status.success() {
        return Err(format!(
            "could not inspect the local origin/main comparison: {}",
            ahead.stderr.trim()
        ));
    }
    println!(
        "pre-push audit passed: branch=main, base=ignored, private-key markers=none, commits={}",
        ahead.stdout.trim()
    );
    Ok(())
}

struct GitOutput {
    status: ExitStatus,
    stdout: String,
    stderr: String,
}

fn git_output(args: &[&str]) -> Result<GitOutput, String> {
    let isolated_home = create_sanitized_test_home()?;
    let mut command = Command::new("git");
    sanitize_process_environment(&mut command);
    apply_isolated_home(&mut command, &isolated_home);
    command.args(args);
    let output = command
        .output()
        .map_err(|error| format!("could not start git: {error}"));
    let cleanup = fs::remove_dir_all(&isolated_home)
        .map_err(|error| format!("could not remove temporary Git home: {error}"));
    cleanup?;
    let output = output?;
    Ok(GitOutput {
        status: output.status,
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

fn suspicious_credential_path(path: &str) -> bool {
    let normalized = path.replace('\\', "/").to_ascii_lowercase();
    let file_name = normalized.rsplit('/').next().unwrap_or(&normalized);
    file_name == ".env"
        || file_name.starts_with(".env.")
        || matches!(
            file_name,
            "id_rsa" | "id_ed25519" | "id_ecdsa" | "known_hosts" | "authorized_keys"
        )
        || file_name.ends_with(".pem")
        || file_name.ends_with(".p12")
        || file_name.ends_with(".pfx")
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
        verify_platform_package_layout(&bundle, PackagePlatform::Macos)?;
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

fn verify_checksum_command(arguments: Vec<String>) -> Result<(), String> {
    if arguments.len() != 2 {
        return Err("usage: cargo xtask verify-checksum <artifact-dir> <manifest>".to_owned());
    }

    let artifact_root = PathBuf::from(&arguments[0]);
    let manifest_path = PathBuf::from(&arguments[1]);
    let bundle_root = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    let manifest_metadata = fs::symlink_metadata(&manifest_path).map_err(|error| {
        format!(
            "could not inspect checksum manifest {}: {error}",
            manifest_path.display()
        )
    })?;
    if !manifest_metadata.file_type().is_file() {
        return Err(format!(
            "checksum manifest must be a regular file: {}",
            manifest_path.display()
        ));
    }

    verify_checksum_manifest(bundle_root, &artifact_root, &manifest_path)?;
    println!(
        "verified checksum manifest for {}: {}",
        artifact_root.display(),
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

const APP_BENCHMARK_SAMPLES: usize = 5;
const APP_BENCHMARK_TIMEOUT: Duration = Duration::from_secs(5);

/// Measure the lightweight version path of an already-built MobaRust binary.
/// The path is explicit and repository-bounded; the app exits before Tauri
/// initialization, so this command never opens a window or application data.
fn benchmark_app(arguments: Vec<String>) -> Result<(), String> {
    if arguments.len() != 1 {
        return Err("benchmark-app requires exactly one repository-local app path".to_owned());
    }
    let root = repository_root()?;
    let executable = resolve_benchmark_binary(&root, &arguments[0])?;
    let size = fs::metadata(&executable)
        .map_err(|error| format!("could not inspect benchmark app: {error}"))?
        .len();
    let mut samples = Vec::with_capacity(APP_BENCHMARK_SAMPLES);
    let mut version = None;
    for _ in 0..APP_BENCHMARK_SAMPLES {
        let (elapsed, output) = run_app_version_probe(&executable, &root)?;
        let output = output.trim();
        if !output.starts_with("MobaRust ") {
            return Err("benchmark app did not return the MobaRust version contract".to_owned());
        }
        version.get_or_insert_with(|| output.to_owned());
        samples.push(elapsed);
    }

    let first = samples[0];
    let repeated = samples.iter().skip(1).sum::<Duration>() / (APP_BENCHMARK_SAMPLES as u32 - 1);
    println!(
        "app_startup version={} bytes={} first_run_ms={:.3} repeated_mean_ms={:.3} samples={}",
        version.unwrap_or_else(|| "unknown".to_owned()),
        size,
        first.as_secs_f64() * 1000.0,
        repeated.as_secs_f64() * 1000.0,
        APP_BENCHMARK_SAMPLES,
    );
    println!(
        "app_startup_note=first_run_and_repeated_process_launch_only; no_gui_no_network_no_application_data"
    );
    Ok(())
}

fn resolve_benchmark_binary(root: &Path, supplied: &str) -> Result<PathBuf, String> {
    let canonical_root = root
        .canonicalize()
        .map_err(|error| format!("could not resolve repository root: {error}"))?;
    let path = PathBuf::from(supplied);
    let path = if path.is_absolute() {
        path
    } else {
        canonical_root.join(path)
    };
    let metadata = fs::symlink_metadata(&path)
        .map_err(|error| format!("benchmark app path is unavailable: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err("benchmark app must be a regular file, not a symlink or directory".to_owned());
    }
    let canonical = path
        .canonicalize()
        .map_err(|error| format!("could not resolve benchmark app path: {error}"))?;
    if !canonical.starts_with(&canonical_root) {
        return Err("benchmark app must remain inside the repository".to_owned());
    }
    let name = canonical
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "benchmark app filename is invalid".to_owned())?;
    if !matches!(name, "mobarust" | "mobarust.exe") {
        return Err("benchmark-app accepts only a mobarust executable".to_owned());
    }
    Ok(canonical)
}

fn run_app_version_probe(executable: &Path, root: &Path) -> Result<(Duration, String), String> {
    let isolated_home = create_sanitized_test_home()?;
    let mut command = Command::new(executable);
    sanitize_process_environment(&mut command);
    apply_isolated_home(&mut command, &isolated_home);
    command
        .arg("--version")
        .current_dir(root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let started = Instant::now();
    let mut child = command
        .spawn()
        .map_err(|error| format!("could not start benchmark app: {error}"))?;
    let status = loop {
        match child
            .try_wait()
            .map_err(|error| format!("could not poll benchmark app: {error}"))?
        {
            Some(status) => break status,
            None if started.elapsed() >= APP_BENCHMARK_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = fs::remove_dir_all(&isolated_home);
                return Err("benchmark app --version exceeded the 5 second timeout".to_owned());
            }
            None => std::thread::sleep(Duration::from_millis(5)),
        }
    };
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    if let Some(mut pipe) = child.stdout.take() {
        pipe.read_to_end(&mut stdout)
            .map_err(|error| format!("could not read benchmark app output: {error}"))?;
    }
    if let Some(mut pipe) = child.stderr.take() {
        pipe.read_to_end(&mut stderr)
            .map_err(|error| format!("could not read benchmark app diagnostics: {error}"))?;
    }
    let cleanup = fs::remove_dir_all(&isolated_home)
        .map_err(|error| format!("could not remove benchmark test home: {error}"));
    cleanup?;
    if !status.success() {
        return Err(format!("benchmark app exited with {status}"));
    }
    if !stderr.is_empty() {
        return Err("benchmark app wrote unexpected diagnostics on --version".to_owned());
    }
    let output = String::from_utf8(stdout)
        .map_err(|_| "benchmark app version output was not valid UTF-8".to_owned())?;
    Ok((started.elapsed(), output))
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
    package_layout_check()?;
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
    command.env("CI", "true");
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
    fn explicit_checksum_command_rejects_tampering_and_unsafe_entries() {
        let root = std::env::temp_dir().join(format!(
            "mobarust-xtask-verify-checksum-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock")
                .as_nanos()
        ));
        let artifact = root.join("artifact");
        let manifest = root.join("MobaRust.sha256");
        fs::create_dir_all(&artifact).expect("create test artifact");
        fs::write(artifact.join("app.bin"), b"fixture").expect("write artifact");
        write_checksum_manifest(&root, &artifact, &manifest).expect("write manifest");
        verify_checksum_command(vec![
            artifact.display().to_string(),
            manifest.display().to_string(),
        ])
        .expect("verify explicit artifact");

        fs::write(artifact.join("app.bin"), b"tampered").expect("tamper artifact");
        let error = verify_checksum_command(vec![
            artifact.display().to_string(),
            manifest.display().to_string(),
        ])
        .expect_err("tampering must be rejected");
        assert!(error.contains("checksum manifest changed"));

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(root.join("outside"), artifact.join("escape"))
                .expect("create fixture symlink");
            let error = checksum_manifest_contents(&root, &artifact, &manifest)
                .expect_err("artifact symlink must be rejected");
            assert!(error.contains("unsupported symlink"));
        }

        fs::remove_dir_all(root).expect("remove checksum fixture");
    }

    #[test]
    fn explicit_checksum_command_accepts_manifest_inside_artifact() {
        let root = std::env::temp_dir().join(format!(
            "mobarust-xtask-inner-manifest-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock")
                .as_nanos()
        ));
        let artifact = root.join("package");
        let manifest = artifact.join("MobaRust.sha256");
        fs::create_dir_all(artifact.join("MobaRust.app/Contents/MacOS"))
            .expect("create packaged artifact");
        fs::write(
            artifact.join("MobaRust.app/Contents/MacOS/mobarust"),
            b"fixture executable",
        )
        .expect("write packaged executable");
        write_checksum_manifest(&artifact, &artifact, &manifest).expect("write inner manifest");

        verify_checksum_command(vec![
            artifact.display().to_string(),
            manifest.display().to_string(),
        ])
        .expect("inner manifest should verify relative to its package root");
        fs::remove_dir_all(root).expect("remove inner manifest fixture");
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
    fn benchmark_binary_scope_rejects_non_app_paths_and_symlinks() {
        let root = std::env::temp_dir().join(format!(
            "mobarust-xtask-benchmark-scope-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock")
                .as_nanos()
        ));
        let app = root.join("target/debug/mobarust");
        fs::create_dir_all(app.parent().expect("app parent")).expect("create app directory");
        fs::write(&app, b"fixture").expect("write app fixture");

        assert_eq!(
            resolve_benchmark_binary(&root, "target/debug/mobarust")
                .expect("app fixture should be accepted"),
            app.canonicalize().expect("canonical app")
        );
        let error = resolve_benchmark_binary(&root, "target/debug/not-mobarust")
            .expect_err("only the app executable is accepted");
        assert!(error.contains("unavailable"));

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&app, root.join("target/debug/mobarust.exe"))
                .expect("create app symlink");
            let error = resolve_benchmark_binary(&root, "target/debug/mobarust.exe")
                .expect_err("symlinked app must be rejected");
            assert!(error.contains("symlink"));
        }

        fs::remove_dir_all(root).expect("remove benchmark scope fixture");
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
    fn suspicious_credential_paths_are_rejected_without_matching_documentation() {
        for path in [
            ".env",
            "local/.env.test",
            "keys/id_ed25519",
            "certs/server.pem",
            "backup/archive.p12",
            "windows/cert.pfx",
            "ssh/known_hosts",
            "ssh/authorized_keys",
        ] {
            assert!(suspicious_credential_path(path), "{path}");
        }
        for path in [
            "docs/security/threat-model.md",
            "docs/release/packaging.md",
            "src/credential-vault.rs",
            "fixtures/README.md",
        ] {
            assert!(!suspicious_credential_path(path), "{path}");
        }
    }

    #[test]
    fn platform_package_contract_covers_mac_windows_and_linux_layouts() {
        let root = std::env::temp_dir().join(format!(
            "mobarust-xtask-platform-layout-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock")
                .as_nanos()
        ));

        for platform in [
            PackagePlatform::Macos,
            PackagePlatform::Windows,
            PackagePlatform::Linux,
        ] {
            let package = root.join(platform.label().to_ascii_lowercase());
            fs::create_dir_all(
                package
                    .join(platform.executable())
                    .parent()
                    .expect("runtime parent"),
            )
            .expect("create runtime directory");
            fs::create_dir_all(
                package
                    .join(platform.vnc_helper())
                    .parent()
                    .expect("helper parent"),
            )
            .expect("create helper directory");
            fs::write(package.join(platform.executable()), b"runtime").expect("write runtime");
            fs::write(package.join(platform.vnc_helper()), b"vnc helper")
                .expect("write VNC helper");

            verify_platform_package_layout(&package, platform)
                .expect("complete platform layout should pass");

            fs::write(package.join(platform.rdp_helper()), b"RDP candidate")
                .expect("write forbidden RDP helper");
            let error = verify_platform_package_layout(&package, platform)
                .expect_err("the RDP candidate must remain excluded");
            assert!(error.contains("unshippable RDP helper"));
        }

        fs::remove_dir_all(root).expect("remove platform layout fixture");
    }

    #[test]
    fn package_platform_parser_requires_an_explicit_supported_target() {
        assert_eq!(PackagePlatform::parse("macos"), Ok(PackagePlatform::Macos));
        assert_eq!(
            PackagePlatform::parse("windows"),
            Ok(PackagePlatform::Windows)
        );
        assert_eq!(PackagePlatform::parse("linux"), Ok(PackagePlatform::Linux));
        let error = PackagePlatform::parse("current").expect_err("target must be explicit");
        assert!(error.contains("expected macos, windows, or linux"));
    }

    #[cfg(unix)]
    #[test]
    fn portable_copy_rejects_symlinked_bundle_entries() {
        let root = std::env::temp_dir().join(format!(
            "mobarust-xtask-portable-copy-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock")
                .as_nanos()
        ));
        let source = root.join("source");
        let destination = root.join("destination");
        fs::create_dir_all(&source).expect("create portable source");
        fs::write(source.join("safe.bin"), b"safe").expect("write safe source");
        std::os::unix::fs::symlink(root.join("outside"), source.join("escape"))
            .expect("create source symlink");

        let error = copy_regular_tree(&source, &destination)
            .expect_err("portable copy must reject symlinked entries");
        assert!(error.contains("unsupported symlink"));
        assert!(!destination.join("escape").exists());
        fs::remove_dir_all(root).expect("remove portable copy fixture");
    }

    #[test]
    fn portable_archive_listing_rejects_escape_and_requires_runtime_entries() {
        let package_name = "MobaRust-macos-arm64";
        let valid = format!(
            "{package_name}/\n{package_name}/LICENSE\n{package_name}/NOTICE\n{package_name}/PORTABLE-UNSIGNED.txt\n{package_name}/MobaRust.sha256\n{package_name}/MobaRust.app/Contents/MacOS/mobarust\n{package_name}/MobaRust.app/Contents/MacOS/portable.flag\n{package_name}/MobaRust.app/Contents/Resources/helpers/mobarust-vnc-helper\n"
        );
        validate_portable_archive_listing(&valid, package_name)
            .expect("complete portable listing should pass");

        let error = validate_portable_archive_listing(
            &valid.replace(&format!("{package_name}/LICENSE\n"), ""),
            package_name,
        )
        .expect_err("portable archive must include the project license");
        assert!(error.contains("required entry") && error.contains("/LICENSE"));

        let error = validate_portable_archive_listing(
            &format!("{package_name}/\n{package_name}/../escape"),
            package_name,
        )
        .expect_err("archive traversal must be rejected");
        assert!(error.contains("unsafe path"));
    }

    #[test]
    fn archive_checksum_manifest_detects_tampering() {
        let root = std::env::temp_dir().join(format!(
            "mobarust-xtask-archive-checksum-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock")
                .as_nanos()
        ));
        fs::create_dir_all(&root).expect("create archive checksum fixture");
        let archive = root.join("MobaRust-macos-arm64.tar.gz");
        let manifest = root.join("MobaRust-macos-arm64.tar.gz.sha256");
        fs::write(&archive, b"fixture archive").expect("write archive fixture");
        write_archive_checksum_manifest(&root, &archive, &manifest)
            .expect("write archive manifest");
        verify_archive_checksum_manifest(&root, &archive, &manifest)
            .expect("verify archive manifest");

        fs::write(&archive, b"tampered archive").expect("tamper archive fixture");
        let error = verify_archive_checksum_manifest(&root, &archive, &manifest)
            .expect_err("archive tampering must be rejected");
        assert!(error.contains("changed during verification"));
        fs::remove_dir_all(root).expect("remove archive checksum fixture");
    }
}
