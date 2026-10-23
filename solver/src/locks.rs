use crate::assembly;
use crate::model::{Map, WeaponPayload};
use rustc_hash::{FxHashMap, FxHashSet};

const STALE_LOCKED_SLOT: &str = "锁定槽位已失效，请解除锁定后重试";
const STALE_LOCKED_TUNING: &str = "锁定精校已失效，请解除锁定后重试";
const LOCK_CONFLICT: &str = "当前配装与锁定条件不一致";

/// Splits a locked tuning key into its `slotPath\0partId\0tuneId` fields and
/// applies the site's implicit slot lock: a locked tuning value only exists
/// for a part installed in the current build, so its slot is locked to that
/// part. Returns the parsed triples for validation against the assembly.
pub fn expand_locked_tuning(
    locked_paths: &mut FxHashMap<String, Option<String>>,
    locked_tuning: &FxHashMap<String, f64>,
) -> Result<Vec<(String, String, String)>, String> {
    let mut parsed = Vec::with_capacity(locked_tuning.len());
    for key in locked_tuning.keys() {
        let mut fields = key.split('\0');
        let (path, part_id, tune_id) =
            match (fields.next(), fields.next(), fields.next(), fields.next()) {
                (Some(path), Some(part_id), Some(tune_id), None)
                    if !path.is_empty() && !part_id.is_empty() && !tune_id.is_empty() =>
                {
                    (path, part_id, tune_id)
                }
                _ => return Err(format!("{STALE_LOCKED_TUNING}: {key}")),
            };
        match locked_paths.get(path) {
            Some(Some(locked)) if locked == part_id => {}
            Some(_) => return Err(format!("{STALE_LOCKED_TUNING}: {path}")),
            None => {
                locked_paths.insert(path.to_string(), Some(part_id.to_string()));
            }
        }
        parsed.push((path.to_string(), part_id.to_string(), tune_id.to_string()));
    }
    Ok(parsed)
}

/// Structural check that a locked path names slots and host parts that exist
/// in the payload. Socket claims from other locks can hide a slot from the
/// resolver, so existence is checked on the slot graph instead of on the
/// resolved assembly; the search still reports such contradictions as a build
/// with no solution.
fn locked_path_exists(payload: &WeaponPayload, weapon_id: &str, path: &str) -> bool {
    let root = format!("weapon:{}", assembly::enc(weapon_id));
    let mut segments = path.split('/');
    if segments.next() != Some(root.as_str()) {
        return false;
    }
    let mut host = weapon_id.to_string();
    loop {
        let Some(segment) = segments.next() else {
            return false;
        };
        let Some(rest) = segment.strip_prefix("slot:") else {
            return false;
        };
        let Some((socket, _name)) = rest.split_once(':') else {
            return false;
        };
        let Some(slots) = payload.slots_by_host.get(&host) else {
            return false;
        };
        if !slots.iter().any(|slot| assembly::enc(&slot.id) == socket) {
            return false;
        }
        let Some(part_segment) = segments.next() else {
            return true;
        };
        let Some(encoded_part) = part_segment.strip_prefix("part:") else {
            return false;
        };
        let Some((part_id, _)) = payload
            .parts
            .iter()
            .find(|(id, _)| assembly::enc(id) == encoded_part)
        else {
            return false;
        };
        host = part_id.clone();
    }
}

/// Request-level lock validation matching the site optimizer. The site rejects
/// a lock whose slot is no longer resolvable, a locked part that is also
/// excluded, and a locked tuning that does not belong to the installed part,
/// instead of searching with a silently different build.
pub fn validate_locks(
    payload: &WeaponPayload,
    weapon_id: &str,
    locked_paths: &FxHashMap<String, Option<String>>,
    excluded: &FxHashSet<String>,
    locked_tuning: &[(String, String, String)],
) -> Result<(), String> {
    for (path, expected) in locked_paths {
        if !locked_path_exists(payload, weapon_id, path) {
            return Err(format!("{STALE_LOCKED_SLOT}: {path}"));
        }
        let Some(part_id) = expected else {
            continue;
        };
        if excluded.contains(part_id) {
            return Err(format!(
                "{LOCK_CONFLICT}: locked part {part_id} at {path} is excluded"
            ));
        }
        if !payload.parts.contains_key(part_id) {
            return Err(format!(
                "{LOCK_CONFLICT}: locked part {part_id} at {path} is not in the payload"
            ));
        }
    }
    let selections: Map<String, Option<String>> = locked_paths
        .iter()
        .map(|(path, part)| (path.clone(), part.clone()))
        .collect();
    let assembler = assembly::Assembler::new(payload, weapon_id);
    let assembly = assembler.resolve(&selections);
    for (path, part_id, tune_id) in locked_tuning {
        if assembly::installed_part_id(&assembly, path) != Some(part_id.as_str()) {
            return Err(format!("{STALE_LOCKED_TUNING}: {path}"));
        }
        let known = payload.parts.get(part_id).is_some_and(|part| {
            part.tuning_options
                .iter()
                .any(|tune| tune.tune_id == *tune_id)
        });
        if !known {
            return Err(format!("{STALE_LOCKED_TUNING}: {path}"));
        }
    }
    Ok(())
}
