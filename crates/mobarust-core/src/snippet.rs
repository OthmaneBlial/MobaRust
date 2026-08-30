use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnippetRecord {
    pub id: Uuid,
    pub title: String,
    pub description: String,
    pub command: String,
    pub tags: Vec<String>,
    pub variables: Vec<String>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SnippetValidationError {
    #[error("snippet title cannot be empty")]
    EmptyTitle,
    #[error("snippet command cannot be empty")]
    EmptyCommand,
    #[error("snippet tag cannot be empty")]
    EmptyTag,
    #[error("snippet variable is invalid")]
    InvalidVariable,
    #[error("snippet variables must be unique")]
    DuplicateVariable,
}

impl SnippetRecord {
    pub fn new(title: impl Into<String>, command: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            title: title.into(),
            description: String::new(),
            command: command.into(),
            tags: Vec::new(),
            variables: Vec::new(),
        }
    }

    pub fn validate(&self) -> Result<(), SnippetValidationError> {
        if self.title.trim().is_empty() {
            return Err(SnippetValidationError::EmptyTitle);
        }
        if self.command.trim().is_empty() {
            return Err(SnippetValidationError::EmptyCommand);
        }
        if self.tags.iter().any(|tag| tag.trim().is_empty()) {
            return Err(SnippetValidationError::EmptyTag);
        }
        for (index, variable) in self.variables.iter().enumerate() {
            if variable.is_empty()
                || !variable.chars().enumerate().all(|(position, character)| {
                    character == '_'
                        || character.is_ascii_alphanumeric()
                            && (position > 0 || character.is_ascii_alphabetic())
                })
            {
                return Err(SnippetValidationError::InvalidVariable);
            }
            if self.variables[..index].contains(variable) {
                return Err(SnippetValidationError::DuplicateVariable);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippets_validate_metadata_and_variable_names() {
        let mut snippet = SnippetRecord::new("Docker logs", "docker logs ${container}");
        snippet.variables = vec!["container".into(), "_namespace".into()];
        snippet.validate().unwrap();

        snippet.variables.push("bad-name".into());
        assert_eq!(
            snippet.validate(),
            Err(SnippetValidationError::InvalidVariable)
        );
    }

    #[test]
    fn snippets_reject_duplicate_variables() {
        let mut snippet = SnippetRecord::new("Inspect", "echo ${host}");
        snippet.variables = vec!["host".into(), "host".into()];
        assert_eq!(
            snippet.validate(),
            Err(SnippetValidationError::DuplicateVariable)
        );
    }
}
