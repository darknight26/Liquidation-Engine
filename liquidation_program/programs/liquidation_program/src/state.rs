use anchor_lang::prelude::*;

#[event]
pub struct LiquidationEvent {
    pub position_owner: Pubkey,
    pub liquidator: Pubkey,
    pub symbol_id: u16,
    pub liquidated_size: u64,
    pub liquidation_price: u64,
    pub margin_before: i64,
    pub margin_after: i64,
    pub liquidator_reward: u64,
    pub bad_debt: u64,
    pub timestamp: i64,
}

#[event]
pub struct ProtocolInsolvencyEvent {
    pub amount: u64,
    pub timestamp: i64,
}


// MERGED ERROR CODES
#[error_code]
pub enum ErrorCode {
    #[msg("Invalid oracle account")]
    InvalidOracleAccount,
    #[msg("Stale oracle price")]
    StaleOraclePrice,
    #[msg("Oracle confidence too high")]
    OracleConfidenceTooHigh,
    #[msg("Invalid oracle price")]
    InvalidOraclePrice,
    #[msg("Arithmetic overflow/underflow")]
    ArithmeticOverflow,
    #[msg("Zero position")]
    ZeroPosition,
    #[msg("Too small to partial")]
    TooSmallToPartial,
    #[msg("Partial insufficient, full required")]
    PartialInsufficient,
    #[msg("Invalid vault bump")]
    InvalidVaultBump,
    #[msg("Invalid insurance bump")]
    InvalidInsuranceBump,
    #[msg("Unauthorized")]
    Unauthorized,
}