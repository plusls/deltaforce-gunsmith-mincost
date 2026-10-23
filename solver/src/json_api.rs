use crate::engine::solve_request;
use crate::model::{Map, WeaponPayload};
use crate::request::{SolveRequest, SolveRequestOptions};
use crate::search::SolveResult;
use std::io::Write;

pub fn solve_slices(payload_json: &[u8], request_json: &[u8]) -> Result<Vec<u8>, String> {
    let payload: WeaponPayload =
        serde_json::from_slice(payload_json).map_err(|e| format!("payload: {e}"))?;
    let options: SolveRequestOptions =
        serde_json::from_slice(request_json).map_err(|e| format!("request: {e}"))?;
    let req = SolveRequest { payload, options };
    let result = solve_request(req)?;
    serde_json::to_vec(&result).map_err(|e| e.to_string())
}

pub fn solve_json_str(payload_json: &str, request_json: &str) -> Result<String, String> {
    let bytes = solve_slices(payload_json.as_bytes(), request_json.as_bytes())?;
    String::from_utf8(bytes).map_err(|e| e.to_string())
}

#[allow(dead_code)]
pub fn write_result<W: Write>(w: &mut W, result: &SolveResult) -> std::io::Result<()> {
    serde_json::to_writer(w, result)?;
    Ok(())
}

#[allow(dead_code)]
pub fn empty_map() -> Map<String, String> {
    Map::default()
}
