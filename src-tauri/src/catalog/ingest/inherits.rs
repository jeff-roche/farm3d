//! Resolves an OrcaSlicer preset's `inherits` chain into one merged object.
//!
//! The resolver is kind-agnostic: it works for machine, process, and
//! filament presets alike, over any [`PresetLookup`]. The catalog generator
//! uses it with an in-memory map of machine presets; P5's preset index
//! (`slicing::presets`) uses it with presets loaded lazily from disk.

use serde_json::Value;
use std::collections::HashMap;

/// P5 D3: `inherits` is followed at most this many times. A longer chain is
/// treated like a cycle.
pub const MAX_INHERITS_DEPTH: usize = 20;

/// Why a chain could not be resolved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InheritsErrorKind {
    /// The named preset, or a parent it inherits from, does not exist.
    NotFound,
    /// The chain loops back on itself.
    Cycle,
    /// The chain is longer than [`MAX_INHERITS_DEPTH`].
    TooDeep,
    /// A preset in the chain exists but could not be read or parsed.
    Unreadable,
}

#[derive(Clone, Debug, PartialEq)]
pub struct InheritsError {
    pub kind: InheritsErrorKind,
    /// The preset where resolution stopped.
    pub preset: String,
    pub message: String,
}

impl InheritsError {
    pub fn new(kind: InheritsErrorKind, preset: &str, message: String) -> Self {
        Self {
            kind,
            preset: preset.to_string(),
            message,
        }
    }
}

impl std::fmt::Display for InheritsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

/// Where the resolver finds a preset's raw JSON object by name.
///
/// A lookup can be scoped: it returns where it found each preset, and the
/// resolver looks that preset's parent up from there. P5's index uses the
/// scope to resolve a parent inside the child's own vendor bundle first,
/// as OrcaSlicer does, because vendors reuse base names such as
/// `fdm_process_common`.
pub trait PresetLookup {
    /// Where a preset was found.
    type Scope;

    /// The preset `name` and its scope, or `Ok(None)` when no preset has
    /// this name. `from` is the scope of the preset that inherits `name`,
    /// or `None` for the preset being resolved.
    fn lookup(
        &self,
        name: &str,
        from: Option<&Self::Scope>,
    ) -> Result<Option<(Value, Self::Scope)>, InheritsError>;
}

/// One flat namespace: the catalog generator indexes one vendor's machine
/// presets at a time.
impl PresetLookup for HashMap<String, Value> {
    type Scope = ();

    fn lookup(&self, name: &str, _from: Option<&()>) -> Result<Option<(Value, ())>, InheritsError> {
        Ok(self.get(name).cloned().map(|preset| (preset, ())))
    }
}

/// Resolves the preset `name` by walking its `inherits` chain from base to
/// derived, merging fields so a derived preset's own keys win. Returns the
/// fully merged object; the `inherits` key of the most derived preset that
/// has one is kept, and callers that need a flat preset remove it.
pub fn resolve_preset<L: PresetLookup>(presets: &L, name: &str) -> Result<Value, InheritsError> {
    resolve_inner(presets, name, None, &mut Vec::new())
}

