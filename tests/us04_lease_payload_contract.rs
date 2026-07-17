use cc_switch_server::clients::router::client::NamespaceLeasePayload;
use cc_switch_server::domain::sharing::router_contract::ShareDescriptor;

#[test]
fn namespace_lease_payload_matches_router_signature_blob() {
    let request_json =
        include_str!("../../cc-switch-router/tests/fixtures/us04_share_lease_request.json");
    let expected_signed =
        include_str!("../../cc-switch-router/tests/fixtures/us04_share_lease_signed_payload.json")
            .trim();
    let value: serde_json::Value = serde_json::from_str(request_json).expect("request json");
    let share: ShareDescriptor =
        serde_json::from_value(value.get("share").cloned().expect("share"))
            .expect("share descriptor");
    let payload = NamespaceLeasePayload {
        protocol_epoch: value["protocolEpoch"].as_str().unwrap().to_string(),
        router_id: value["routerId"].as_str().unwrap().to_string(),
        route_id: value["routeId"].as_str().unwrap().to_string(),
        rotation_id: value["rotationId"].as_str().unwrap().to_string(),
        generation: value["generation"].as_u64().unwrap(),
        expected_generation: value["expectedGeneration"].as_u64().unwrap(),
        requested_subdomain: value["requestedSubdomain"].as_str().unwrap().to_string(),
        tunnel_type: value["tunnelType"].as_str().unwrap().to_string(),
        share: Some(share),
    };
    let actual = serde_json::to_string(&payload).expect("serialize payload");
    if actual != expected_signed {
        let common = actual
            .chars()
            .zip(expected_signed.chars())
            .take_while(|(a, b)| a == b)
            .count();
        eprintln!("first mismatch at byte {common}");
        eprintln!(
            "actual around mismatch: {}",
            &actual[common.saturating_sub(40)..actual.len().min(common + 120)]
        );
        eprintln!(
            "expected around mismatch: {}",
            &expected_signed[common.saturating_sub(40)..expected_signed.len().min(common + 120)]
        );
    }
    assert_eq!(actual, expected_signed);
}
