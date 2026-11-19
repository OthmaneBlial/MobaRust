//! MobaXterm session migration (partial support).
//!
//! Parses the `[Bookmarks]` / `[Bookmarks_N]` sections used by exported
//! `.mxtsessions` files and by `MobaXterm.ini`. Only SSH bookmarks (type code
//! `109`) are imported, and only the host, port and username fields, which are
//! consistently documented by public converters. Every other field is reported
//! as not imported instead of being guessed. Password and credential sections
//! are never read.

use std::collections::BTreeSet;

use mobarust_core::{
    AuthMethod, JumpHostRecord, MAX_SSH_JUMP_HOSTS, Protocol, SessionId, SessionRecord,
};
use serde::{Deserialize, Serialize};

pub const MOBAXTERM_SSH_TYPE: &str = "0";
const MOBAXTERM_SFTP_TYPE: &str = "7";
const ROOT_FOLDER: &str = "MobaXterm import";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobaXtermCandidate {
    pub session: SessionRecord,
    /// An existing saved session that looks like the same target.
    pub duplicate_of: Option<SessionId>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobaXtermPreview {
    pub candidates: Vec<MobaXtermCandidate>,
    /// Human-readable reasons for bookmarks that were not converted.
    pub skipped: Vec<String>,
    /// Fields or sections that exist in the input but are not imported.
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ImportAction {
    Add,
    Replace,
    Skip,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportDecision {
    pub session: SessionRecord,
    pub action: ImportAction,
    pub replace_id: Option<SessionId>,
}

/// Decode file bytes as Windows writes them: UTF-8 (optional BOM), UTF-16
/// with a BOM, otherwise Windows-1252 (MobaXterm's ANSI export encoding).
pub fn decode_mobaxterm_bytes(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return decode_utf16(rest, u16::from_le_bytes);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return decode_utf16(rest, u16::from_be_bytes);
    }
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_owned(),
        Err(_) => bytes.iter().map(|&byte| windows_1252(byte)).collect(),
    }
}

fn decode_utf16(bytes: &[u8], word: fn([u8; 2]) -> u16) -> String {
    let words: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| word([pair[0], pair[1]]))
        .collect();
    String::from_utf16_lossy(&words)
}

fn windows_1252(byte: u8) -> char {
    const HIGH: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8D}', 'Ž',
        '\u{8F}', '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9D}',
        'ž', 'Ÿ',
    ];
    match byte {
        0x80..=0x9F => HIGH[usize::from(byte - 0x80)],
        _ => char::from(byte),
    }
}

