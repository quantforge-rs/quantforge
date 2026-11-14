//! The receipts a delivery is answered with: the ones the format takes
//! and refuses, the states a receipt may give, and the receipt of a signal
//! that was refused.

use quantforge::model::Refusal;
use quantforge::model::futures::{IntentState, SignalRefusal};
use quantforge::protocol::signal::{ReceiptState, SignalReceipt};
use quantforge::server::refusal_receipt;
use serde_json::Value;

use crate::cases::fixture;

#[test]
fn every_receipt_of_the_cases_is_taken_or_refused_as_they_say() {
    let fixture = fixture();
    let mut shown = Vec::new();
    for case in &fixture.receipts {
        let read = serde_json::from_value::<SignalReceipt>(case.body.clone());
        match (case.valid, read) {
            (true, Ok(receipt)) => {
                // Written back, it is the body it was read from.
                let written = serde_json::to_value(&receipt).expect("json");
                assert_eq!(written, case.body, "{}", case.id);
                assert_eq!(
                    Some(receipt.state().as_str()),
                    case.body["state"].as_str(),
                    "{}",
                    case.id
                );
                assert_eq!(
                    receipt.refusal(),
                    case.body.get("refusal").and_then(Value::as_str)
                );
                assert_eq!(
                    receipt.message(),
                    case.body.get("message").and_then(Value::as_str)
                );
                assert_eq!(Some(receipt.signal_id()), case.body["signal_id"].as_str());
                shown.push(receipt.state());
            }
            (false, Err(_)) => {}
            (true, Err(error)) => panic!("{}: {} is refused: {error}", case.id, case.note),
            (false, Ok(receipt)) => panic!("{}: {} is taken: {receipt:?}", case.id, case.note),
        }
    }
    for state in ReceiptState::ALL {
        assert!(shown.contains(&state), "no case shows a {state} receipt");
    }
}

#[test]
fn a_receipt_writes_the_fields_the_cases_name_in_their_order() {
    let fixture = fixture();
    let full = SignalReceipt::rejected("s", "stale").with_message("m");
    let text = serde_json::to_string(&full).expect("json");
    let positions: Vec<usize> = fixture
        .receipt_fields
        .iter()
        .map(|field| {
            text.find(&format!("\"{field}\":"))
                .unwrap_or_else(|| panic!("{text} has no {field}"))
        })
        .collect();
    assert!(positions.windows(2).all(|pair| pair[0] < pair[1]), "{text}");
    let members = serde_json::from_str::<Value>(&text).expect("json");
    assert_eq!(
        members.as_object().map(|members| members.len()),
        Some(fixture.receipt_fields.len())
    );
}

#[test]
fn a_receipt_gives_only_the_states_a_signal_reaches_without_an_order() {
    // Where a signal is recorded, and where its first step can take it.
    let mut without_an_order = vec![IntentState::INITIAL];
    for (from, to) in IntentState::TRANSITIONS {
        if from == IntentState::INITIAL && !without_an_order.contains(&to) {
            without_an_order.push(to);
        }
    }
    let names: Vec<&str> = without_an_order
        .iter()
        .map(|state| state.as_str())
        .collect();
    let receipts: Vec<&str> = ReceiptState::ALL
        .iter()
        .map(|state| state.as_str())
        .collect();
    assert_eq!(receipts, names);
    // Every other state of a signal needs an order, and no receipt gives it.
    for state in IntentState::ALL {
        let read = serde_json::from_value::<ReceiptState>(Value::from(state.as_str()));
        assert_eq!(read.is_ok(), without_an_order.contains(&state), "{state:?}");
    }
}

#[test]
fn a_refused_signal_is_answered_with_its_refusals_name_and_message() {
    let fixture = fixture();
    let signal = fixture.signal("by-linear-long-entry");
    for refusal in Refusal::ALL {
        let refused = SignalRefusal {
            refusal,
            message: format!("what to change about {refusal}"),
        };
        let receipt = refusal_receipt(&signal, &refused);
        assert_eq!(receipt.state(), ReceiptState::Rejected);
        assert_eq!(receipt.signal_id(), signal.id.as_str());
        assert_eq!(receipt.refusal(), Some(refusal.as_str()));
        assert_eq!(receipt.message(), Some(refused.message.as_str()));
        // It is a receipt the format takes.
        let json = serde_json::to_string(&receipt).expect("json");
        assert_eq!(
            serde_json::from_str::<SignalReceipt>(&json).expect("a receipt"),
            receipt
        );
    }
    let stale = SignalRefusal {
        refusal: Refusal::Stale,
        message: "the alert fired 75 s ago, and profile by-linear takes entries up to 60 s old"
            .to_string(),
    };
    assert_eq!(
        serde_json::to_string(&refusal_receipt(&signal, &stale)).expect("json"),
        "{\"version\":1,\"signal_id\":\"trend:entry:long:SOLUSDT.P:2026-10-20T00:00:00Z\",\
         \"state\":\"rejected\",\"refusal\":\"stale\",\"message\":\"the alert fired 75 s ago, and \
         profile by-linear takes entries up to 60 s old\"}"
    );
}
