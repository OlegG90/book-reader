//! The optional in-tree modules: everything Settings › Modules can turn off.
//!
//! Layer 0 (the core) is never modular: book identity and fingerprints,
//! positions and bookmarks, the library scan and cache, the naming template,
//! the network policy, the settings merge across windows. Layer 1 is listed
//! below; a module turned off disappears from the interface and refuses its
//! actions with a sentence, and the settings remember it in `disabledModules`.

use crate::store::{self, Store};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeSet;
use tauri::State;

/// Which layer-1 group a module belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ModuleKind {
    Format,
    Capability,
}

/// One layer-1 module as the Settings › Modules section shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Module {
    pub id: &'static str,
    pub kind: ModuleKind,
    pub label: &'static str,
    pub hint: &'static str,
    pub extensions: &'static [&'static str],
    pub needs_network: bool,
    pub optional: bool,
    pub enabled: bool,
    pub in_use: bool,
    /// Why the switch is locked, for the settings to show; `None` when free.
    pub in_use_reason: Option<String>,
}

/// One layer-1 module and everything about it that never changes at runtime.
struct Def {
    id: &'static str,
    kind: ModuleKind,
    label: &'static str,
    hint: &'static str,
    extensions: &'static [&'static str],
    needs_network: bool,
    optional: bool,
    /// Modules that must stay on while this one is on: turning one of them
    /// off is refused while this module is still on. Transport-level needs
    /// live in the transports themselves (`sync_cmds.rs`), not here:
    /// `capability:progress-sync` is only the user's intent ("sync my
    /// positions"), so a cloud transport keeps working with catalogs off.
    requires: &'static [&'static str],
}

/// Every layer-1 module. File associations do nothing on Android (no desktop
/// to register with), so there the module cannot be turned off.
fn definitions() -> Vec<Def> {
    vec![
        Def {
            id: "format:epub",
            kind: ModuleKind::Format,
            label: "EPUB",
            hint: "Books in the EPUB format.",
            extensions: &["epub"],
            needs_network: false,
            optional: true,
            requires: &[],
        },
        Def {
            id: "format:fb2",
            kind: ModuleKind::Format,
            label: "FB2",
            hint: "Books as FB2, including .fb2.zip and .fbz archives.",
            extensions: &["fb2.zip", "fbz", "fb2"],
            needs_network: false,
            optional: true,
            requires: &[],
        },
        Def {
            id: "capability:catalogs",
            kind: ModuleKind::Capability,
            label: "Catalogs",
            hint: "Browse OPDS catalogs and download books from them. Can be turned off only when no catalog sources are set up.",
            extensions: &[],
            needs_network: true,
            optional: true,
            requires: &[],
        },
        Def {
            id: "capability:progress-sync",
            kind: ModuleKind::Capability,
            label: "Progress sync",
            hint: "Sync reading positions between your devices, through the catalog server or a cloud store.",
            extensions: &[],
            needs_network: true,
            optional: true,
            requires: &[],
        },
        Def {
            id: "capability:organize",
            kind: ModuleKind::Capability,
            label: "Organize",
            hint: "Rename the library's books to follow the naming pattern, with a preview.",
            extensions: &[],
            needs_network: false,
            optional: true,
            requires: &[],
        },
        Def {
            id: "capability:file-types",
            kind: ModuleKind::Capability,
            label: "File types",
            hint: "Open book files in Book Reader from the file manager.",
            extensions: &[],
            needs_network: false,
            #[cfg(mobile)]
            optional: false,
            #[cfg(not(mobile))]
            optional: true,
            requires: &[],
        },
        Def {
            id: "capability:update-check",
            kind: ModuleKind::Capability,
            label: "Update check",
            hint: "Ask GitHub for the latest release from Settings > About.",
            extensions: &[],
            needs_network: true,
            optional: true,
            requires: &[],
        },
        Def {
            id: "capability:dropbox",
            kind: ModuleKind::Capability,
            label: "Dropbox",
            hint: "Sync reading positions through your Dropbox, without catalogs.",
            extensions: &[],
            needs_network: true,
            optional: true,
            requires: &[],
        },
        Def {
            id: "capability:googledrive",
            kind: ModuleKind::Capability,
            label: "Google Drive",
            hint: "Sync reading positions through your Google Drive, without catalogs.",
            extensions: &[],
            needs_network: true,
            optional: true,
            requires: &[],
        },
    ]
}