/// Parse MobaXterm bookmarks without touching any store.
pub fn parse_mobaxterm_sessions(contents: &str, existing: &[SessionRecord]) -> MobaXtermPreview {
    let mut preview = MobaXtermPreview::default();
    let mut section: Option<String> = None;
    let mut folder = ROOT_FOLDER.to_owned();
    let mut ignored_sections = BTreeSet::new();
    let mut other_types = BTreeSet::new();
    let mut seen = BTreeSet::new();
    let mut found_bookmarks = false;

    for raw_line in contents.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with(';') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            let name = line[1..line.len() - 1].trim().to_owned();
            if is_bookmark_section(&name) {
                found_bookmarks = true;
                section = Some(name);
                folder = ROOT_FOLDER.to_owned();
            } else {
                // Never read credential or other sections; only name them.
                ignored_sections.insert(name);
                section = None;
            }
            continue;
        }
        let Some(section_name) = &section else {
            continue;
        };
        let Some((key, value)) = line.split_once('=') else {
            preview
                .skipped
                .push(format!("[{section_name}] malformed line without '='"));
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        match key.to_ascii_lowercase().as_str() {
            "subrep" => {
                folder = folder_from_subrep(value);
                continue;
            }
            "imgnum" => continue,
            _ => {}
        }
        if key.is_empty() {
            preview
                .skipped
                .push(format!("[{section_name}] bookmark without a name"));
            continue;
        }
        // Layout: <reconnect flag>#<icon>#<type%host%port%user%...>#<terminal>#<start mode>#<comment>#...
        // The icon is user-editable; the session type is the first '%' field.
        let segments: Vec<&str> = value.split('#').collect();
        if segments.len() < 3 {
            preview
                .skipped
                .push(format!("{key}: unrecognized bookmark format"));
            continue;
        }
        let fields: Vec<&str> = segments[2].split('%').map(str::trim).collect();
        let field = |index: usize| fields.get(index).copied().unwrap_or_default();
        // Field indexes for the private key, gateway hosts/ports/users/keys and login command.
        let (key_index, gateway, command_index) = match field(0) {
            MOBAXTERM_SSH_TYPE => (14, Some([8, 9, 10, 15]), Some(7)),
            MOBAXTERM_SFTP_TYPE => (9, None, None),
            kind => {
                let label = session_type_label(kind);
                other_types.insert(label.clone());
                preview.skipped.push(format!(
                    "{key}: {label} sessions are not imported (SSH and SFTP only)"
                ));
                continue;
            }
        };
        let host = field(1);
        if host.is_empty() || host.chars().any(char::is_control) {
            preview.skipped.push(format!("{key}: missing remote host"));
            continue;
        }
        let Some(port) = parse_port(field(2)) else {
            preview.skipped.push(format!("{key}: invalid port"));
            continue;
        };
        let username = clean_user(field(3));
        if !seen.insert((folder.clone(), key.to_owned())) {
            preview
                .skipped
                .push(format!("{folder}/{key}: duplicate bookmark in file"));
            continue;
        }
        let mut notes = vec!["Imported from MobaXterm.".to_owned()];
        if username.is_none() {
            notes.push(
                "No username was saved (MobaXterm default login); set one before connecting."
                    .into(),
            );
        }
        let (auth, key_note) = key_auth(field(key_index));
        notes.extend(key_note);
        let mut jump_host_profiles = Vec::new();
        if let Some([hosts, ports, users, keys]) = gateway {
            let hosts = split_pipe(field(hosts));
            let (ports, users, keys) = (
                split_pipe(field(ports)),
                split_pipe(field(users)),
                split_pipe(field(keys)),
            );
            if hosts.len() > MAX_SSH_JUMP_HOSTS {
                preview
                    .skipped
                    .push(format!("{key}: too many SSH gateways"));
                continue;
            }
            for (index, gateway_host) in hosts.iter().enumerate() {
                let Some(gateway_port) = parse_port(ports.get(index).map_or("", String::as_str))
                else {
                    break;
                };
                let gateway_user = users
                    .get(index)
                    .and_then(|user| clean_user(user))
                    .or_else(|| username.clone());
                let Some(gateway_user) = gateway_user else {
                    notes.push(format!(
                        "Gateway {gateway_host} has no username; add it before connecting."
                    ));
                    break;
                };
                let (gateway_auth, gateway_note) =
                    key_auth(keys.get(index).map_or("", String::as_str));
                notes.extend(gateway_note.map(|note| format!("Gateway {gateway_host}: {note}")));
                jump_host_profiles.push(JumpHostRecord {
                    host: gateway_host.clone(),
                    port: gateway_port,
                    username: gateway_user,
                    auth: gateway_auth,
                    known_hosts_path: None,
                    pinned_fingerprint: None,
                    server_alive_interval: None,
                });
            }
            if jump_host_profiles.len() != hosts.len() {
                notes.push("Some SSH gateways could not be imported; check the jump hosts.".into());
                jump_host_profiles.clear();
            }
        }
        let startup_command = command_index
            .map(|index| unescape(field(index)))
            .filter(|command| !command.trim().is_empty());
        notes.extend(
            segments
                .get(5)
                .map(|value| unescape(value.trim()))
                .filter(|comment| !comment.trim().is_empty()),
        );
        let session = SessionRecord {
            id: SessionId::new(),
            name: key.to_owned(),
            protocol: Protocol::Ssh,
            hostname: host.to_owned(),
            port,
            username,
            auth,
            last_used_at: None,
            known_hosts_path: None,
            pinned_fingerprint: None,
            x11_display: None,
            x11_single_connection: false,
            server_alive_interval: None,
            folder: Some(folder.clone()),
            tags: vec!["imported".into(), "mobaxterm".into()],
            favorite: false,
            startup_directory: None,
            startup_command,
            environment: Vec::new(),
            jump_hosts: jump_host_profiles
                .iter()
                .map(|jump| jump.host.clone())
                .collect(),
            jump_host_profiles,
            notes: Some(notes.join(" ")),
            serial_profile: None,
            telnet_profile: None,
            remote_desktop_profile: None,
        };
        if let Err(error) = session.validate() {
            preview.skipped.push(format!("{key}: {error}"));
            continue;
        }
        let duplicate_of = find_duplicate(existing, &session);
        preview.candidates.push(MobaXtermCandidate {
            session,
            duplicate_of,
        });
    }

    if !found_bookmarks {
        preview
            .warnings
            .push("No [Bookmarks] sections were found; is this a MobaXterm session export?".into());
    }
    if !preview.candidates.is_empty() {
        preview.warnings.push(
            "Imported: name, folder, host, port, username, login command, SSH gateways and private-key file names (mapped to ~/.ssh). Never imported: passwords, X11, proxy, terminal and font settings.".into(),
        );
    }
    if !ignored_sections.is_empty() {
        let names: Vec<_> = ignored_sections.into_iter().collect();
        preview.warnings.push(format!(
            "Ignored non-session sections (never read, including any stored credentials): {}",
            names.join(", ")
        ));
    }
    if !other_types.is_empty() {
        let names: Vec<_> = other_types.into_iter().collect();
        preview.warnings.push(format!(
            "Unsupported MobaXterm session types skipped: {}",
            names.join(", ")
        ));
    }
    preview
}

