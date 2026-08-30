use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

pub const MAX_MACRO_ACTIONS: usize = 64;
pub const MAX_MACRO_TEXT_BYTES: usize = 64 * 1024;
pub const MAX_MACRO_WAIT_MILLISECONDS: u64 = 300_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MacroRecord {
    pub id: Uuid,
    pub title: String,
    pub description: String,
    pub tags: Vec<String>,
    pub actions: Vec<MacroAction>,
    #[serde(default)]
    pub approval: MacroApprovalPolicy,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MacroApprovalPolicy {
    #[default]
    BeforeRun,
    EachAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum MacroAction {
    SendText { text: String },
    Wait { milliseconds: u64 },
    SendKey { key: MacroKey },
    ExecuteCommand { command: String },
    OpenSession { session_id: Uuid },
    SwitchWorkspace { workspace_id: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MacroKey {
    Enter,
    Escape,
    Tab,
    Backspace,
    CtrlC,
    CtrlD,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum MacroValidationError {
    #[error("macro title cannot be empty")]
    EmptyTitle,
    #[error("macro title is too long")]
    TitleTooLong,
    #[error("macro description is too long")]
    DescriptionTooLong,
    #[error("macro tag cannot be empty")]
    EmptyTag,
    #[error("macro must contain between 1 and 64 actions")]
    InvalidActionCount,
    #[error("macro text is empty")]
    EmptyText,
    #[error("macro text exceeds the 64 KiB limit")]
    TextTooLong,
    #[error("macro text contains a NUL byte")]
    NulByte,
    #[error("macro wait must be between 1 and 300000 milliseconds")]
    InvalidWait,
    #[error("macro workspace reference is invalid")]
    InvalidWorkspace,
}

impl MacroRecord {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            title: title.into(),
            description: String::new(),
            tags: Vec::new(),
            actions: Vec::new(),
            approval: MacroApprovalPolicy::BeforeRun,
        }
    }

    pub fn validate(&self) -> Result<(), MacroValidationError> {
        let title_bytes = self.title.trim().len();
        if title_bytes == 0 {
            return Err(MacroValidationError::EmptyTitle);
        }
        if title_bytes > 200 {
            return Err(MacroValidationError::TitleTooLong);
        }
        if self.description.len() > 4 * 1024 {
            return Err(MacroValidationError::DescriptionTooLong);
        }
        if self.tags.iter().any(|tag| tag.trim().is_empty()) {
            return Err(MacroValidationError::EmptyTag);
        }
        if !(1..=MAX_MACRO_ACTIONS).contains(&self.actions.len()) {
            return Err(MacroValidationError::InvalidActionCount);
        }
        for action in &self.actions {
            match action {
                MacroAction::SendText { text } | MacroAction::ExecuteCommand { command: text } => {
                    validate_text(text)?;
                }
                MacroAction::Wait { milliseconds } => {
                    if !(1..=MAX_MACRO_WAIT_MILLISECONDS).contains(milliseconds) {
                        return Err(MacroValidationError::InvalidWait);
                    }
                }
                MacroAction::SendKey { .. } | MacroAction::OpenSession { .. } => {}
                MacroAction::SwitchWorkspace { workspace_id } => {
                    if workspace_id.trim().is_empty()
                        || workspace_id.len() > 128
                        || workspace_id.chars().any(|character| character.is_control())
                    {
                        return Err(MacroValidationError::InvalidWorkspace);
                    }
                }
            }
        }
        Ok(())
    }

    pub fn requires_elevated_confirmation(&self) -> bool {
        self.actions.iter().any(|action| {
            matches!(
                action,
                MacroAction::ExecuteCommand { .. }
                    | MacroAction::OpenSession { .. }
                    | MacroAction::SwitchWorkspace { .. }
            )
        })
    }
}

fn validate_text(text: &str) -> Result<(), MacroValidationError> {
    if text.is_empty() {
        return Err(MacroValidationError::EmptyText);
    }
    if text.len() > MAX_MACRO_TEXT_BYTES {
        return Err(MacroValidationError::TextTooLong);
    }
    if text.contains('\0') {
        return Err(MacroValidationError::NulByte);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macro_round_trip_preserves_typed_actions() {
        let mut record = MacroRecord::new("Deploy");
        record.actions = vec![
            MacroAction::SendText {
                text: "git pull".into(),
            },
            MacroAction::SendKey {
                key: MacroKey::Enter,
            },
            MacroAction::Wait { milliseconds: 250 },
            MacroAction::ExecuteCommand {
                command: "systemctl status app".into(),
            },
        ];
        record.approval = MacroApprovalPolicy::EachAction;
        record.validate().unwrap();
        let encoded = serde_json::to_string(&record).unwrap();
        let decoded: MacroRecord = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, record);
        assert!(decoded.requires_elevated_confirmation());
    }

    #[test]
    fn legacy_macros_default_to_before_run_approval() {
        let encoded = r#"{"id":"00000000-0000-0000-0000-000000000001","title":"Legacy","description":"","tags":[],"actions":[{"kind":"sendKey","key":"enter"}]}"#;
        let decoded: MacroRecord = serde_json::from_str(encoded).unwrap();
        assert_eq!(decoded.approval, MacroApprovalPolicy::BeforeRun);
    }

    #[test]
    fn macros_reject_unbounded_or_unsafe_payloads() {
        let mut record = MacroRecord::new("Bad");
        record.actions = vec![MacroAction::Wait {
            milliseconds: MAX_MACRO_WAIT_MILLISECONDS + 1,
        }];
        assert_eq!(record.validate(), Err(MacroValidationError::InvalidWait));

        record.actions = vec![MacroAction::SendText { text: "\0".into() }];
        assert_eq!(record.validate(), Err(MacroValidationError::NulByte));

        record.actions = vec![MacroAction::SwitchWorkspace {
            workspace_id: "\n".into(),
        }];
        assert_eq!(
            record.validate(),
            Err(MacroValidationError::InvalidWorkspace)
        );
    }

    #[test]
    fn empty_macros_are_not_runnable() {
        let record = MacroRecord::new("Empty");
        assert_eq!(
            record.validate(),
            Err(MacroValidationError::InvalidActionCount)
        );
    }
}
