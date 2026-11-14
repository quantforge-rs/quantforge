//! A delivery's body as the model's signal, and a refusal as the receipt
//! that answers it: the one place the wire's signal becomes the model's.
//! The webhook route that authenticates a delivery, records it and answers
//! it has a story of its own; this is what it reads a body with.

use crate::model::futures::{
    ExternalSignal, ProfileId, SignalAction, SignalHash, SignalId, SignalRefusal,
};
use crate::model::{Category, ModelError, PositionSide, Price};
use crate::protocol::signal::{Signal, SignalCategory, SignalReceipt, SignalSide};
use crate::protocol::{ErrorBody, ErrorCode, ErrorEnvelope, ExactDecimal};

/// Reads a delivery's body, exactly as it arrived. A body that is not a
/// signal is answered `400` with the envelope returned here: its code is
/// `invalid_request`, which TradingView does not send again, and its
/// message names everything wrong with the body.
pub fn read_signal(body: &[u8]) -> Result<ExternalSignal, ErrorEnvelope> {
    let refuse = |message: String| ErrorEnvelope {
        error: ErrorBody {
            code: ErrorCode::InvalidRequest,
            message,
            supported: None,
        },
    };
    let signal = Signal::read(body).map_err(|problems| refuse(problems.to_string()))?;
    // The format's rules are the model's, so this does not fail; if they
    // ever part, the body is refused rather than half read.
    external_signal(&signal).map_err(|error| refuse(format!("the signal cannot be read: {error}")))
}

/// The model's signal for the wire's: the same alert, with its prices as
/// prices, its side as a position's, and its ID and payload hash as the
/// format builds them.
pub fn external_signal(signal: &Signal) -> Result<ExternalSignal, ModelError> {
    let price = |value: ExactDecimal| Price::new(value.get());
    let action = match signal.price() {
        Some(expected) => SignalAction::Entry {
            expected_price: price(expected)?,
            stop: signal.stop().map(price).transpose()?,
        },
        None => SignalAction::Exit,
    };
    Ok(ExternalSignal {
        id: SignalId::new(signal.signal_id())?,
        payload_hash: SignalHash::from_digest(*signal.payload_hash().digest()),
        profile: ProfileId::new(signal.profile())?,
        ticker: signal.ticker().to_string(),
        exchange: signal.exchange().map(str::to_string),
        category: signal.category().map(|category| match category {
            SignalCategory::Linear => Category::Linear,
            SignalCategory::Inverse => Category::Inverse,
        }),
        side: match signal.side() {
            SignalSide::Long => PositionSide::Long,
            SignalSide::Short => PositionSide::Short,
        },
        action,
        bar_time_ms: signal.bar_time().ms(),
        fire_time_ms: signal.fire_time().ms(),
    })
}

/// The receipt of a signal that was recorded and refused: the refusal's
/// name for a program, and its message for the operator.
pub fn refusal_receipt(signal: &ExternalSignal, refusal: &SignalRefusal) -> SignalReceipt {
    SignalReceipt::rejected(signal.id.as_str(), refusal.refusal.as_str())
        .with_message(refusal.message.as_str())
}