/// Keep the SSH agent for keys not present in ~/.ssh on this machine, so an
/// import never turns a working agent login into a missing-key failure.
pub fn use_agent_for_missing_keys(
    preview: &mut MobaXtermPreview,
    key_exists: impl Fn(&str) -> bool,
) {
    fn check(auth: &mut AuthMethod, missing: &mut Vec<String>, key_exists: &impl Fn(&str) -> bool) {
        if let AuthMethod::PrivateKey { key_ref, .. } = auth
            && !key_exists(key_ref)
        {
            missing.push(key_ref.clone());
            *auth = AuthMethod::Agent;
        }
    }
    for candidate in &mut preview.candidates {
        let session = &mut candidate.session;
        let mut missing = Vec::new();
        check(&mut session.auth, &mut missing, &key_exists);
        for jump in &mut session.jump_host_profiles {
            check(&mut jump.auth, &mut missing, &key_exists);
        }
        if !missing.is_empty() {
            let notes = session.notes.get_or_insert_with(String::new);
            notes.push_str(&format!(
                " Not found on this computer: {}. Copy the key there and select it; the SSH agent is used until then.",
                missing.join(", ")
            ));
        }
    }
}

fn parse_port(value: &str) -> Option<u16> {
    if value.is_empty() {
        return Some(22);
    }
    value.parse::<u16>().ok().filter(|port| *port > 0)
}

/// "<default>" means MobaXterm's global default login, which is not exported.
fn clean_user(value: &str) -> Option<String> {
    Some(value.trim())
        .filter(|user| {
            !user.is_empty() && *user != "<default>" && !user.chars().any(char::is_control)
        })
        .map(str::to_owned)
}

fn split_pipe(value: &str) -> Vec<String> {
    if value.is_empty() {
        return Vec::new();
    }
    value
        .split("__PIPE__")
        .map(|part| part.trim().to_owned())
        .collect()
}

fn unescape(value: &str) -> String {
    value
        .replace("__PTVIRG__", ";")
        .replace("__DBLQUO__", "\"")
        .replace("__PIPE__", "|")
        .replace("__PERCENT__", "%")
        .replace("__DIEZE__", "#")
}