/// The ids in the settings' `disabledModules`. A missing, corrupt or partial
/// entry is tolerated on load, like every other setting: only strings count.
pub fn disabled_ids(state: &store::State) -> BTreeSet<String> {
    match state.settings.get("disabledModules") {
        Some(Value::Array(items)) => items.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect(),
        _ => BTreeSet::new(),
    }
}

/// Whether the module is turned off.
pub fn is_off(state: &store::State, id: &str) -> bool {
    disabled_ids(state).contains(id)
}

/// The labels of the enabled modules that require `id`, i.e. that block
/// turning it off while they are still on.
fn required_by(state: &store::State, id: &str) -> Vec<&'static str> {
    let disabled = disabled_ids(state);
    definitions()
        .into_iter()
        .filter(|d| d.id != id && !disabled.contains(d.id) && d.requires.contains(&id))
        .map(|d| d.label)
        .collect()
}

/// Why a module counts as in use (its switch shows the reason): catalogs
/// hold the OPDS sources, and whatever requires the module needs it on.
fn use_reason(state: &store::State, id: &str) -> Option<String> {
    if id == "capability:catalogs" && !state.sources.is_empty() {
        return Some("delete its catalog sources first".into());
    }
    required_by(state, id).first().map(|label| format!("turn off \"{label}\" first"))
}

/// Every layer-1 module with its current switch position.
pub fn list(state: &store::State) -> Vec<Module> {
    let disabled = disabled_ids(state);
    definitions()
        .into_iter()
        .map(|def| {
            let enabled = !disabled.contains(def.id);
            let in_use_reason = use_reason(state, def.id);
            Module {
                id: def.id,
                kind: def.kind,
                label: def.label,
                hint: def.hint,
                extensions: def.extensions,
                needs_network: def.needs_network,
                optional: def.optional,
                enabled,
                in_use: in_use_reason.is_some(),
                in_use_reason,
            }
        })
        .collect()
}

/// Refuses a `disabledModules` value that turns off an unknown, mandatory or
/// in-use module, with a sentence for the reader.
pub fn check_turning_off(state: &store::State, next: &BTreeSet<String>) -> Result<(), String> {
    let defs = definitions();
    for id in next {
        let Some(def) = defs.iter().find(|d| d.id == id) else {
            return Err(format!("Unknown module: \"{id}\"."));
        };
        if !def.optional {
            return Err(format!("\"{}\" cannot be turned off.", def.label));
        }
    }
    if next.contains("capability:catalogs") && !state.sources.is_empty() {
        return Err("\"Catalogs\" is in use: delete its catalog sources first.".into());
    }
    // A module stays usable only while what it requires stays on: turning
    // one of them off is refused while a module that needs it is still on
    // (unless it turns off together with it).
    for id in next {
        let dependent = defs.iter().find(|d| !next.contains(d.id) && d.requires.contains(&id.as_str()));
        if let Some(d) = dependent {
            // Unknown ids were already refused above; the fallback never runs.
            let label: &str = match defs.iter().find(|d| d.id == id.as_str()) {
                Some(d) => d.label,
                None => id.as_str(),
            };
            return Err(format!("\"{label}\" is in use: turn off \"{}\" first.", d.label));
        }
    }
    Ok(())
}

/// The sentence a refused action reports when its module is turned off.
pub fn off_message(label: &str) -> String {
    format!("{label} is turned off in Settings > Modules")
}

