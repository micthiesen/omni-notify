//! A `briefing-history` row whose cost is `null` (an unpriced model), written by
//! node-cbor (committed `tests/golden/ts_rows.json`): `null` must
//! survive a read-modify-write instead of collapsing to absent.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use omni_briefings::BriefingHistoryData;
use omni_briefings::persistence::CostCents;
use omni_store::cbor;

#[test]
fn null_costs_round_trip_byte_for_byte() {
    let golden = omni_testkit::golden("ts_rows.json");
    let hex = golden["briefing-history-null-cost"].as_str().unwrap();
    let original: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect();
    let history: BriefingHistoryData = cbor::from_value(cbor::decode(&original).unwrap()).unwrap();
    assert_eq!(history.notifications[0].cost_cents, CostCents::Unpriced);
    let encoded = cbor::encode(&cbor::to_value(&history).unwrap());
    assert_eq!(encoded, original);
}