/// Windows key paths cannot be used elsewhere; point at the same file name in
/// ~/.ssh, where users copy their keys. PuTTY .ppk keys need converting first.
fn key_auth(path: &str) -> (AuthMethod, Option<String>) {
    let file = path.rsplit(['\\', '/']).next().unwrap_or_default().trim();
    if file.is_empty() || file.chars().any(char::is_control) {
        return (AuthMethod::Agent, None);
    }
    if file.to_ascii_lowercase().ends_with(".ppk") {
        return (
            AuthMethod::Agent,
            Some(format!(
                "MobaXterm used the PuTTY key {file}; convert it with `puttygen {file} -O private-openssh -o ~/.ssh/<name>` and select it, or load it in your SSH agent."
            )),
        );
    }
    (
        AuthMethod::PrivateKey {
            key_ref: format!("~/.ssh/{file}"),
            credential_ref: None,
        },
        Some(format!(
            "Key mapped from {} to ~/.ssh/{file}.",
            path.replace("_CurrentDrive_", "C")
        )),
    )
}

fn session_type_label(kind: &str) -> String {
    match kind {
        "4" => "RDP".into(),
        "5" => "VNC".into(),
        "11" => "Browser".into(),
        other => format!("type {other}"),
    }
}

fn is_bookmark_section(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name == "bookmarks"
        || name
            .strip_prefix("bookmarks_")
            .is_some_and(|suffix| !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit()))
}

fn folder_from_subrep(value: &str) -> String {
    let parts: Vec<&str> = value
        .split('\\')
        .map(str::trim)
        .filter(|part| !part.is_empty() && !part.chars().any(char::is_control))
        .collect();
    if parts.is_empty() {
        ROOT_FOLDER.to_owned()
    } else {
        parts.join("/")
    }
}

