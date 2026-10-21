use serde_json::Value;
use std::time::Instant;

fn run_case(request_file: &str, expected_total_price: u64) {
    run_case_with_payload(
        "rm277-stability-fire-camera.payload.json",
        request_file,
        expected_total_price,
    );
}

fn run_case_with_payload(payload_file: &str, request_file: &str, expected_total_price: u64) {
    run_case_with_payload_and_leaves(payload_file, request_file, expected_total_price, None);
}

fn run_case_with_payload_and_leaves(
    payload_file: &str,
    request_file: &str,
    expected_total_price: u64,
    max_leaves: Option<u64>,
) {
    let payload = std::fs::read(format!(
        "{}/tests/fixtures/{payload_file}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    let request = std::fs::read(format!(
        "{}/tests/fixtures/{request_file}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    let started = Instant::now();
    let out = delta_weapon_solver::solve_slices(&payload, &request).unwrap();
    let elapsed = started.elapsed();
    let out: Value = serde_json::from_slice(&out).unwrap();
    println!("{request_file}: {elapsed:?}");
    assert_eq!(out["optimal"], true, "{request_file}: {out}");
    assert_eq!(out["truncated"], false, "{request_file}: {out}");
    assert_eq!(
        out["best"]["total_price"], expected_total_price,
        "{request_file}: {out}"
    );
    if let Some(max_leaves) = max_leaves {
        assert!(
            out["leaves"].as_u64().unwrap() < max_leaves,
            "{request_file}: {} leaves is not below {max_leaves}",
            out["leaves"]
        );
    }
}

#[test]
#[ignore = "slow regression; run with --release -- --ignored"]
fn live_request_aim_regression() {
    run_case("live_request_aim.request.json", 254841);
}

#[test]
#[ignore = "slow regression; run with --release -- --ignored"]
fn live_mix2_regression() {
    // 171528 missed a build sitting exactly on the rounded fire-stability bound.
    run_case("live_mix2.request.json", 168002);
}

#[test]
#[ignore = "slow regression; run with --release -- --ignored"]
fn live_mix3_regression() {
    run_case("live_mix3.request.json", 166209);
}

#[test]
#[ignore = "slow regression; run with --release -- --ignored"]
fn live_aim400_only_regression() {
    run_case("live_aim400_only.request.json", 105865);
}

#[test]
#[ignore = "slow regression; run with --release -- --ignored"]
fn no_aim_regression() {
    // 252275 missed a build sitting exactly on the rounded fire-stability bound.
    run_case("no_aim.request.json", 251420);
}

#[test]
#[ignore = "slow regression; run with --release -- --ignored"]
fn ak12_aim_recoil_regression() {
    run_case_with_payload_and_leaves(
        "ak12-aim-recoil.payload.json",
        "ak12-aim-recoil.request.json",
        208037,
        Some(5_000),
    );
}
