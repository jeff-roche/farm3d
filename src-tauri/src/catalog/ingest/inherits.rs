use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, PartialEq)]
pub struct InheritsError(pub String);

impl std::fmt::Display for InheritsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Resolves a named machine preset within `index` (preset name -> raw JSON
/// object) by walking its `inherits` chain from base to derived, merging
/// fields so a derived preset's own keys win. Returns the fully merged object.
pub fn resolve_machine_preset(
    index: &HashMap<String, Value>,
    name: &str,
) -> Result<Value, InheritsError> {
    resolve_inner(index, name, &mut Vec::new())
}

fn resolve_inner(
    index: &HashMap<String, Value>,
    name: &str,
    seen: &mut Vec<String>,
) -> Result<Value, InheritsError> {
    if seen.iter().any(|s| s == name) {
        return Err(InheritsError(format!(
            "inherits cycle detected at {name:?}: {seen:?}"
        )));
    }
    seen.push(name.to_string());

    let preset = index
        .get(name)
        .ok_or_else(|| InheritsError(format!("unknown preset {name:?} (dangling inherits)")))?;

    let mut merged = match preset.get("inherits").and_then(|v| v.as_str()) {
        Some(parent) => resolve_inner(index, parent, seen)?,
        None => Value::Object(serde_json::Map::new()),
    };

    if let (Value::Object(base), Value::Object(overlay)) = (&mut merged, preset) {
        for (k, v) in overlay {
            base.insert(k.clone(), v.clone());
        }
    }
    Ok(merged)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn index(entries: &[(&str, Value)]) -> HashMap<String, Value> {
        entries
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    #[test]
    fn leaf_with_no_own_fields_inherits_everything() {
        let idx = index(&[
            (
                "base",
                json!({ "printable_height": 250, "gcode_flavor": "klipper" }),
            ),
            ("leaf", json!({ "inherits": "base", "name": "leaf" })),
        ]);
        let resolved = resolve_machine_preset(&idx, "leaf").unwrap();
        assert_eq!(resolved["printable_height"], 250);
        assert_eq!(resolved["gcode_flavor"], "klipper");
        assert_eq!(resolved["name"], "leaf");
    }

    #[test]
    fn three_deep_chain_merges_base_to_derived_with_derived_winning() {
        let idx = index(&[
            (
                "fdm_machine_common",
                json!({ "printable_height": 200, "auxiliary_fan": 0 }),
            ),
            (
                "family_common",
                json!({ "inherits": "fdm_machine_common", "auxiliary_fan": 1 }),
            ),
            (
                "leaf",
                json!({ "inherits": "family_common", "printable_height": 256 }),
            ),
        ]);
        let resolved = resolve_machine_preset(&idx, "leaf").unwrap();
        // Derived (leaf) overrides the common ancestor's printable_height.
        assert_eq!(resolved["printable_height"], 256);
        // Mid-chain override (family_common) wins over the base.
        assert_eq!(resolved["auxiliary_fan"], 1);
    }

    #[test]
    fn dangling_inherits_is_an_error_not_a_panic() {
        let idx = index(&[("leaf", json!({ "inherits": "does_not_exist" }))]);
        let result = resolve_machine_preset(&idx, "leaf");
        assert!(result.is_err());
    }

    #[test]
    fn inherits_cycle_terminates_with_an_error() {
        let idx = index(&[
            ("a", json!({ "inherits": "b" })),
            ("b", json!({ "inherits": "a" })),
        ]);
        let result = resolve_machine_preset(&idx, "a");
        assert!(result.is_err());
    }

    #[test]
    fn unknown_root_name_is_an_error() {
        let idx: HashMap<String, Value> = HashMap::new();
        assert!(resolve_machine_preset(&idx, "nope").is_err());
    }
}