fn find_duplicate(existing: &[SessionRecord], session: &SessionRecord) -> Option<SessionId> {
    existing
        .iter()
        .find(|item| {
            item.protocol == Protocol::Ssh
                && ((item.hostname.eq_ignore_ascii_case(&session.hostname)
                    && item.port == session.port
                    && item.username == session.username)
                    || (item.name == session.name && item.folder == session.folder))
        })
        .map(|item| item.id)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../tests/fixtures/sample.mxtsessions");

    #[test]
    fn parses_nested_ssh_bookmarks_and_reports_unsupported() {
        let preview = parse_mobaxterm_sessions(FIXTURE, &[]);
        let names: Vec<_> = preview
            .candidates
            .iter()
            .map(|c| (c.session.folder.clone().unwrap(), c.session.name.clone()))
            .collect();
        assert!(names.contains(&("MobaXterm import".into(), "bastion".into())));
        assert!(names.contains(&("Production/Web".into(), "web 01".into())));
        let web = preview
            .candidates
            .iter()
            .find(|c| c.session.name == "web 01")
            .unwrap();
        assert_eq!(web.session.hostname, "web01.example.test");
        assert_eq!(web.session.port, 2222);
        assert_eq!(web.session.username.as_deref(), Some("deploy"));
        assert!(
            preview
                .skipped
                .iter()
                .any(|s| s.contains("RDP sessions are not imported"))
        );
        assert!(preview.skipped.iter().any(|s| s.contains("invalid port")));
        assert!(
            preview
                .skipped
                .iter()
                .any(|s| s.contains("missing remote host"))
        );
        assert!(preview.warnings.iter().any(|w| w.contains("passwords")));
        // The credential section is named but never parsed into anything.
        let serialized = serde_json::to_string(&preview).unwrap();
        assert!(!serialized.contains("SECRET-SHOULD-NOT-LEAK"));
    }

    #[test]
    fn detects_duplicates_against_existing_sessions() {
        let first = parse_mobaxterm_sessions(FIXTURE, &[]);
        let existing: Vec<_> = first.candidates.iter().map(|c| c.session.clone()).collect();
        let second = parse_mobaxterm_sessions(FIXTURE, &existing);
        assert!(second.candidates.iter().all(|c| c.duplicate_of.is_some()));
    }

    #[test]
    fn handles_garbage_and_latin1_without_panicking() {
        let preview =
            parse_mobaxterm_sessions("\u{0}\n[Bookmarks]\nnoequals\n=#109#0%h%22%u\n", &[]);
        assert!(preview.candidates.is_empty());
        assert_eq!(preview.skipped.len(), 2);
        let text = decode_mobaxterm_bytes(b"[Bookmarks]\ncaf\xe9=#109#0%h%22%u\n");
        let preview = parse_mobaxterm_sessions(&text, &[]);
        assert_eq!(preview.candidates[0].session.name, "café");
    }

    const WINDOWS_FIXTURE: &[u8] = include_bytes!("../tests/fixtures/windows-cp1252.mxtsessions");

    fn assert_windows_export(preview: &MobaXtermPreview) {
        let rows: Vec<_> = preview
            .candidates
            .iter()
            .map(|c| {
                let s = &c.session;
                (
                    s.name.as_str(),
                    s.folder.as_deref(),
                    s.hostname.as_str(),
                    s.port,
                    s.username.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            rows,
            [
                (
                    "Serveur café",
                    Some("MobaXterm import"),
                    "cafe.example.test",
                    22,
                    Some("opé")
                ),
                (
                    "Facturation €",
                    Some("Clients/Acme/Prod"),
                    "billing.example.test",
                    2222,
                    Some("deploy")
                ),
                (
                    "Ops’ box",
                    Some("Clients/Acme/Prod"),
                    "ops.example.test",
                    22,
                    None
                ),
                (
                    "Files",
                    Some("Clients/Acme/Prod"),
                    "files.example.test",
                    2022,
                    Some("sftpuser")
                ),
            ]
        );
        let cafe = &preview.candidates[0].session;
        assert_eq!(
            cafe.auth,
            AuthMethod::PrivateKey {
                key_ref: "~/.ssh/id_ed25519".into(),
                credential_ref: None
            }
        );
        assert_eq!(cafe.startup_command.as_deref(), Some("cd /var/log; ls"));
        assert!(cafe.notes.as_deref().unwrap().contains("Rack #4"));
        let billing = &preview.candidates[1].session;
        assert_eq!(billing.auth, AuthMethod::Agent);
        assert!(
            billing
                .notes
                .as_deref()
                .unwrap()
                .contains("puttygen acme.ppk")
        );
        assert_eq!(billing.jump_hosts, ["bastion.example.test"]);
        let jump = &billing.jump_host_profiles[0];
        assert_eq!(
            (jump.host.as_str(), jump.port, jump.username.as_str()),
            ("bastion.example.test", 2200, "jump")
        );
        assert_eq!(
            jump.auth,
            AuthMethod::PrivateKey {
                key_ref: "~/.ssh/jump_ed25519".into(),
                credential_ref: None
            }
        );
        assert!(preview.skipped.iter().any(|s| s.starts_with("Desk: VNC")));
        assert!(preview.skipped.iter().any(|s| s.starts_with("Win: RDP")));
        assert!(!format!("{preview:?}").contains("SECRET-SHOULD-NOT-LEAK"));
    }

    #[test]
    fn imports_windows_cp1252_crlf_export() {
        let text = decode_mobaxterm_bytes(WINDOWS_FIXTURE);
        assert_windows_export(&parse_mobaxterm_sessions(&text, &[]));
    }

    #[test]
    fn imports_utf16_exports_with_bom() {
        let text = decode_mobaxterm_bytes(WINDOWS_FIXTURE);
        let mut le = vec![0xFF, 0xFE];
        let mut be = vec![0xFE, 0xFF];
        for unit in text.encode_utf16() {
            le.extend(unit.to_le_bytes());
            be.extend(unit.to_be_bytes());
        }
        for bytes in [le, be] {
            assert_windows_export(&parse_mobaxterm_sessions(
                &decode_mobaxterm_bytes(&bytes),
                &[],
            ));
        }
    }

    #[test]
    fn missing_mapped_keys_fall_back_to_agent() {
        let text = decode_mobaxterm_bytes(WINDOWS_FIXTURE);
        let mut preview = parse_mobaxterm_sessions(&text, &[]);
        use_agent_for_missing_keys(&mut preview, |key| key == "~/.ssh/jump_ed25519");
        let cafe = &preview.candidates[0].session;
        assert_eq!(cafe.auth, AuthMethod::Agent);
        assert!(
            cafe.notes
                .as_deref()
                .unwrap()
                .contains("Not found on this computer: ~/.ssh/id_ed25519")
        );
        assert!(matches!(
            preview.candidates[1].session.jump_host_profiles[0].auth,
            AuthMethod::PrivateKey { .. }
        ));
    }
}
