use crate::model::{Map, Part, PresetNode, SlotDef, VirtualRule, WeaponPayload};
use rustc_hash::{FxHashMap, FxHashSet};
use serde_json::Value;
use std::collections::BTreeMap;
use std::rc::Rc;

#[derive(Debug, Clone)]
pub struct SlotView {
    pub id: String,
    pub name: String,
    pub virtual_candidate_ids: Vec<String>,
}

impl SlotView {
    fn from_slot(slot: &SlotDef) -> Self {
        Self {
            id: slot.id.clone(),
            name: slot.name.clone(),
            virtual_candidate_ids: slot.virtual_candidate_ids.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct VisibleSlot {
    pub path_key: String,
    pub parent_slot_path_key: Option<String>,
    pub host_part_id: String,
    pub depth: usize,
    pub slot: Rc<SlotView>,
    pub candidate_ids: Rc<Vec<String>>,
    pub selected_part_id: Option<String>,
    pub internal_part_id: Option<String>,
    pub internal_is_default: bool,
}

#[derive(Debug, Clone)]
pub struct InternalPart {
    pub slot_path_key: String,
    pub parent_slot_path_key: Option<String>,
    pub host_part_id: String,
    pub depth: usize,
    pub slot: Rc<SlotView>,
    pub part_id: String,
    pub source: String,
}

#[derive(Debug, Clone, Default)]
pub struct Assembly {
    pub visible_slots: Vec<VisibleSlot>,
    pub internal_parts: Vec<InternalPart>,
    pub active_part_ids: FxHashSet<String>,
    pub non_virtual_claimed_socket_ids: FxHashSet<String>,
}

impl Assembly {
    pub fn installed_part_ids(&self) -> Vec<String> {
        let mut out = Vec::new();
        for slot in &self.visible_slots {
            if let Some(id) = &slot.selected_part_id {
                out.push(id.clone());
            }
        }
        for part in &self.internal_parts {
            out.push(part.part_id.clone());
        }
        out
    }

    pub fn all_part_ids(&self) -> FxHashSet<String> {
        self.installed_part_ids().into_iter().collect()
    }
}

#[derive(Debug, Clone)]
struct DefaultNode {
    part_id: String,
    tune_pairs: Vec<(String, f64)>,
}

#[derive(Debug, Clone)]
struct Claim {
    socket_id: String,
    scoped_key: Option<String>,
    owner_path_key: String,
}

struct SlotData<'a> {
    suffix: String,
    view: Rc<SlotView>,
    candidates: Rc<Vec<&'a Part>>,
    candidate_ids: Rc<Vec<String>>,
}

pub struct Assembler<'a> {
    payload: &'a WeaponPayload,
    weapon_id: &'a str,
    defaults: Map<String, DefaultNode>,
    virtual_groups: Vec<(String, Vec<&'a VirtualRule>)>,
    hide_overrides: Vec<&'a crate::model::CandidateOverride>,
    excluded: FxHashSet<String>,
    slot_data: FxHashMap<usize, Rc<SlotData<'a>>>,
}

impl<'a> Assembler<'a> {
    pub fn new(payload: &'a WeaponPayload, weapon_id: &'a str) -> Self {
        let mut defaults = Map::default();
        if let Some(preset) = payload.default_presets.get(weapon_id) {
            for node in &preset.nodes {
                if node.is_root || node.is_virtual {
                    continue;
                }
                if let Some(path) = &node.path_key {
                    if payload.parts.contains_key(&node.part_id) {
                        defaults.insert(
                            path.clone(),
                            DefaultNode {
                                part_id: node.part_id.clone(),
                                tune_pairs: node
                                    .tune_pairs
                                    .iter()
                                    .map(|t| (t.tune_id.clone(), t.tune_value))
                                    .collect(),
                            },
                        );
                    }
                }
            }
        }

        let mut groups: BTreeMap<String, Vec<&VirtualRule>> = BTreeMap::new();
        if let Some(rules) = payload.virtual_rules.get(weapon_id) {
            for rule in rules {
                groups.entry(rule.socket_id.clone()).or_default().push(rule);
            }
        }
        let mut virtual_groups: Vec<(String, Vec<&VirtualRule>)> = groups.into_iter().collect();
        virtual_groups.sort_by(|a, b| {
            let na = a.0.parse::<f64>().unwrap_or(f64::INFINITY);
            let nb = b.0.parse::<f64>().unwrap_or(f64::INFINITY);
            na.partial_cmp(&nb).unwrap_or(std::cmp::Ordering::Equal)
        });

        let mut hide_overrides = Vec::new();
        if let Some(list) = payload.candidate_overrides.get("*") {
            hide_overrides.extend(list.iter());
        }
        if let Some(list) = payload.candidate_overrides.get(weapon_id) {
            hide_overrides.extend(list.iter());
        }

        let mut out = Self {
            payload,
            weapon_id,
            defaults,
            virtual_groups,
            hide_overrides,
            excluded: FxHashSet::default(),
            slot_data: FxHashMap::default(),
        };
        out.prepare_slots();
        out
    }

    fn prepare_slots(&mut self) {
        let mut cache: FxHashMap<usize, Rc<SlotData<'a>>> = FxHashMap::default();
        for (host, slots) in &self.payload.slots_by_host {
            for slot in slots {
                let candidates = self.candidates(host, slot);
                let candidate_ids: Vec<String> = candidates.iter().map(|p| p.id.clone()).collect();
                let mut suffix = String::with_capacity(8 + slot.id.len() + slot.name.len());
                suffix.push_str("/slot:");
                enc_into(&mut suffix, &slot.id);
                suffix.push(':');
                enc_into(&mut suffix, &slot.name);
                cache.insert(
                    slot as *const SlotDef as usize,
                    Rc::new(SlotData {
                        suffix,
                        view: Rc::new(SlotView::from_slot(slot)),
                        candidate_ids: Rc::new(candidate_ids),
                        candidates: Rc::new(candidates),
                    }),
                );
            }
        }
        self.slot_data = cache;
    }

    pub fn set_excluded(&mut self, excluded: FxHashSet<String>) {
        if excluded.is_empty() {
            return;
        }
        self.defaults
            .retain(|_, node| !excluded.contains(&node.part_id));
        self.excluded = excluded;
        self.prepare_slots();
    }

    pub fn default_tune(&self, path: &str, part_id: &str, tune_id: &str) -> Option<f64> {
        self.defaults.get(path).and_then(|d| {
            if d.part_id != part_id {
                return None;
            }
            d.tune_pairs
                .iter()
                .find(|(id, _)| id == tune_id)
                .map(|(_, value)| *value)
        })
    }

    pub fn default_node_part(&self, path: &str) -> Option<&str> {
        self.defaults.get(path).map(|d| d.part_id.as_str())
    }

    pub fn resolve(&self, selections: &Map<String, Option<String>>) -> Assembly {
        let (mut active, claimed) = self.initial_active(selections);
        let mut virtual_sel = self.select_virtual(&active, &claimed);
        let mut tree = self.build_tree(selections, &virtual_sel);

        for _ in 0..64 {
            active = tree
                .active_part_ids
                .iter()
                .filter(|id| {
                    self.payload
                        .parts
                        .get(*id)
                        .map(|p| !p.is_virtual)
                        .unwrap_or(false)
                })
                .cloned()
                .collect();
            let next = self.select_virtual(&active, &tree.non_virtual_claimed_socket_ids);
            if same_virtual(&virtual_sel, &next) {
                return tree;
            }
            virtual_sel = next;
            tree = self.build_tree(selections, &virtual_sel);
        }
        tree
    }

    /// True when the selection names the default part for `path`. The site
    /// normalizes those before building the tree as well (`Wi`/`qt` run ahead
    /// of `yn`/`Oe`), so an explicit default is treated exactly like an unset
    /// slot: the part is installed internally and stays out of the visible
    /// panel. Checking at the point of use avoids copying the whole selection
    /// map on every resolve.
    fn selection_is_default(&self, path: &str, part_id: &str) -> bool {
        self.defaults
            .get(path)
            .map(|d| d.part_id == part_id)
            .unwrap_or(false)
    }

    fn initial_active(
        &self,
        selections: &Map<String, Option<String>>,
    ) -> (FxHashSet<String>, FxHashSet<String>) {
        let mut active = FxHashSet::default();
        let mut claimed = FxHashSet::default();
        if let Some(root) = self.payload.parts.get(self.weapon_id) {
            active.insert(root.id.clone());
        }
        for (path, part_id) in selections {
            let Some(part_id) = part_id else { continue };
            if self.selection_is_default(path, part_id) {
                continue;
            }
            let Some(part) = self.payload.parts.get(part_id) else {
                continue;
            };
            if part.is_virtual || self.excluded.contains(&part.id) {
                continue;
            }
            if let Some(slot_id) = socket_id_from_path(path) {
                active.insert(part.id.clone());
                for socket in socket_set(&slot_id, part) {
                    claimed.insert(socket);
                }
            }
        }
        for (path, node) in &self.defaults {
            let explicitly_set = selections.get(path).is_some_and(|part| {
                !part
                    .as_deref()
                    .is_some_and(|id| self.selection_is_default(path, id))
            });
            if explicitly_set {
                continue;
            }
            let Some(part) = self.payload.parts.get(&node.part_id) else {
                continue;
            };
            if part.is_virtual {
                continue;
            }
            if let Some(slot_id) = socket_id_from_path(path) {
                active.insert(part.id.clone());
                for socket in socket_set(&slot_id, part) {
                    claimed.insert(socket);
                }
            }
        }
        (active, claimed)
    }

    fn select_virtual(
        &self,
        active: &FxHashSet<String>,
        claimed: &FxHashSet<String>,
    ) -> Map<String, String> {
        let mut previous: Map<String, String> = Map::default();
        for _ in 0..64 {
            let mut selected: Map<String, String> = Map::default();
            let mut newly_claimed: Vec<String> = Vec::new();
            let is_active = |id: &str| active.contains(id) || previous.values().any(|p| p == id);
            for (socket_id, rules) in &self.virtual_groups {
                if claimed.contains(socket_id) {
                    continue;
                }
                let mut picked = None;
                for rule in rules {
                    if rule.condition_type == "ItemId"
                        && rule.condition_item_ids.iter().any(|id| is_active(id))
                    {
                        picked = Some(*rule);
                        break;
                    }
                }
                if picked.is_none() {
                    for rule in rules {
                        if rule.condition_type == "None" {
                            picked = Some(*rule);
                            break;
                        }
                    }
                }
                let Some(rule) = picked else { continue };
                let Some(part) = self.payload.parts.get(&rule.virtual_part_id) else {
                    continue;
                };
                if !part.is_virtual || self.excluded.contains(&part.id) {
                    continue;
                }
                let sockets = socket_set(socket_id, part);
                if sockets
                    .iter()
                    .any(|s| claimed.contains(s) || newly_claimed.contains(s))
                {
                    continue;
                }
                selected.insert(socket_id.clone(), part.id.clone());
                newly_claimed.extend(sockets);
            }
            if same_virtual(&previous, &selected) {
                return selected;
            }
            previous = selected;
        }
        previous
    }

    fn build_tree(
        &self,
        selections: &Map<String, Option<String>>,
        virtual_sel: &Map<String, String>,
    ) -> Assembly {
        let mut state = TreeState::default();
        let mut seen = FxHashSet::default();
        seen.insert(self.weapon_id.to_string());
        let root_path = format!("weapon:{}", enc(self.weapon_id));
        self.walk(
            self.weapon_id,
            &root_path,
            None,
            0,
            &mut seen,
            selections,
            virtual_sel,
            &mut state,
        );

        let mut visible = Vec::new();
        for slot in state.visible_slots {
            let host_path = host_path_key(&slot.path_key);
            let scoped_key = socket_key(&host_path, &slot.slot.id);
            let scoped = state.scoped_claims.get(&scoped_key);
            let global = state.global_claims.get(&slot.slot.id);
            let block = match global {
                Some(owner) if !is_descendant_path(&slot.path_key, owner) => Some(owner),
                _ => None,
            };
            let claim = scoped.or(block);
            if let Some(owner) = claim {
                if owner != &slot.path_key {
                    let allowed_internal = scoped == Some(owner)
                        && state.forced_paths.contains(owner)
                        && socket_id_from_path(owner).as_deref() == Some(slot.slot.id.as_str());
                    if !allowed_internal {
                        continue;
                    }
                }
            }
            visible.push(slot);
        }

        Assembly {
            visible_slots: visible,
            internal_parts: state.internal_parts,
            active_part_ids: state.active_part_ids,
            non_virtual_claimed_socket_ids: state.non_virtual_claimed_socket_ids,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn walk(
        &self,
        host_id: &str,
        host_path: &str,
        parent_slot_path: Option<&str>,
        depth: usize,
        seen: &mut FxHashSet<String>,
        selections: &Map<String, Option<String>>,
        virtual_sel: &Map<String, String>,
        state: &mut TreeState,
    ) {
        let Some(slots) = self.payload.slots_by_host.get(host_id) else {
            return;
        };
        for original in slots {
            let meta = match self.slot_data.get(&(original as *const SlotDef as usize)) {
                Some(meta) => meta.clone(),
                None => {
                    let candidates = self.candidates(host_id, original);
                    let candidate_ids: Vec<String> =
                        candidates.iter().map(|p| p.id.clone()).collect();
                    let mut suffix = String::new();
                    suffix.push_str("/slot:");
                    enc_into(&mut suffix, &original.id);
                    suffix.push(':');
                    enc_into(&mut suffix, &original.name);
                    Rc::new(SlotData {
                        suffix,
                        view: Rc::new(SlotView::from_slot(original)),
                        candidate_ids: Rc::new(candidate_ids),
                        candidates: Rc::new(candidates),
                    })
                }
            };
            let slot: &SlotDef = original;
            let mut path_key = String::with_capacity(host_path.len() + meta.suffix.len());
            path_key.push_str(host_path);
            path_key.push_str(&meta.suffix);
            let candidates = &meta.candidates;
            let raw_selection = selections.get(&path_key);
            let default_like = raw_selection
                .and_then(|v| v.as_deref())
                .is_some_and(|id| self.selection_is_default(&path_key, id));
            let has_selection = raw_selection.is_some() && !default_like;
            let selected_raw = if has_selection {
                raw_selection.and_then(|v| v.clone())
            } else {
                None
            };

            let explicit = selected_raw.and_then(|id| {
                if candidates.iter().any(|p| p.id == id) {
                    self.payload.parts.get(&id)
                } else {
                    None
                }
            });

            let default_part = if !has_selection {
                self.defaults
                    .get(&path_key)
                    .and_then(|d| self.payload.parts.get(&d.part_id))
                    .filter(|p| !p.is_virtual)
            } else {
                None
            };

            let virtual_part = virtual_sel
                .get(&slot.id)
                .and_then(|id| self.payload.parts.get(id))
                .filter(|p| p.is_virtual && slot.virtual_candidate_ids.contains(&p.id));

            let mut chosen: Option<&Part> = explicit.or(default_part).or(virtual_part);
            let mut internal = if chosen.is_some() && explicit.is_none() {
                if default_part.is_some() {
                    Some("default")
                } else {
                    Some("virtual")
                }
            } else {
                None
            };

            if let Some(part) = chosen {
                let claims = self.claims(&path_key, &slot.id, part);
                if self.claims_conflict(&claims, state) {
                    chosen = None;
                    internal = None;
                } else {
                    state.active_part_ids.insert(part.id.clone());
                    for claim in &claims {
                        if let Some(key) = &claim.scoped_key {
                            state.scoped_claims.insert(key.clone(), claim.owner_path_key.clone());
                            state
                                .socket_claims
                                .insert(claim.socket_id.clone(), claim.owner_path_key.clone());
                        } else {
                            state
                                .global_claims
                                .insert(claim.socket_id.clone(), claim.owner_path_key.clone());
                        }
                    }
                    if !part.is_virtual {
                        for socket in socket_set(&slot.id, part) {
                            state.non_virtual_claimed_socket_ids.insert(socket);
                        }
                    }
                    if internal.is_some() {
                        state.forced_paths.insert(path_key.clone());
                    }
                }
            }

            if !slot.default_hidden
                && !slot.hide_for_player
                && !slot.all_candidates_virtual
                && !candidates.is_empty()
            {
                let (selected_id, internal_id) = match (&chosen, &internal) {
                    (Some(part), None) => (Some(part.id.clone()), None),
                    (Some(part), Some(_)) => (None, Some(part.id.clone())),
                    _ => (None, None),
                };
                state.visible_slots.push(VisibleSlot {
                    path_key: path_key.clone(),
                    parent_slot_path_key: parent_slot_path.map(|s| s.to_string()),
                    host_part_id: host_id.to_string(),
                    depth,
                    slot: meta.view.clone(),
                    candidate_ids: meta.candidate_ids.clone(),
                    selected_part_id: selected_id,
                    internal_part_id: internal_id,
                    internal_is_default: internal == Some("default"),
                });
            }

            if let (Some(part), Some(source)) = (&chosen, &internal) {
                state.internal_parts.push(InternalPart {
                    slot_path_key: path_key.clone(),
                    parent_slot_path_key: parent_slot_path.map(|s| s.to_string()),
                    host_part_id: host_id.to_string(),
                    depth,
                    slot: meta.view.clone(),
                    part_id: part.id.clone(),
                    source: (*source).to_string(),
                });
            }

            if let Some(part) = chosen {
                if depth < 32 && !seen.contains(&part.id) {
                    seen.insert(part.id.clone());
                    let mut child_host_path =
                        String::with_capacity(path_key.len() + 6 + part.id.len());
                    child_host_path.push_str(&path_key);
                    child_host_path.push_str("/part:");
                    enc_into(&mut child_host_path, &part.id);
                    self.walk(
                        &part.id,
                        &child_host_path,
                        Some(&path_key),
                        depth + 1,
                        seen,
                        selections,
                        virtual_sel,
                        state,
                    );
                    seen.remove(&part.id);
                }
            }
        }
    }

    fn candidates(&self, host_id: &str, slot: &SlotDef) -> Vec<&'a Part> {
        slot.candidate_ids
            .iter()
            .filter_map(|id| self.payload.parts.get(id))
            .filter(|part| {
                if part.is_virtual || part.is_hidden() || self.excluded.contains(&part.id) {
                    return false;
                }
                !self.hide_overrides.iter().any(|ov| {
                    ov.decision == "hide_candidate"
                        && ov.part_id == part.id
                        && ov
                            .host_part_id
                            .as_deref()
                            .map(|h| h == host_id)
                            .unwrap_or(true)
                        && ov
                            .socket_id
                            .as_deref()
                            .map(|s| s == slot.id)
                            .unwrap_or(true)
                })
            })
            .collect()
    }

    fn claims(&self, path: &str, socket_id: &str, part: &Part) -> Vec<Claim> {
        let host = host_path_key(path);
        let mut out = vec![Claim {
            socket_id: socket_id.to_string(),
            scoped_key: Some(socket_key(&host, socket_id)),
            owner_path_key: path.to_string(),
        }];
        for extra in &part.extra_occupy_socket_ids {
            out.push(Claim {
                socket_id: extra.clone(),
                scoped_key: None,
                owner_path_key: path.to_string(),
            });
        }
        out
    }

    fn claims_conflict(&self, claims: &[Claim], state: &TreeState) -> bool {
        for claim in claims {
            match &claim.scoped_key {
                Some(key) => {
                    if state.scoped_claims.contains_key(key) {
                        return true;
                    }
                    if let Some(owner) = state.global_claims.get(&claim.socket_id) {
                        if !is_descendant_path(&claim.owner_path_key, owner) {
                            return true;
                        }
                    }
                }
                None => {
                    if state.global_claims.contains_key(&claim.socket_id)
                        || state.socket_claims.contains_key(&claim.socket_id)
                    {
                        return true;
                    }
                }
            }
        }
        false
    }
}

#[derive(Default)]
struct TreeState {
    visible_slots: Vec<VisibleSlot>,
    internal_parts: Vec<InternalPart>,
    active_part_ids: FxHashSet<String>,
    non_virtual_claimed_socket_ids: FxHashSet<String>,
    scoped_claims: FxHashMap<String, String>,
    socket_claims: FxHashMap<String, String>,
    global_claims: FxHashMap<String, String>,
    forced_paths: FxHashSet<String>,
}

fn same_virtual(a: &Map<String, String>, b: &Map<String, String>) -> bool {
    a.len() == b.len() && a.iter().all(|(k, v)| b.get(k) == Some(v))
}

fn socket_set(slot_id: &str, part: &Part) -> FxHashSet<String> {
    let mut set = FxHashSet::default();
    set.insert(slot_id.to_string());
    for id in &part.extra_occupy_socket_ids {
        set.insert(id.clone());
    }
    set
}

pub fn host_path_key(path: &str) -> String {
    match path.rfind("/slot:") {
        Some(idx) => path[..idx].to_string(),
        None => path.to_string(),
    }
}

pub fn socket_id_from_path(path: &str) -> Option<String> {
    let idx = path.rfind("/slot:")?;
    let rest = &path[idx + 6..];
    let end = rest.find(':').unwrap_or(rest.len());
    Some(dec(&rest[..end]))
}

pub fn socket_key(host_path: &str, socket_id: &str) -> String {
    let mut out = String::with_capacity(host_path.len() + 8 + socket_id.len());
    out.push_str(host_path);
    out.push_str("/socket:");
    enc_into(&mut out, socket_id);
    out
}

fn enc_into(out: &mut String, value: &str) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for b in value.as_bytes() {
        let c = *b as char;
        if c.is_ascii_alphanumeric()
            || matches!(c, '-' | '_' | '.' | '!' | '~' | '*' | '\'' | '(' | ')')
        {
            out.push(c);
        } else {
            out.push('%');
            out.push(HEX[(b >> 4) as usize] as char);
            out.push(HEX[(b & 0x0f) as usize] as char);
        }
    }
}

fn is_descendant_path(path: &str, ancestor: &str) -> bool {
    path.len() >= ancestor.len() + b"/part:".len()
        && path.as_bytes().starts_with(ancestor.as_bytes())
        && path.as_bytes()[ancestor.len()..].starts_with(b"/part:")
}

/// The part a resolved slot actually installs, whether it is a visible
/// selection, a default/virtual part shown inside a visible slot, or an
/// internal part of a slot that never becomes visible (hidden or all-virtual
/// slots).
pub fn installed_part_id<'a>(assembly: &'a Assembly, path: &str) -> Option<&'a str> {
    if let Some(slot) = assembly
        .visible_slots
        .iter()
        .find(|slot| slot.path_key == path)
    {
        return slot
            .selected_part_id
            .as_deref()
            .or(slot.internal_part_id.as_deref());
    }
    assembly
        .internal_parts
        .iter()
        .find(|part| part.slot_path_key == path)
        .map(|part| part.part_id.as_str())
}

