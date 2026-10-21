use serde_json::Value;
use std::time::Instant;

fn run_case(request_file: &str, expected_total_price: u64) {
    let payload = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/rm277-stability-fire-camera.payload.json"
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
}

#[test]
#[ignore = "slow regression; run with --release -- --ignored"]
fn live_request_aim_regression() {
    run_case("live_request_aim.request.json", 254841);
}

#[test]
#[ignore = "slow regression; run with --release -- --ignored"]
fn live_mix2_regression() {
    run_case("live_mix2.request.json", 171528);
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
    run_case("no_aim.request.json", 252275);
}
