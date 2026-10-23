use gunsmith_mincost::assembly::Assembler;
use gunsmith_mincost::model::{Map, WeaponPayload};
use serde_json::json;

fn payload() -> WeaponPayload {
    serde_json::from_value(json!({
        "dataVersion": "test",
        "parts": {
            "w1": {"id": "w1", "name": "gun"},
            "h": {"id": "h", "name": "h"},
            "cond": {"id": "cond", "name": "cond"},
            "v_none": {"id": "v_none", "name": "v_none", "isVirtual": true},
            "v_item": {"id": "v_item", "name": "v_item", "isVirtual": true}
        },
        "slotsByHost": {
            "w1": [
                {"id": "1", "name": "Muzzle_point", "candidateIds": ["h"]},
                {"id": "3", "name": "Cond_point", "candidateIds": ["cond"]}
            ],
            "h": [
                {
                    "id": "2",
                    "name": "Underbarrel_point",
                    "candidateIds": [],
                    "virtualCandidateIds": ["v_none", "v_item"],
                    "allCandidatesVirtual": true
                }
            ]
        },
        "defaultPresetsByWeapon": {"w1": {"nodes": [{"isRoot": true, "partId": "w1"}]}},
        "virtualRulesByWeapon": {"w1": [
            {"socketId": "2", "virtualPartId": "v_none", "conditionType": "None"},
            {"socketId": "2", "virtualPartId": "v_item", "conditionType": "ItemId", "conditionItemIds": ["cond"]}
        ]}
    }))
    .expect("payload")
}

fn selections(with_condition: bool) -> Map<String, Option<String>> {
    let mut sel = Map::default();
    sel.insert(
        "weapon:w1/slot:1:Muzzle_point".to_string(),
        Some("h".to_string()),
    );
    if with_condition {
        sel.insert(
            "weapon:w1/slot:3:Cond_point".to_string(),
            Some("cond".to_string()),
        );
    }
    sel
}

#[test]
fn item_id_rule_beats_none_rule_regardless_of_payload_order() {
    let payload = payload();
    let assembler = Assembler::new(&payload, "w1");
    let assembly = assembler.resolve(&selections(true));
    let ids = assembly.all_part_ids();
    assert!(ids.contains("v_item"), "installed: {ids:?}");
    assert!(!ids.contains("v_none"), "installed: {ids:?}");
}

#[test]
fn none_rule_is_used_when_no_item_id_condition_is_active() {
    let payload = payload();
    let assembler = Assembler::new(&payload, "w1");
    let assembly = assembler.resolve(&selections(false));
    let ids = assembly.all_part_ids();
    assert!(ids.contains("v_none"), "installed: {ids:?}");
    assert!(!ids.contains("v_item"), "installed: {ids:?}");
}