/// Whether a locked path is still honoured by the resolved assembly. The
/// resolver silently drops a locked part when an earlier slot claims its
/// socket through `extraOccupySocketIds`, so the raw selection map alone is
/// no evidence that the lock was installed. All-virtual slots never produce a
/// visible slot even when their virtual part is installed; those show up as
/// internal parts instead.
pub fn lock_honoured(assembly: &Assembly, path: &str, expected: Option<&str>) -> bool {
    let visible = assembly
        .visible_slots
        .iter()
        .any(|slot| slot.path_key == path);
    let installed = installed_part_id(assembly, path);
    match expected {
        Some(expected) => installed == Some(expected),
        None => visible && installed.is_none(),
    }
}

pub fn locks_honoured(locked_paths: &Map<String, Option<String>>, assembly: &Assembly) -> bool {
    locked_paths
        .iter()
        .all(|(path, expected)| lock_honoured(assembly, path, expected.as_deref()))
}

pub fn enc(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for b in value.as_bytes() {
        let c = *b as char;
        if c.is_ascii_alphanumeric()
            || matches!(c, '-' | '_' | '.' | '!' | '~' | '*' | '\'' | '(' | ')')
        {
            out.push(c);
        } else {
            out.push('%');
            out.push_str(&format!("{:02X}", b));
        }
    }
    out
}

fn dec(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(&value[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

pub fn preset_node_part<'a>(
    payload: &'a WeaponPayload,
    node: &PresetNode,
) -> Option<&'a Part> {
    payload.parts.get(&node.part_id)
}

pub fn value_str(value: &Value) -> Option<&str> {
    value.as_str()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descendant_paths_match_without_allocation() {
        assert!(is_descendant_path("weapon:w/slot:1:X/part:p", "weapon:w/slot:1:X"));
        assert!(!is_descendant_path("weapon:w/slot:1:X", "weapon:w/slot:1:X"));
        assert!(!is_descendant_path("weapon:w/slot:1:X/other:p", "weapon:w/slot:1:X"));
        assert!(!is_descendant_path("weapon:w/slot:1:XY/part:p", "weapon:w/slot:1:X"));
    }
}
