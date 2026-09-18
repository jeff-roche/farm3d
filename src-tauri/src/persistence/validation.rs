use std::collections::BTreeMap;

use serde_json::value::RawValue;

use crate::contracts::command::JsonNumber;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportValidationError {
    pub field_path: String,
    pub kind: ImportValidationErrorKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImportValidationErrorKind {
    CredentialField,
    UnsupportedNumber,
    InvalidJson,
}

pub fn validate_import_json(bytes: &[u8]) -> Result<(), ImportValidationError> {
    let raw =
        serde_json::from_slice::<Box<RawValue>>(bytes).map_err(|_| ImportValidationError {
            field_path: "$".to_string(),
            kind: ImportValidationErrorKind::InvalidJson,
        })?;
    validate_raw(raw.as_ref(), "$")
}

fn validate_raw(raw: &RawValue, path: &str) -> Result<(), ImportValidationError> {
    let text = raw.get();
    match text.as_bytes().first() {
        Some(b'{') => {
            let fields = serde_json::from_str::<BTreeMap<String, Box<RawValue>>>(text)
                .map_err(|_| invalid(path))?;
            for (key, value) in fields {
                let field_path = format!("{path}.{}", escape_path_key(&key));
                let normalized = normalize_key(&key);
                if forbidden_key(&normalized)
                    && !(is_typed_connection_path(path) && key == "credentialRef")
                {
                    return Err(ImportValidationError {
                        field_path,
                        kind: ImportValidationErrorKind::CredentialField,
                    });
                }
                validate_raw(value.as_ref(), &field_path)?;
            }
            Ok(())
        }
        Some(b'[') => {
            let values =
                serde_json::from_str::<Vec<Box<RawValue>>>(text).map_err(|_| invalid(path))?;
            for (index, value) in values.iter().enumerate() {
                validate_raw(value.as_ref(), &format!("{path}[{index}]"))?;
            }
            Ok(())
        }
        Some(b'-' | b'0'..=b'9') => serde_json::from_str::<JsonNumber>(text)
            .map(|_| ())
            .map_err(|_| ImportValidationError {
                field_path: path.to_string(),
                kind: ImportValidationErrorKind::UnsupportedNumber,
            }),
        Some(b'n' | b't' | b'f' | b'"') => Ok(()),
        _ => Err(invalid(path)),
    }
}

fn invalid(path: &str) -> ImportValidationError {
    ImportValidationError {
        field_path: path.to_string(),
        kind: ImportValidationErrorKind::InvalidJson,
    }
}

fn normalize_key(key: &str) -> String {
    key.chars()
        .filter(|character| character.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn is_typed_connection_path(path: &str) -> bool {
    path.strip_prefix("$.printers[")
        .and_then(|rest| rest.split_once(']'))
        .is_some_and(|(index, suffix)| {
            !index.is_empty()
                && index.bytes().all(|byte| byte.is_ascii_digit())
                && suffix == ".connection"
        })
}

fn forbidden_key(normalized: &str) -> bool {
    matches!(
        normalized,
        "password"
            | "authorization"
            | "authheader"
            | "cookie"
            | "setcookie"
            | "credentialref"
            | "credentialreference"
    ) || [
        "password",
        "passwd",
        "secret",
        "token",
        "apikey",
        "privatekey",
        "accesskey",
        "credential",
    ]
    .iter()
    .any(|suffix| normalized.ends_with(suffix))
}

fn escape_path_key(key: &str) -> String {
    if key
        .chars()
        .all(|character| character.is_alphanumeric() || character == '_')
    {
        key.to_string()
    } else {
        format!("[{key:?}]")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation_reports_the_path_without_echoing_an_unsupported_number() {
        let error = validate_import_json(
            br#"{"printers":[{"connection":{"credentialRef":"opaque"},"overrides":{"future":[0.123456789012345678901]}}]}"#,
        )
        .unwrap_err();

        assert_eq!(error.kind, ImportValidationErrorKind::UnsupportedNumber);
        assert_eq!(error.field_path, "$.printers[0].overrides.future[0]");
    }

    #[test]
    fn only_the_typed_connection_credential_reference_key_is_allowed() {
        assert!(validate_import_json(
            br#"{"printers":[{"connection":{"credentialRef":"opaque"}}]}"#
        )
        .is_ok());
        let error =
            validate_import_json(br#"{"printers":[{"overrides":{"credentialRef":"opaque"}}]}"#)
                .unwrap_err();
        assert_eq!(error.kind, ImportValidationErrorKind::CredentialField);
        assert_eq!(error.field_path, "$.printers[0].overrides.credentialRef");
    }
}
