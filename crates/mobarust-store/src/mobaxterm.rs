//! MobaXterm session migration (partial support).
//!
//! Parses the `[Bookmarks]` / `[Bookmarks_N]` sections used by exported
//! `.mxtsessions` files and by `MobaXterm.ini`. Only SSH bookmarks (type code
//! `109`) are imported, and only the host, port and username fields, which are
//! consistently documented by public converters. Every other field is reported
//! as not imported instead of being guessed. Password and credential sections
//! are never read.

use std::collections::BTreeSet;

use mobarust_core::{AuthMethod, Protocol, SessionId, SessionRecord};
use serde::{Deserialize, Serialize};

pub const MOBAXTERM_SSH_TYPE: &str = "109";
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
        let segments: Vec<&str> = value.split('#').collect();
        if segments.len() < 3 || !segments[0].is_empty() {
            preview
                .skipped
                .push(format!("{key}: unrecognized bookmark format"));
            continue;
        }
        let kind = segments[1].trim();
        if kind != MOBAXTERM_SSH_TYPE {
            other_types.insert(kind.to_owned());
            preview.skipped.push(format!(
                "{key}: session type {kind} is not supported (SSH only)"
            ));
            continue;
        }
        let fields: Vec<&str> = segments[2].split('%').collect();
        let host = fields.get(1).map(|value| value.trim()).unwrap_or_default();
        if host.is_empty() || host.chars().any(char::is_control) {
            preview.skipped.push(format!("{key}: missing remote host"));
            continue;
        }
        let port = match fields.get(2).map(|value| value.trim()) {
            None | Some("") => 22,
            Some(value) => match value.parse::<u16>() {
                Ok(port) if port > 0 => port,
                _ => {
                    preview.skipped.push(format!("{key}: invalid port"));
                    continue;
                }
            },
        };
        let username = fields
            .get(3)
            .map(|value| value.trim())
            .filter(|value| !value.is_empty() && !value.chars().any(char::is_control))
            .map(str::to_owned);
        if !seen.insert((folder.clone(), key.to_owned())) {
            preview
                .skipped
                .push(format!("{folder}/{key}: duplicate bookmark in file"));
            continue;
        }
        let notes = if username.is_none() {
            Some(
                "Imported from MobaXterm without a username; set one before connecting.".to_owned(),
            )
        } else {
            Some("Imported from MobaXterm (host, port and username only).".to_owned())
        };
        let session = SessionRecord {
            id: SessionId::new(),
            name: key.to_owned(),
            protocol: Protocol::Ssh,
            hostname: host.to_owned(),
            port,
            username,
            auth: AuthMethod::Agent,
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
            startup_command: None,
            environment: Vec::new(),
            jump_hosts: Vec::new(),
            jump_host_profiles: Vec::new(),
            notes,
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
            "Only host, port and username are imported. Passwords, private-key paths, gateway/jump hosts, X11, terminal and font settings are not imported; imported sessions use the SSH agent until edited.".into(),
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
            "Unsupported MobaXterm session type codes skipped: {}",
            names.join(", ")
        ));
    }
    preview
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
        assert!(preview.skipped.iter().any(|s| s.contains("type 91")));
        assert!(preview.skipped.iter().any(|s| s.contains("invalid port")));
        assert!(
            preview
                .skipped
                .iter()
                .any(|s| s.contains("missing remote host"))
        );
        assert!(preview.warnings.iter().any(|w| w.contains("Passwords")));
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
                (
                    c.session.name.as_str(),
                    c.session.folder.as_deref(),
                    c.session.hostname.as_str(),
                    c.session.port,
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
                    22
                ),
                (
                    "Facturation €",
                    Some("Clients/Acme/Prod"),
                    "billing.example.test",
                    2222
                ),
                (
                    "Ops’ box",
                    Some("Clients/Acme/Prod"),
                    "ops.example.test",
                    22
                ),
            ]
        );
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
}