fn resolve_inner<L: PresetLookup>(
    presets: &L,
    name: &str,
    from: Option<&L::Scope>,
    seen: &mut Vec<String>,
) -> Result<Value, InheritsError> {
    if seen.iter().any(|s| s == name) {
        return Err(InheritsError::new(
            InheritsErrorKind::Cycle,
            name,
            format!("inherits cycle detected at {name:?}: {seen:?}"),
        ));
    }
    if seen.len() > MAX_INHERITS_DEPTH {
        return Err(InheritsError::new(
            InheritsErrorKind::TooDeep,
            name,
            format!("inherits chain deeper than {MAX_INHERITS_DEPTH} at {name:?}"),
        ));
    }
    seen.push(name.to_string());

    let (preset, scope) = presets.lookup(name, from)?.ok_or_else(|| {
        InheritsError::new(
            InheritsErrorKind::NotFound,
            name,
            format!("unknown preset {name:?} (dangling inherits)"),
        )
    })?;

    let mut merged = match preset.get("inherits").and_then(|v| v.as_str()) {
        Some(parent) => resolve_inner(presets, parent, Some(&scope), seen)?,
        None => Value::Object(serde_json::Map::new()),
    };

    if let (Value::Object(base), Value::Object(overlay)) = (&mut merged, preset) {
        for (k, v) in overlay {
            base.insert(k, v);
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
        let resolved = resolve_preset(&idx, "leaf").unwrap();
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
        let resolved = resolve_preset(&idx, "leaf").unwrap();
        // Derived (leaf) overrides the common ancestor's printable_height.
        assert_eq!(resolved["printable_height"], 256);
        // Mid-chain override (family_common) wins over the base.
        assert_eq!(resolved["auxiliary_fan"], 1);
    }

    #[test]
    fn dangling_inherits_is_an_error_not_a_panic() {
        let idx = index(&[("leaf", json!({ "inherits": "does_not_exist" }))]);
        let result = resolve_preset(&idx, "leaf");
        assert!(result.is_err());
    }

    #[test]
    fn inherits_cycle_terminates_with_an_error() {
        let idx = index(&[
            ("a", json!({ "inherits": "b" })),
            ("b", json!({ "inherits": "a" })),
        ]);
        let result = resolve_preset(&idx, "a");
        assert!(result.is_err());
    }

    #[test]
    fn unknown_root_name_is_an_error() {
        let idx: HashMap<String, Value> = HashMap::new();
        assert!(resolve_preset(&idx, "nope").is_err());
    }

    #[test]
    fn errors_name_their_kind() {
        let dangling = index(&[("leaf", json!({ "inherits": "gone" }))]);
        let error = resolve_preset(&dangling, "leaf").unwrap_err();
        assert_eq!(error.kind, InheritsErrorKind::NotFound);
        assert_eq!(error.preset, "gone");

        let cycle = index(&[
            ("a", json!({ "inherits": "b" })),
            ("b", json!({ "inherits": "a" })),
        ]);
        assert_eq!(
            resolve_preset(&cycle, "a").unwrap_err().kind,
            InheritsErrorKind::Cycle
        );
    }

    /// A chain of `n` presets where `p{i}` inherits `p{i-1}`.
    fn chain(n: usize) -> HashMap<String, Value> {
        (0..n)
            .map(|i| {
                let preset = if i == 0 {
                    json!({ "name": "p0", "depth": 0 })
                } else {
                    json!({ "name": format!("p{i}"), "inherits": format!("p{}", i - 1), "depth": i })
                };
                (format!("p{i}"), preset)
            })
            .collect()
    }

    #[test]
    fn follows_inherits_up_to_the_depth_limit() {
        let deepest = MAX_INHERITS_DEPTH;
        let idx = chain(deepest + 1);
        let resolved = resolve_preset(&idx, &format!("p{deepest}")).unwrap();
        assert_eq!(resolved["depth"], deepest);

        let too_deep = chain(deepest + 2);
        let error = resolve_preset(&too_deep, &format!("p{}", deepest + 1)).unwrap_err();
        assert_eq!(error.kind, InheritsErrorKind::TooDeep);
    }

    /// A lookup whose scope is the bundle a preset came from: a parent is
    /// looked up in its child's bundle first.
    struct Bundles(Vec<HashMap<String, Value>>);

    impl PresetLookup for Bundles {
        type Scope = usize;

        fn lookup(
            &self,
            name: &str,
            from: Option<&usize>,
        ) -> Result<Option<(Value, usize)>, InheritsError> {
            let order = from.into_iter().copied().chain(0..self.0.len());
            Ok(order
                .filter_map(|bundle| {
                    self.0[bundle]
                        .get(name)
                        .map(|preset| (preset.clone(), bundle))
                })
                .next())
        }
    }

    #[test]
    fn a_scoped_lookup_resolves_parents_from_the_child_scope() {
        let bundles = Bundles(vec![
            index(&[("common", json!({ "from_bundle": 0 }))]),
            index(&[
                ("common", json!({ "from_bundle": 1 })),
                ("leaf", json!({ "inherits": "common" })),
            ]),
        ]);
        assert_eq!(resolve_preset(&bundles, "leaf").unwrap()["from_bundle"], 1);
        assert_eq!(
            resolve_preset(&bundles, "common").unwrap()["from_bundle"],
            0
        );
    }

    #[test]
    fn works_for_any_preset_kind() {
        let idx = index(&[
            (
                "fdm_process_common",
                json!({ "type": "process", "wall_loops": "2", "layer_height": "0.2" }),
            ),
            (
                "0.20mm Standard",
                json!({ "type": "process", "inherits": "fdm_process_common", "wall_loops": "3" }),
            ),
        ]);
        let resolved = resolve_preset(&idx, "0.20mm Standard").unwrap();
        assert_eq!(resolved["wall_loops"], "3");
        assert_eq!(resolved["layer_height"], "0.2");
    }
}
