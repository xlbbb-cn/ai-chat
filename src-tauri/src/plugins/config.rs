//! Per-plugin configuration for the plugin manager window.
//!
//! A plugin ships a declarative schema (`config.schema.yaml`) next to its
//! manifest; the manager renders it as a generic form, so a Rust, Node or
//! Python service gets a config page without any frontend code of its own.
//! A plugin that needs a bespoke page instead declares `config_page` in
//! `plugin.json` and the window loads that document — the schema still owns
//! the values, defaults and validation either way.
//!
//! Values live in `config.yaml` in the plugin folder, which keeps a plugin
//! folder self-contained and copyable. Secrets are stored there in clear text
//! (the folder is workspace state, like a skill) but never leave the backend
//! through a form read: `form()` masks them and reports `has_value` instead.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

/// Schema file name inside a plugin folder.
pub const SCHEMA_FILE: &str = "config.schema.yaml";
/// Values file name inside a plugin folder (owned by the manager).
pub const VALUES_FILE: &str = "config.yaml";
/// What a secret field reads back as; a save echoes it to mean "unchanged".
pub const SECRET_MASK: &str = "\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FieldKind {
    #[default]
    Text,
    Multiline,
    Number,
    Integer,
    Boolean,
    /// One of `options[].value`.
    Enum,
    /// A workspace- or plugin-relative path; the form offers a picker.
    Path,
    /// Stored, never echoed back in clear text.
    Secret,
    /// Free-form JSON, edited as text.
    Json,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldOption {
    pub value: String,
    #[serde(default)]
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevealWhen {
    pub key: String,
    pub equals: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Field {
    pub key: String,
    #[serde(default, rename = "type")]
    pub kind: FieldKind,
    /// Label under the input. `label:` reads the same: schemas are written by
    /// hand, and an alias beats a silently empty label.
    #[serde(default, alias = "label")]
    pub title: String,
    /// Hint line under the label; `help:` reads the same.
    #[serde(default, alias = "help")]
    pub description: String,
    #[serde(default)]
    pub placeholder: String,
    #[serde(default)]
    pub unit: String,
    #[serde(default)]
    pub default: Option<Value>,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
    #[serde(default)]
    pub step: Option<f64>,
    #[serde(default)]
    pub max_length: Option<usize>,
    #[serde(default)]
    pub options: Vec<FieldOption>,
    /// Collapsed under "advanced" so the common case stays short.
    #[serde(default)]
    pub advanced: bool,
    #[serde(default)]
    pub reveal_when: Option<RevealWhen>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Section {
    /// Anchor and form id — not user text. `load_schema` derives one from the
    /// title when a hand-written schema leaves it out.
    #[serde(default)]
    pub key: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub fields: Vec<Field>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigSchema {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub sections: Vec<Section>,
}

fn default_version() -> u32 {
    1
}

/// One field as the window renders it: schema plus the current value.
#[derive(Debug, Clone, Serialize)]
pub struct FormField {
    pub key: String,
    #[serde(rename = "type")]
    pub kind: FieldKind,
    pub title: String,
    pub description: String,
    pub placeholder: String,
    pub unit: String,
    pub value: Value,
    /// `Secret` fields carry no value; `true` means one is stored.
    pub has_value: bool,
    pub required: bool,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub step: Option<f64>,
    pub max_length: Option<usize>,
    pub options: Vec<FieldOption>,
    pub advanced: bool,
    pub reveal_when: Option<RevealWhen>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FormSection {
    pub key: String,
    pub title: String,
    pub description: String,
    pub fields: Vec<FormField>,
}

/// Everything the manager window needs to draw one plugin's settings.
#[derive(Debug, Clone, Serialize)]
pub struct ConfigForm {
    pub title: String,
    pub description: String,
    pub sections: Vec<FormSection>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FieldError {
    pub key: String,
    pub message: String,
}

pub fn schema_path(dir: &Path, file: &str) -> PathBuf {
    dir.join(file)
}

pub fn values_path(dir: &Path) -> PathBuf {
    dir.join(VALUES_FILE)
}

/// Read the schema; `Ok(None)` when the plugin has none (no config page).
pub fn load_schema(dir: &Path, file: &str) -> Result<Option<ConfigSchema>, String> {
    let path = schema_path(dir, file);
    if !path.is_file() {
        return Ok(None);
    }
    let raw =
        std::fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let mut schema: ConfigSchema =
        serde_yaml::from_str(&raw).map_err(|e| format!("parse {}: {e}", path.display()))?;
    if schema.title.is_empty() {
        schema.title = "Settings".to_string();
    }
    normalize_section_keys(&mut schema);
    Ok(Some(schema))
}

/// Give every section a unique, non-empty key.
///
/// The key is an anchor rather than plugin content, so a schema that omits it —
/// or repeats one — still loads: the title seeds it, and a title without ASCII
/// letters falls back to the section's position.
fn normalize_section_keys(schema: &mut ConfigSchema) {
    let mut taken: Vec<String> = Vec::new();
    for (index, section) in schema.sections.iter_mut().enumerate() {
        let mut seeded = super::slugify_segment(&section.key);
        if seeded.is_empty() {
            seeded = super::slugify_segment(&section.title);
        }
        if seeded.is_empty() {
            seeded = format!("section_{}", index + 1);
        }
        let mut key = seeded.clone();
        let mut attempt = 2;
        while taken.contains(&key) {
            key = format!("{seeded}_{attempt}");
            attempt += 1;
        }
        taken.push(key.clone());
        section.key = key;
    }
}

/// Read the stored values (missing file = empty).
pub fn load_values(dir: &Path) -> Map<String, Value> {
    let path = values_path(dir);
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return Map::new();
    };
    match serde_yaml::from_str::<Value>(&raw) {
        Ok(Value::Object(map)) => map,
        _ => Map::new(),
    }
}

/// Persist values as a stable, human-diffable YAML mapping.
pub fn write_values(dir: &Path, values: &Map<String, Value>) -> Result<(), String> {
    let path = values_path(dir);
    let text = serde_yaml::to_string(&Value::Object(values.clone()))
        .map_err(|e| format!("serialize values: {e}"))?;
    std::fs::write(&path, text).map_err(|e| format!("write {}: {e}", path.display()))
}

/// Declared fields, in schema order.
pub fn fields(schema: &ConfigSchema) -> Vec<&Field> {
    schema
        .sections
        .iter()
        .flat_map(|s| s.fields.iter())
        .collect()
}

/// Declared keys with their defaults, before stored values are applied.
fn defaults(schema: &ConfigSchema) -> Map<String, Value> {
    let mut out = Map::new();
    for field in fields(schema) {
        if let Some(default) = &field.default {
            out.insert(field.key.clone(), default.clone());
        }
    }
    out
}

/// Defaults overlaid with stored values, restricted to declared keys.
///
/// This is what a plugin receives: it never sees a stale key that the schema
/// no longer declares.
pub fn effective(schema: &ConfigSchema, stored: &Map<String, Value>) -> Map<String, Value> {
    let mut out = defaults(schema);
    for field in fields(schema) {
        if let Some(value) = stored.get(&field.key) {
            out.insert(field.key.clone(), value.clone());
        }
    }
    out
}

/// Coerce and validate form input, returning the values to persist.
///
/// `input` holds raw form values. A `Secret` field set to `null`, absent, or
/// echoing [`SECRET_MASK`] keeps the stored secret; an empty string clears it.
pub fn validate(
    schema: &ConfigSchema,
    stored: &Map<String, Value>,
    input: &Value,
) -> Result<Map<String, Value>, Vec<FieldError>> {
    let Some(input) = input.as_object() else {
        return Err(vec![FieldError {
            key: String::new(),
            message: "expected an object of field values".into(),
        }]);
    };
    let mut out = Map::new();
    let mut errors = Vec::new();
    for field in fields(schema) {
        let raw = input.get(&field.key);
        if field.kind == FieldKind::Secret {
            match raw {
                None | Some(Value::Null) => {
                    if let Some(previous) = stored.get(&field.key) {
                        out.insert(field.key.clone(), previous.clone());
                    }
                    continue;
                }
                Some(Value::String(text)) if text == SECRET_MASK => {
                    if let Some(previous) = stored.get(&field.key) {
                        out.insert(field.key.clone(), previous.clone());
                    }
                    continue;
                }
                Some(Value::String(text)) if text.is_empty() => continue,
                _ => {}
            }
        }
        let Some(raw) = raw.filter(|v| !v.is_null()) else {
            if field.required {
                errors.push(FieldError {
                    key: field.key.clone(),
                    message: "required".into(),
                });
            }
            continue;
        };
        match coerce(field, raw) {
            Ok(value) => {
                out.insert(field.key.clone(), value);
            }
            Err(message) => errors.push(FieldError {
                key: field.key.clone(),
                message,
            }),
        }
    }
    if errors.is_empty() {
        Ok(out)
    } else {
        Err(errors)
    }
}

fn coerce(field: &Field, raw: &Value) -> Result<Value, String> {
    let value = match field.kind {
        FieldKind::Number => {
            let number = match raw {
                Value::Number(n) => n.as_f64(),
                Value::String(s) => s.trim().parse::<f64>().ok(),
                _ => None,
            }
            .ok_or_else(|| "expected a number".to_string())?;
            if !number.is_finite() {
                return Err("expected a finite number".into());
            }
            if let Some(min) = field.min {
                if number < min {
                    return Err(format!("must be at least {min}"));
                }
            }
            if let Some(max) = field.max {
                if number > max {
                    return Err(format!("must be at most {max}"));
                }
            }
            serde_json::json!(number)
        }
        FieldKind::Integer => {
            let number = match raw {
                Value::Number(n) => n.as_f64(),
                Value::String(s) => s.trim().parse::<f64>().ok(),
                _ => None,
            }
            .ok_or_else(|| "expected a whole number".to_string())?;
            if number.fract() != 0.0 {
                return Err("expected a whole number".into());
            }
            if let Some(min) = field.min {
                if number < min {
                    return Err(format!("must be at least {min}"));
                }
            }
            if let Some(max) = field.max {
                if number > max {
                    return Err(format!("must be at most {max}"));
                }
            }
            serde_json::json!(number as i64)
        }
        FieldKind::Boolean => match raw {
            Value::Bool(flag) => Value::Bool(*flag),
            Value::String(text) if text == "true" || text == "1" => Value::Bool(true),
            Value::String(text) if text == "false" || text == "0" => Value::Bool(false),
            _ => return Err("expected true or false".into()),
        },
        FieldKind::Enum => {
            let text = raw.as_str().ok_or_else(|| "expected text".to_string())?;
            if !field.options.is_empty() && !field.options.iter().any(|o| o.value == text) {
                return Err(format!(
                    "not one of: {}",
                    field
                        .options
                        .iter()
                        .map(|o| o.value.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            Value::String(text.to_string())
        }
        FieldKind::Json => match raw {
            Value::String(text) if text.trim().is_empty() => return Ok(Value::Null),
            Value::String(text) => {
                serde_json::from_str::<Value>(text).map_err(|e| format!("invalid JSON: {e}"))?
            }
            other => other.clone(),
        },
        FieldKind::Text | FieldKind::Multiline | FieldKind::Path | FieldKind::Secret => {
            let text = raw.as_str().ok_or_else(|| "expected text".to_string())?;
            let text = text.trim();
            if text.is_empty() {
                return Err("must not be empty".into());
            }
            if let Some(limit) = field.max_length {
                if text.chars().count() > limit {
                    return Err(format!("must be at most {limit} characters"));
                }
            }
            Value::String(text.to_string())
        }
    };
    Ok(value)
}

/// Build the form the window renders. Secrets come back masked.
pub fn form(schema: &ConfigSchema, stored: &Map<String, Value>) -> ConfigForm {
    let values = effective(schema, stored);
    let sections = schema
        .sections
        .iter()
        .map(|section| FormSection {
            key: section.key.clone(),
            title: if section.title.is_empty() {
                section.key.clone()
            } else {
                section.title.clone()
            },
            description: section.description.clone(),
            fields: section
                .fields
                .iter()
                .map(|field| {
                    let value = values.get(&field.key).cloned().unwrap_or(Value::Null);
                    let secret = field.kind == FieldKind::Secret;
                    FormField {
                        key: field.key.clone(),
                        kind: field.kind,
                        title: if field.title.is_empty() {
                            field.key.clone()
                        } else {
                            field.title.clone()
                        },
                        description: field.description.clone(),
                        placeholder: field.placeholder.clone(),
                        unit: field.unit.clone(),
                        has_value: secret && !value.is_null(),
                        value: if secret { Value::Null } else { value },
                        required: field.required,
                        min: field.min,
                        max: field.max,
                        step: field.step,
                        max_length: field.max_length,
                        options: field.options.clone(),
                        advanced: field.advanced,
                        reveal_when: field.reveal_when.clone(),
                    }
                })
                .collect(),
        })
        .collect();
    ConfigForm {
        title: schema.title.clone(),
        description: schema.description.clone(),
        sections,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(yaml: &str) -> ConfigSchema {
        serde_yaml::from_str(yaml).unwrap_or_else(|err| panic!("{err}"))
    }

    #[test]
    fn a_section_without_a_key_gets_one_from_its_title() {
        let mut schema =
            parse("sections:\n  - title: Message home\n  - title: Message home\n  - title: 消息\n");
        normalize_section_keys(&mut schema);
        assert_eq!(schema.sections[0].key, "message_home");
        assert_eq!(schema.sections[1].key, "message_home_2");
        assert_eq!(schema.sections[2].key, "section_3");
    }

    #[test]
    fn a_declared_section_key_is_kept() {
        let mut schema = parse("sections:\n  - key: Advanced\n    title: Advanced\n");
        normalize_section_keys(&mut schema);
        assert_eq!(schema.sections[0].key, "advanced");
    }

    #[test]
    fn label_and_help_read_as_title_and_description() {
        let schema = parse(
            "sections:\n  - key: main\n    fields:\n      - key: prefix\n        label: Prefix\n        help: Printed first.\n",
        );
        let field = &schema.sections[0].fields[0];
        assert_eq!(field.title, "Prefix");
        assert_eq!(field.description, "Printed first.");
    }

    #[test]
    fn a_field_without_a_key_is_still_refused() {
        // A field key is a storage name: guessing one would orphan a saved value.
        let err = serde_yaml::from_str::<ConfigSchema>(
            "sections:\n  - key: main\n    fields:\n      - type: text\n",
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("missing field `key`"), "{err}");
    }
}
