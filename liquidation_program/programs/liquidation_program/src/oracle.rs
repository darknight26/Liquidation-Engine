use anchor_lang::prelude::*;
use pyth_sdk_solana::state::load_price_feed_from_account_info;
use anchor_lang::solana_program::account_info::AccountInfo;

use crate::constants::*;
use crate::state::*;



pub fn get_oracle_price(oracle_acc: &AccountInfo) -> Result<u64> {
    //  Parse Pyth price feed
    let price_feed = load_price_feed_from_account_info(oracle_acc)
        .map_err(|_| error!(ErrorCode::InvalidOracleAccount))?;

    // Latest price data
    let price_data = price_feed
        .get_price_no_older_than(Clock::get()?.unix_timestamp, MAX_ORACLE_STALENESS)
        .ok_or(error!(ErrorCode::StaleOraclePrice))?;

    let price_i64 = price_data.price;
    let conf_i64 = price_data.conf;

    // Reject negative or zero prices
    require!(price_i64 > 0, ErrorCode::InvalidOraclePrice);

    // Confidence interval check (prevent manipulation)
    //
    // If confidence range > 1% of price → price unreliable
    // (You may tighten or loosen this)
    //
    require!(
        conf_i64 < price_i64 / MAX_CONF_FACTOR,
        ErrorCode::OracleConfidenceTooHigh
    );

    // Scale price → convert to u64 with PRICE_PRECISION=1e6
    //
    // Pyth returns prices in 10^exponent scaling.
    // exponent usually = -8 for BTC, -6 for SOL etc.
    //
    let exponent = price_data.expo;

    let scaled_price = scale_price_to_precision(price_i64, exponent)?;

    Ok(scaled_price)
}

/// Convert Pyth price from exponent form into your PRICE_PRECISION (1e6)
pub fn scale_price_to_precision(price: i64, expo: i32) -> Result<u64> {
    // convert i64→i128 for safe math
    let mut v = price as i128;

    // Compute how much to shift from Pyth exponent to PRICE_PRECISION exponent.
    //
    // Example:
    // pyth exponent = -8
    // PRICE_PRECISION exponent = -6
    // need to divide by 10^( -6 - (-8) ) = 10^2
    //
    let target_exponent: i32 = -6; // because PRICE_PRECISION = 1e6
    let delta = target_exponent - expo;

    if delta < 0 {
        for _ in 0..delta {
            v = v
                .checked_mul(10)
                .ok_or(error!(ErrorCode::ArithmeticOverflow))?;
        }
    } else if delta > 0 {
        for _ in 0..(-delta) {
            v = v
                .checked_div(10)
                .ok_or(error!(ErrorCode::ArithmeticOverflow))?;
        }
    }

    // convert back to u64
    Ok(v.try_into().map_err(|_| error!(ErrorCode::ArithmeticOverflow))?)
}
