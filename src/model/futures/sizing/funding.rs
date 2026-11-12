//! Whether sizing can trust what it is given, looked at in one fixed
//! order before anything is worked out, and which of the account's pools
//! and wallets fund an entry. The order is part of the contract: the
//! first input at fault is the one reported.

use super::fault::{SizingFault, SizingInput};
use super::request::{SizingRequest, UsdIndex};
use crate::model::futures::{AccountSnapshot, MarginPool, PoolScope, WalletBalance};
use crate::model::{Asset, Money};
use rust_decimal::Decimal;

/// The asset an account's USD pool is stated in.
const USD: &str = "USD";

/// What funds an entry, read from a snapshot sizing trusts.
pub(super) struct Funding<'a> {
    pub(super) pool: &'a MarginPool,
    /// The index price a USD pool is measured at; `None` for an asset's
    /// own pool.
    pub(super) index: Option<&'a UsdIndex>,
    /// The settlement asset's wallet; `None` where the account lists
    /// none, which holds nothing.
    pub(super) wallet: Option<&'a WalletBalance>,
}

/// The pool an entry settled in `settle` draws its margin from: the
/// asset's own pool, or the account's USD pool. A venue's adapter reports
/// the pools the account's margin mode gives it, so exactly one of the two
/// is there. An account that reports both, or neither, is a fault: sizing
/// never chooses a pool. A caller totals its reservations against the
/// pool this names.
pub fn margin_pool<'a>(
    account: &'a AccountSnapshot,
    settle: &Asset,
) -> Result<&'a MarginPool, SizingFault> {
    let mut serving = account.pools.iter().filter(|pool| match &pool.scope {
        PoolScope::Asset(asset) => asset == settle,
        PoolScope::AccountUsd => true,
    });
    let Some(pool) = serving.next() else {
        return Err(SizingFault::missing(
            SizingInput::Pool,
            format!("the account reports no {settle} pool and no USD pool"),
        ));
    };
    let others = serving.count();
    if others > 0 {
        return Err(SizingFault::ambiguous(
            SizingInput::Pool,
            format!(
                "the account reports {} pools an entry settled in {settle} could draw on, and \
                 sizing does not choose between them",
                others + 1
            ),
        ));
    }
    let unit = match &pool.scope {
        PoolScope::Asset(asset) => asset.as_str(),
        PoolScope::AccountUsd => USD,
    };
    if pool.available.asset().as_str() != unit {
        return Err(SizingFault::invalid(
            SizingInput::Pool,
            format!(
                "its available margin is stated in {}, and the pool is in {unit}",
                pool.available.asset()
            ),
        ));
    }
    Ok(pool)
}

/// Every check of what a request brings, in the contract's order.
pub(super) fn trusted<'a>(request: &SizingRequest<'a>) -> Result<Funding<'a>, SizingFault> {
    let policy = request.policy;
    let listed = request.instrument.id();
    let settle = &policy.instrument().settle;
    let (now, limits) = (request.now_ms, request.freshness);
    if listed != policy.instrument() {
        return Err(SizingFault::mismatched(
            SizingInput::Rules,
            format!(
                "they are {listed}'s, and the entry was admitted on {}",
                policy.instrument()
            ),
        ));
    }
    let account = request.account;
    if &account.account != policy.account() {
        return Err(SizingFault::mismatched(
            SizingInput::Account,
            "it is another account's than the one the entry's profile trades on",
        ));
    }
    let rules_at = request.instrument.spec().observed_at_ms;
    SizingFault::unless_fresh(SizingInput::Rules, "", rules_at, now, limits.rules_ms)?;

    let brackets = request.brackets;
    if brackets.value.is_empty() {
        return Err(SizingFault::missing(
            SizingInput::Brackets,
            format!("the venue gave none for {}", listed.symbol),
        ));
    }
    let unusable = |cap: &Money| cap.asset() != settle || cap.amount() <= Decimal::ZERO;
    if let Some(tier) = brackets.value.iter().find(|tier| unusable(&tier.cap)) {
        return Err(SizingFault::invalid(
            SizingInput::Brackets,
            format!(
                "a cap of {} cannot hold a position settled in {settle}",
                tier.cap
            ),
        ));
    }
    let (at_ms, limit) = (brackets.at_ms, limits.rules_ms);
    SizingFault::unless_fresh(SizingInput::Brackets, "", at_ms, now, limit)?;

    let taker = request.fees.value.taker;
    if taker < Decimal::ZERO {
        return Err(SizingFault::invalid(
            SizingInput::Fees,
            format!("a taker fee of {taker} is below zero"),
        ));
    }
    let (at_ms, limit) = (request.fees.at_ms, limits.rules_ms);
    SizingFault::unless_fresh(SizingInput::Fees, "", at_ms, now, limit)?;
    let (at_ms, limit) = (account.observed_at_ms, limits.account_ms);
    SizingFault::unless_fresh(SizingInput::Account, "", at_ms, now, limit)?;

    let pool = margin_pool(account, settle)?;
    let unit = pool.available.asset();
    let reserved = request.reserved;
    if reserved.asset() != unit {
        return Err(SizingFault::invalid(
            SizingInput::Reservation,
            format!(
                "it is stated in {}, and the pool is in {unit}",
                reserved.asset()
            ),
        ));
    }
    if reserved.amount() < Decimal::ZERO {
        return Err(SizingFault::invalid(
            SizingInput::Reservation,
            format!("{reserved} is below zero"),
        ));
    }
    let index = match pool.scope {
        PoolScope::Asset(_) => None,
        PoolScope::AccountUsd => Some(usd_index(request, settle)?),
    };

    let mut wallets = account
        .wallets
        .iter()
        .filter(|wallet| wallet.balance.asset() == settle);
    let wallet = wallets.next();
    if wallets.next().is_some() {
        return Err(SizingFault::ambiguous(
            SizingInput::Wallet,
            format!("the account reports more than one {settle} wallet"),
        ));
    }
    Ok(Funding {
        pool,
        index,
        wallet,
    })
}

/// The index price that measures `settle` against a USD pool.
fn usd_index<'a>(request: &SizingRequest<'a>, settle: &Asset) -> Result<&'a UsdIndex, SizingFault> {
    let Some(index) = request.usd_index else {
        return Err(SizingFault::missing(
            SizingInput::UsdIndex,
            format!(
                "the entry's margin pool is in USD, and no index price of {settle} was given to \
                 measure it at"
            ),
        ));
    };
    if &index.asset != settle {
        return Err(SizingFault::mismatched(
            SizingInput::UsdIndex,
            format!("it is {}'s, and the entry settles in {settle}", index.asset),
        ));
    }
    if index.source.trim().is_empty() {
        return Err(SizingFault::invalid(
            SizingInput::UsdIndex,
            "it does not say which of the venue's indexes it is",
        ));
    }
    SizingFault::unless_fresh(
        SizingInput::UsdIndex,
        "",
        index.observed_at_ms,
        request.now_ms,
        request.freshness.prices_ms,
    )?;
    Ok(index)
}
