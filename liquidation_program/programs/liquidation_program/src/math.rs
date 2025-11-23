use anchor_lang::prelude::*;

/// Calculates PnL for a position
/// Long:  (price - entry) * quantity
/// Short: (entry - price) * quantity
pub fn compute_pnl(entry_price: u64, price: u64, quantity: u64, is_long: bool) -> i64 {
    let price_diff = if is_long {
        price as i64 - entry_price as i64
    } else {
        entry_price as i64 - price as i64
    };

    price_diff * (quantity as i64)
}

/// Collateral + PnL
pub fn compute_margin(collateral: i64, pnl: i64) -> i64 {
    collateral + pnl
}

/// Notional = price * quantity
pub fn compute_notional(price: u64, quantity: u64) -> u128 {
    (price as u128) * (quantity as u128)
}

/// Maintenance margin = 0.5% of notional
/// Example: 50 bps = 50 / 10000
pub fn compute_maintenance_margin(notional: u128, bps: u64) -> i64 {
    ((notional * (bps as u128)) / 10_000u128) as i64
}

/// Margin Ratio = margin / notional * 10000 (basis points)
pub fn compute_margin_ratio(margin: i64, notional: u128) -> i64 {
    if notional == 0 {
        return i64::MAX; // avoid division by zero
    }
    ((margin as i128 * 10_000) / notional as i128) as i64
}

/// Partial liquidation quantity = 50% of current quantity
pub fn compute_partial_liquidation_amount(quantity: u64) -> u64 {
    quantity / 2
}

/// Reward given to the liquidator
pub fn compute_liquidator_reward(notional: u128, reward_bps: u64) -> u64 {
    ((notional * reward_bps as u128) / 10_000u128) as u64
}