/// Refuses with the Modules sentence when the module is turned off.
pub fn require(state: &store::State, id: &str) -> Result<(), String> {
    if is_off(state, id) {
        let label = definitions().into_iter().find(|d| d.id == id).map(|d| d.label).unwrap_or(id);
        return Err(off_message(label));
    }
    Ok(())
}

/// Every layer-1 module with its current switch position.
#[tauri::command]
pub fn list_modules(store: State<Store>) -> Vec<Module> {
    store.read(list)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Map, json};

    fn state_with(settings: Map<String, Value>) -> store::State {
        store::State { settings, ..store::State::default() }
    }

    fn disabled(names: &[&str]) -> Map<String, Value> {
        Map::from_iter([("disabledModules".into(), json!(names))])
    }

    #[test]
    fn list_reflects_settings() {
        let all = list(&state_with(Map::new()));
        assert_eq!(all.len(), 9);
        assert!(all.iter().all(|m| m.enabled));
        // Progress sync alone locks nothing: the intent is no blocker,
        // clouds keep working with catalogs off.
        assert!(!all.iter().find(|m| m.id == "capability:catalogs").unwrap().in_use);
        assert!(!all.iter().find(|m| m.id == "format:epub").unwrap().in_use);
        let some = list(&state_with(disabled(&["format:fb2"])));
        assert!(!some.iter().find(|m| m.id == "format:fb2").unwrap().enabled);
        assert!(some.iter().find(|m| m.id == "format:epub").unwrap().enabled);
    }

    #[test]
    fn unknown_and_corrupt_entries_are_tolerated_on_load() {
        let state = state_with(disabled(&["format:fb2"]));
        assert!(is_off(&state, "format:fb2"));
        assert!(!is_off(&state, "format:epub"));
        // Unknown ids change nothing; corrupt shapes mean nothing is off.
        let state = state_with(disabled(&["format:fb2", "wizardry:fireball"]));
        assert!(is_off(&state, "format:fb2"));
        assert_eq!(list(&state).len(), 9);
        let state = state_with(Map::from_iter([("disabledModules".into(), json!("fb2"))]));
        assert!(!is_off(&state, "format:fb2"));
        let state = state_with(Map::from_iter([("disabledModules".into(), json!([42]))]));
        assert!(!is_off(&state, "format:fb2"));
    }

    #[test]
    fn progress_sync_alone_locks_nothing() {
        // The intent is no blocker: with no sources catalogs turn off
        // freely, and clouds keep working afterwards.
        let all = list(&state_with(Map::new()));
        let catalogs = all.iter().find(|m| m.id == "capability:catalogs").unwrap();
        assert!(!catalogs.in_use);
        assert_eq!(catalogs.in_use_reason, None);
    }

    #[test]
    fn list_serializes_camel_case_for_the_settings() {
        // The settings read these keys; renaming one breaks the Modules section.
        let json = serde_json::to_value(list(&state_with(Map::new()))).unwrap();
        for key in ["id", "kind", "label", "hint", "extensions", "needsNetwork", "optional", "enabled", "inUse", "inUseReason"] {
            assert!(json[0].get(key).is_some(), "missing {key}");
        }
        assert_eq!(json[0]["kind"], "format");
    }

    #[test]
    fn format_modules_match_the_registry() {
        // The Modules section and the readers stay one list: same ids,
        // labels and endings in the same order.
        let defs = definitions();
        let formats: Vec<_> = defs.iter().filter(|d| d.kind == ModuleKind::Format).collect();
        let readers = crate::formats::registry();
        assert_eq!(formats.len(), readers.len());
        for (def, reader) in formats.iter().zip(readers.iter()) {
            assert_eq!(def.id, reader.module());
            assert_eq!(def.label, reader.label());
            let endings: Vec<String> = reader.suffixes().iter().map(|s| s[1..].to_string()).collect();
            assert_eq!(def.extensions, endings.iter().map(String::as_str).collect::<Vec<_>>());
        }
    }

    #[test]
    fn require_refuses_an_off_module_with_its_sentence() {
        let on = state_with(Map::new());
        assert_eq!(require(&on, "capability:catalogs"), Ok(()));
        let off = state_with(disabled(&["capability:catalogs"]));
        assert_eq!(require(&off, "capability:catalogs"), Err("Catalogs is turned off in Settings > Modules".into()));
    }

    #[test]
    fn require_names_each_system_capability() {
        for (id, label) in [
            ("capability:organize", "Organize"),
            ("capability:file-types", "File types"),
            ("capability:update-check", "Update check"),
            ("capability:dropbox", "Dropbox"),
            ("capability:googledrive", "Google Drive"),
        ] {
            let off = state_with(disabled(&[id]));
            assert_eq!(require(&off, id), Err(format!("{label} is turned off in Settings > Modules")));
        }
    }

    #[test]
    fn list_names_a_source_blocked_catalogs() {
        let mut state = state_with(Map::new());
        state.sources.push(crate::catalog::Source {
            id: "1".into(),
            name: "CWA".into(),
            url: "https://books.example/opds".into(),
            user: String::new(),
            sync: false,
        });
        let catalogs = list(&state).into_iter().find(|m| m.id == "capability:catalogs").unwrap();
        assert!(catalogs.in_use);
        assert_eq!(catalogs.in_use_reason.as_deref(), Some("delete its catalog sources first"));
    }

    #[test]
    fn unknown_ids_are_refused_on_write() {
        let state = state_with(Map::new());
        let next = BTreeSet::from(["wizardry:fireball".into()]);
        assert_eq!(check_turning_off(&state, &next), Err("Unknown module: \"wizardry:fireball\".".into()));
    }

    #[test]
    fn catalogs_cannot_turn_off_while_sources_exist() {
        let mut state = state_with(disabled(&[]));
        state.sources.push(crate::catalog::Source {
            id: "s".into(),
            name: "CWA".into(),
            url: "https://books.example/opds".into(),
            user: String::new(),
            sync: false,
        });
        let next = BTreeSet::from(["capability:catalogs".into(), "capability:progress-sync".into()]);
        assert!(check_turning_off(&state, &next).is_err());
    }

    #[test]
    fn catalogs_turn_off_while_progress_sync_is_on() {
        // The intent is no blocker: clouds keep working with catalogs off.
        let state = state_with(Map::new());
        let next = BTreeSet::from(["capability:catalogs".into()]);
        assert_eq!(check_turning_off(&state, &next), Ok(()));
    }

    #[test]
    fn unrelated_modules_turn_off_freely() {
        let state = state_with(Map::new());
        let next = BTreeSet::from(["format:fb2".into(), "capability:update-check".into()]);
        assert_eq!(check_turning_off(&state, &next), Ok(()));
    }

    #[test]
    fn requires_edges_resolve_and_never_point_at_themselves() {
        let ids: Vec<&str> = definitions().iter().map(|d| d.id).collect();
        for def in definitions() {
            for req in def.requires {
                assert!(ids.contains(req), "{} requires unknown {}", def.id, req);
                assert_ne!(def.id, *req, "{} requires itself", def.id);
            }
        }
    }

    #[test]
    fn every_requires_edge_blocks_and_shows() {
        // The edge is data, not code: each `requires` entry refuses the write
        // and locks the switch, with the dependent named in both sentences.
        for def in definitions() {
            for req in def.requires {
                let state = state_with(Map::new());
                let next = BTreeSet::from([req.to_string()]);
                let req_label = definitions().iter().find(|d| d.id == *req).unwrap().label;
                assert_eq!(
                    check_turning_off(&state, &next),
                    Err(format!("\"{req_label}\" is in use: turn off \"{}\" first.", def.label))
                );
                let listed = list(&state).into_iter().find(|m| m.id == *req).unwrap();
                assert!(listed.in_use);
                assert_eq!(listed.in_use_reason.as_deref(), Some(format!("turn off \"{}\" first", def.label).as_str()));
            }
        }
    }
}
