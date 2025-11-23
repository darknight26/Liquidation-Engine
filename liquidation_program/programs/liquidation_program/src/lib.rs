use anchor_lang::prelude::*;
use anchor_spl::token::{self, Token, TokenAccount, Transfer};
use std::convert::TryInto;


pub mod math;
pub mod oracle;
pub mod state;
pub mod cpi_helpers;

use crate::math::*;
use crate::oracle::*;
use crate::state::*;
use crate::cpi_helpers::{token_transfer_pda};

declare_id!("3cVSJYSXY3yscUwcxrWR5sqoJ4Mcbu1qrQKRjgXbi5AS");

#[program]
pub mod liquidation_program {
    use super::*;

    pub fn initialize_insurance_fund(
        ctx: Context<initializeInsuranceFund>,
        authority: Pubkey) -> Result<()>{
            let fund = &mut ctx.accounts.insurance_fund;
            fund.authority = authority;
            fund.balance = 0;
            fund.total_bad_debt_covered = 0;
            fund.total_contributions = 0;
            fund.utilization_ratio = 0;
            Ok(())
    }

    pub fn create_position(
        ctx: Context<CreatePosition>,
        entry_price: u64,
        size:u64,
        collateral:i64,
        is_long:bool,
        leverage:u16,
    )-> Result<()>{
        let pos =&mut ctx.accounts.position;
        pos.owner = ctx.accounts.owner.key();
        pos.entry_price = entry_price;
        pos.size = size;
        pos.collateral = collateral;
        pos.is_long = is_long;
        pos.leverage = leverage;
        pos.last_update_ts = Clock::get()?.unix_timestamp;
        Ok(())
    }
    
    pub fn get_maintenance_margin_bps(leverage: u16) -> Result<u64> {
        let bps = match leverage {
            1..=20 => 250,      // 2.5%
            21..=50 => 100,     // 1.0%
            51..=100 => 50,     // 0.5%
            101..=500 => 25,    // 0.25%
            501..=1000 => 10,   // 0.1%
            _ => 250,           // safest default (2.5%)
        };

        Ok(bps)
    }


    pub fn write_liquidation_record(
        ctx: Context<WriteLiquidationRecord>,
        position_owner: Pubkey,
        liquidator: Pubkey,
        symbol_id: u16,
        liquidated_size: u64,
        liquidation_price: u64,
        margin_before: i64,
        margin_after: i64,
        liquidator_reward: u64,
        bad_debt: u64,
        timestamp: i64,
    ) -> Result<()> {

        let record = &mut ctx.accounts.liquidation_record;

        record.position_owner = position_owner;
        record.liquidator = liquidator;
        record.symbol_id = symbol_id;
        record.liquidated_size = liquidated_size;
        record.liquidation_price = liquidation_price;
        record.margin_before = margin_before;
        record.margin_after = margin_after;
        record.liquidator_reward = liquidator_reward;
        record.bad_debt = bad_debt;
        record.timestamp = timestamp;

        Ok(())
    }



    // Partial liquidation
    pub fn liquidate_partial(ctx: Context<LiquidatePartial>) -> Result<()> {
        use crate::math::*;
        // constants (i128)
        const PRICE_PRECISION_I128: i128 = PRICE_PRECISION as i128;
        const BPS_DENOM_I128: i128 = BPS_DENOM as i128;
        let reward_bps_i128: i128 = LIQUIDATOR_REWARD_BPS as i128;

        // bump for vault authority signing
        let vault_bump = *ctx
            .bumps
            .get("vault_authority")
            .ok_or(error!(ErrorCode::InvalidVaultBump))?;

        // load accounts
        let pos = &mut ctx.accounts.position;
        let fund = &mut ctx.accounts.insurance_fund;
        let liquidator = &ctx.accounts.liquidator;

        // price
        let P_u64 = get_oracle_price(&ctx.accounts.oracle)?;
        let P_i128: i128 = P_u64 as i128;
        require!(P_i128 > 0, ErrorCode::InvalidOraclePrice);

        // position fields
        let E_i128: i128 = pos.entry_price as i128;
        let Q_i128: i128 = pos.size as i128;
        let C_i128: i128 = pos.collateral as i128;
        require!(Q_i128 > 0, ErrorCode::ZeroPosition);

        // notional N = Q * P / PRICE_PRECISION
        let N_i128 = if Q_i128 == 0 {
            0
        } else {
            (Q_i128
                .checked_mul(P_i128)
                .ok_or(error!(ErrorCode::ArithmeticOverflow))?)
            .checked_div(PRICE_PRECISION_I128)
            .ok_or(error!(ErrorCode::ArithmeticOverflow))?
        };

        // per-unit pnl (long/short)
        let raw_pnl_per_unit = if pos.is_long {
            P_i128.checked_sub(E_i128)
        } else {
            E_i128.checked_sub(P_i128)
        }
        .ok_or(error!(ErrorCode::ArithmeticOverflow))?;

        // current upl
        let upl_i128 = raw_pnl_per_unit
            .checked_mul(Q_i128).ok_or(error!(ErrorCode::ArithmeticOverflow))?
            .checked_div(PRICE_PRECISION_I128).ok_or(error!(ErrorCode::ArithmeticOverflow))?;

        // margin and ratio
        let margin_i128 = C_i128.checked_add(upl_i128).ok_or(error!(ErrorCode::ArithmeticOverflow))?;
        let margin_ratio_bps_i128 = if N_i128 == 0 {
            i128::MAX
        } else {
            (margin_i128
                .checked_mul(BPS_DENOM_I128).ok_or(error!(ErrorCode::ArithmeticOverflow))?)
                .checked_div(N_i128).ok_or(error!(ErrorCode::ArithmeticOverflow))?
        };

        // check healthy
        let mmr_bps_i128 = get_maintenance_margin_bps(pos.leverage)? as i128;
        if margin_i128 > 0 && margin_ratio_bps_i128 >= mmr_bps_i128 {
            return Ok(());
        }

        // closed quantity (50% partial)
        let closed_qty_i128 = (pos.size / 2) as i128;
        if closed_qty_i128 <= 0 {
            return Err(error!(ErrorCode::TooSmallToPartial));
        }

        // proceeds from selling closed_qty: closed_qty * P / PRICE_PRECISION
        let proceeds_i128 = (closed_qty_i128
            .checked_mul(P_i128)
            .ok_or(error!(ErrorCode::ArithmeticOverflow))?)
        .checked_div(PRICE_PRECISION_I128)
        .ok_or(error!(ErrorCode::ArithmeticOverflow))?;

        // liquidator reward from proceeds
        let reward_i128 = (proceeds_i128
            .checked_mul(reward_bps_i128)
            .ok_or(error!(ErrorCode::ArithmeticOverflow))?)
        .checked_div(BPS_DENOM_I128)
        .ok_or(error!(ErrorCode::ArithmeticOverflow))?;

        // net proceeds
        let net_proceeds_i128 = proceeds_i128.checked_sub(reward_i128).ok_or(error!(ErrorCode::ArithmeticOverflow))?;

        // remaining qty, new notional & new upl (use same per-unit pnl sign)
        let remaining_qty_i128 = Q_i128.checked_sub(closed_qty_i128).ok_or(error!(ErrorCode::ArithmeticOverflow))?;
        let new_notional_i128 = (remaining_qty_i128
            .checked_mul(P_i128)
            .ok_or(error!(ErrorCode::ArithmeticOverflow))?)
        .checked_div(PRICE_PRECISION_I128)
        .ok_or(error!(ErrorCode::ArithmeticOverflow))?;

        let new_upl_i128 = raw_pnl_per_unit
            .checked_mul(remaining_qty_i128)
            .ok_or(error!(ErrorCode::ArithmeticOverflow))?
            .checked_div(PRICE_PRECISION_I128)
            .ok_or(error!(ErrorCode::ArithmeticOverflow))?;

        // new margin after crediting net proceeds (collateral + new_upl + net_proceeds)
        let new_margin_i128 = C_i128
            .checked_add(new_upl_i128)
            .ok_or(error!(ErrorCode::ArithmeticOverflow))?
            .checked_add(net_proceeds_i128)
            .ok_or(error!(ErrorCode::ArithmeticOverflow))?;

        // new margin ratio
        let new_margin_ratio_bps_i128 = if new_notional_i128 == 0 {
            i128::MAX
        } else {
            (new_margin_i128
                .checked_mul(BPS_DENOM_I128)
                .ok_or(error!(ErrorCode::ArithmeticOverflow))?)
            .checked_div(new_notional_i128)
            .ok_or(error!(ErrorCode::ArithmeticOverflow))?
        };

        // decide: execute partial only if new state healthy
        if new_margin_i128 > 0 && new_margin_ratio_bps_i128 >= mmr_bps_i128 {
            // execute partial atomically
            pos.size = remaining_qty_i128 as u64;

            // **Important**: update collateral to the new margin (no artificial 'entry cost removal')
            let new_collateral_i128 = new_margin_i128;
            pos.collateral = new_collateral_i128.try_into().map_err(|_| error!(ErrorCode::ArithmeticOverflow))?;
            pos.last_update_ts = Clock::get()?.unix_timestamp;

            // transfer tokens from protocol_vault to liquidator + trader (only if amounts positive)
            if reward_i128 > 0 {
                let reward_u64: u64 = reward_i128.try_into().map_err(|_| error!(ErrorCode::ArithmeticOverflow))?;
                token_transfer_pda(
                    ctx.accounts.protocol_vault.to_account_info(),
                    ctx.accounts.liquidator_token_account.to_account_info(),
                    ctx.accounts.vault_authority.to_account_info(),
                    ctx.accounts.token_program.to_account_info(),
                    reward_u64,
                    vault_bump,
                    &[VAULT_AUTH_SEED, VAULT_SEED],
                )?;
            }

            if net_proceeds_i128 > 0 {
                let net_u64: u64 = net_proceeds_i128.try_into().map_err(|_| error!(ErrorCode::ArithmeticOverflow))?;
                token_transfer_pda(
                    ctx.accounts.protocol_vault.to_account_info(),
                    ctx.accounts.trader_token_account.to_account_info(),
                    ctx.accounts.vault_authority.to_account_info(),
                    ctx.accounts.token_program.to_account_info(),
                    net_u64,
                    vault_bump,
                    &[VAULT_AUTH_SEED, VAULT_SEED],
                )?;
            }

            emit!(LiquidationEvent {
                position_owner: pos.owner,
                liquidator: liquidator.key(),
                symbol_id: 0u16,
                liquidated_size: closed_qty_i128 as u64,
                liquidation_price: P_u64,
                margin_before: margin_i128 as i64,
                margin_after: new_collateral_i128 as i64,
                liquidator_reward: reward_i128.try_into().unwrap_or(0),
                bad_debt: 0u64,
                timestamp: Clock::get()?.unix_timestamp,
            });


            // write to liquidation record
            let clock = Clock::get()?;
            let ts = clock.unix_timestamp;

            let symbol_bytes = {
                let mut arr = [0u8; 16];
                let sym = b"BTCUSD";   // replace with real symbol
                arr[..sym.len()].copy_from_slice(sym);
                arr
            };

            let record_data = LiquidationRecord {
                position_owner: pos.owner,
                liquidator: liquidator.key(),
                symbol: symbol_bytes,
                liquidated_size: closed_qty_i128 as u64,
                liquidation_price: P_u64,
                margin_before: margin_i128 as i64,
                margin_after: new_collateral_i128 as i64,
                liquidator_reward: reward_i128 as u64,
                bad_debt: 0,
                timestamp: ts,
            };

            let cpi_ctx = Context::new(
                ctx.program_id,
                CreateLiquidationRecord {
                    record: ctx.accounts.liq_record.to_account_info(),
                    payer: ctx.accounts.liquidator.to_account_info(),
                    system_program: ctx.accounts.system_program.to_account_info(),
                },
                vec![],
            );

            write_liquidation_record(cpi_ctx, record_data)?;

            return Ok(());
        } else {
            return Err(error!(ErrorCode::PartialInsufficient));
        }
    }

    pub fn liquidate_full(ctx: Context<LiquidateFull>) -> Result<()> {
        
        const PRICE_PRECISION_I128: i128 = PRICE_PRECISION as i128;
        const BPS_DENOM_I128: i128 = BPS_DENOM as i128;
        let reward_bps_i128: i128 = LIQUIDATOR_REWARD_BPS as i128;

        // load accounts
        let pos = &mut ctx.accounts.position;
        let fund = &mut ctx.accounts.insurance_fund;
        let liquidator = &ctx.accounts.liquidator;

        // vault bump for PDA signing
        let vault_bump = *ctx
            .bumps
            .get("vault_authority")
            .ok_or(error!(ErrorCode::InvalidVaultBump))?;

        // insurance bump
        let insurance_bump = *ctx
            .bumps
            .get("insurance_authority")
            .ok_or(error!(ErrorCode::InvalidInsuranceBump))?;

        // read oracle price
        let P_u64 = get_oracle_price(&ctx.accounts.oracle)?;
        let P_i128: i128 = P_u64 as i128;
        require!(P_i128 > 0, ErrorCode::InvalidOraclePrice);

        // fields
        let E_i128: i128 = pos.entry_price as i128;
        let Q_i128: i128 = pos.size as i128;
        let C_i128: i128 = pos.collateral as i128; // current collateral (signed)
        require!(Q_i128 > 0, ErrorCode::ZeroPosition);

        // per-unit pnl
        let raw_pnl_per_unit = if pos.is_long {
            P_i128.checked_sub(E_i128).ok_or(error!(ErrorCode::ArithmeticOverflow))?
        } else {
            E_i128.checked_sub(P_i128).ok_or(error!(ErrorCode::ArithmeticOverflow))?
        };

        // upl
        let upl_i128 = raw_pnl_per_unit
            .checked_mul(Q_i128).ok_or(error!(ErrorCode::ArithmeticOverflow))?
            .checked_div(PRICE_PRECISION_I128).ok_or(error!(ErrorCode::ArithmeticOverflow))?;

        //notional
        let notional_i128 = (
            Q_i128
            .checked_mul(P_i128).ok_or(error!(ErrorCode::ArithmeticOverflow))?)
            .checked_div(PRICE_PRECISION_I128).ok_or(error!(ErrorCode::ArithmeticOverflow))?;

        // final margin after closing position 
        let final_margin_i128 = C_i128.checked_add(upl_i128).ok_or(error!(ErrorCode::ArithmeticOverflow))?;


        // If final_margin > 0 => leftover exists and liquidator gets reward
        if final_margin_i128 >= 0 {
            // compute liquidator reward (from final_margin)
            let reward_i128 = (final_margin_i128
                .checked_mul(reward_bps_i128).ok_or(error!(ErrorCode::ArithmeticOverflow))?)
                .checked_div(BPS_DENOM_I128).ok_or(error!(ErrorCode::ArithmeticOverflow))?;

            // remaining to user after reward
            let remaining_i128 = final_margin_i128
                .checked_sub(reward_i128).ok_or(error!(ErrorCode::ArithmeticOverflow))?;

            // update position: close
            pos.size = 0;
            pos.collateral = 0; // cleared (we will record leftover/transfer via CPI)
            pos.last_update_ts = Clock::get()?.unix_timestamp;

            // transfer reward from protocol_vault
            if reward_i128 > 0 {
                let reward_u64: u64 = reward_i128.try_into().map_err(|_| error!(ErrorCode::ArithmeticOverflow))?;
                token_transfer_pda(
                    ctx.accounts.protocol_vault.to_account_info(),
                    ctx.accounts.liquidator_token_account.to_account_info(),
                    ctx.accounts.vault_authority.to_account_info(),
                    ctx.accounts.token_program.to_account_info(),
                    reward_u64,
                    vault_bump,
                    &[VAULT_AUTH_SEED, VAULT_SEED],
                )?;
            }

            // transfer remainder to trader if >0
            if remaining_i128 > 0 {
                let rem_u64: u64 = remaining_i128.try_into().map_err(|_| error!(ErrorCode::ArithmeticOverflow))?;
                token_transfer_pda(
                    ctx.accounts.protocol_vault.to_account_info(),
                    ctx.accounts.trader_token_account.to_account_info(),
                    ctx.accounts.vault_authority.to_account_info(),
                    ctx.accounts.token_program.to_account_info(),
                    rem_u64,
                    vault_bump,
                    &[VAULT_AUTH_SEED, VAULT_SEED],
                )?;
            }

            // Emit event (bad_debt = 0)
            emit!(LiquidationEvent {
                position_owner: pos.owner,
                liquidator: liquidator.key(),
                symbol_id: 0u16,
                liquidated_size: Q_i128 as u64,
                liquidation_price: P_u64,
                margin_before: (C_i128.checked_add(upl_i128).ok_or(error!(ErrorCode::ArithmeticOverflow))?) as i64,
                margin_after: remaining_i128 as i64,
                liquidator_reward: reward_i128 as u64,
                bad_debt: 0u64,
                timestamp: Clock::get()?.unix_timestamp,
            });

            let ts = Clock::get()?.unix_timestamp;

            let mut symbol_bytes = [0u8; 16];
            let sym = b"BTCUSD";
            symbol_bytes[..sym.len()].copy_from_slice(sym);

            let record_data = LiquidationRecord {
                position_owner: pos.owner,
                liquidator: liquidator.key(),
                symbol: symbol_bytes,
                liquidated_size: Q_i128 as u64,
                liquidation_price: P_u64,
                margin_before: (C_i128 + upl_i128) as i64,
                margin_after: 0,
                liquidator_reward: reward_paid_i128 as u64,
                bad_debt: leftover_bad_debt_i128 as u64,
                timestamp: ts,
            };

            let cpi_ctx = Context::new(
                ctx.program_id,
                CreateLiquidationRecord {
                    record: ctx.accounts.liq_record.to_account_info(),
                    payer: ctx.accounts.liquidator.to_account_info(),
                    system_program: ctx.accounts.system_program.to_account_info(),
                },
                vec![],
            );

            write_liquidation_record(cpi_ctx, record_data)?;


            return Ok(());
        }

         // final_margin < 0 -> bad debt
        let bad_debt_i128 = final_margin_i128.checked_neg().ok_or(error!(ErrorCode::ArithmeticOverflow))?;

        // try to cover from insurance vault (real tokens)
        // The insurance vault must hold tokens; fund.balance is bookkeeping derived from real vault.
        let insurance_balance_i128 = ctx.accounts.insurance_vault.amount as i128;

        let mut bad_debt_covered_i128: i128 = 0;
        let mut leftover_bad_debt_i128: i128 = bad_debt_i128;

        if insurance_balance_i128 >= bad_debt_i128 {
            bad_debt_covered_i128 = bad_debt_i128;
            // transfer actual tokens from insurance_vault -> protocol_vault (or directly to liquidator/trader as needed)
            // We'll transfer to liquidator as reward and any remainder adjust fund accounting
            // for simplicity: move bad_debt_covered to protocol_vault, then reimburse parties
            // but better: pay liquidator reward from insurance directly
            // Here we just deduct bookkeeping and leave transfer to reward below.
            // Subtract from insurance_vault physically by moving to protocol_vault (optional)
            // We'll keep it simple: reduce insurance.vault bookkeeping and later transfer reward only.
            // Update insurance fund accounting
            // (We do not do a big transfer of bad debt dollars to traders here)
            // Instead, we'll use fund.balance bookkeeping to record coverage.
            let new_balance = insurance_balance_i128.checked_sub(bad_debt_covered_i128).ok_or(error!(ErrorCode::ArithmeticOverflow))?;
            // For bookkeeping: set fund.balance to new_balance (u64)
            fund.balance = new_balance.try_into().map_err(|_| error!(ErrorCode::ArithmeticOverflow))?;
            leftover_bad_debt_i128 = 0;
        } else {
            // partial cover
            bad_debt_covered_i128 = insurance_balance_i128;
            leftover_bad_debt_i128 = bad_debt_i128.checked_sub(insurance_balance_i128).ok_or(error!(ErrorCode::ArithmeticOverflow))?;
            fund.balance = 0;
        }

        fund.total_bad_debt_covered = fund.total_bad_debt_covered.checked_add(bad_debt_covered_i128 as u64).ok_or(error!(ErrorCode::ArithmeticOverflow))?;

        // close position numerically
        pos.size = 0;
        pos.collateral = 0;
        pos.last_update_ts = Clock::get()?.unix_timestamp;

        // reward calculation: pay reward from covered amount proportionally
        let max_reward_i128 = (bad_debt_i128.checked_mul(reward_bps_i128).ok_or(error!(ErrorCode::ArithmeticOverflow))?).checked_div(BPS_DENOM_I128).ok_or(error!(ErrorCode::ArithmeticOverflow))?;
        let reward_paid_i128 = if bad_debt_covered_i128 > 0 {
            if bad_debt_covered_i128 >= max_reward_i128 { max_reward_i128 } else { bad_debt_covered_i128 }
        } else {
            0
        };

        if reward_paid_i128 > 0 {
            let reward_paid_u64: u64 = reward_paid_i128.try_into().map_err(|_| error!(ErrorCode::ArithmeticOverflow))?;
            // pay liquidator from insurance_vault
            token_transfer_pda(
                ctx.accounts.insurance_vault.to_account_info(),
                ctx.accounts.liquidator_token_account.to_account_info(),
                ctx.accounts.insurance_authority.to_account_info(),
                ctx.accounts.token_program.to_account_info(),
                reward_paid_u64,
                insurance_bump,
                &[INSURANCE_AUTH_SEED, INSURANCE_SEED],
            )?;
        }

        // bookkeeping event
        emit!(LiquidationEvent {
            position_owner: pos.owner,
            liquidator: liquidator.key(),
            symbol_id: 0u16,
            liquidated_size: Q_i128 as u64,
            liquidation_price: P_u64,
            margin_before: (C_i128.checked_add(upl_i128).ok_or(error!(ErrorCode::ArithmeticOverflow))?) as i64,
            margin_after: 0i64,
            liquidator_reward: reward_paid_i128.try_into().unwrap_or(0),
            bad_debt: leftover_bad_debt_i128 as u64,
            timestamp: Clock::get()?.unix_timestamp,
        });

        let ts = Clock::get()?.unix_timestamp;

        let mut symbol_bytes = [0u8; 16];
        let sym = b"BTCUSD";
        symbol_bytes[..sym.len()].copy_from_slice(sym);

        let record_data = LiquidationRecord {
            position_owner: pos.owner,
            liquidator: liquidator.key(),
            symbol: symbol_bytes,
            liquidated_size: Q_i128 as u64,
            liquidation_price: P_u64,
            margin_before: (C_i128 + upl_i128) as i64,
            margin_after: 0,
            liquidator_reward: reward_paid_i128 as u64,
            bad_debt: leftover_bad_debt_i128 as u64,
            timestamp: ts,
        };

        let cpi_ctx = Context::new(
            ctx.program_id,
            CreateLiquidationRecord {
                record: ctx.accounts.liq_record.to_account_info(),
                payer: ctx.accounts.liquidator.to_account_info(),
                system_program: ctx.accounts.system_program.to_account_info(),
            },
            vec![],
        );

        write_liquidation_record(cpi_ctx, record_data)?;


        if leftover_bad_debt_i128 > 0 {
            emit!(ProtocolInsolvencyEvent {
                amount: leftover_bad_debt_i128 as u64,
                timestamp: Clock::get()?.unix_timestamp,
            });
        }


        Ok(())
    }
}




#[derive(Accounts)]
pub struct InitializeInsuranceFund<'info> {
    #[account(init, payer = payer, space = InsuranceFund::LEN)]
    pub insurance_fund: Account<'info, InsuranceFund>,

    #[account(
        init,
        payer = payer,
        token::mint = mint,
        token::authority = insurance_authority,
        seeds = [INSURANCE_SEED],
        bump,
    )]
    pub insurance_vault: Account<'info, TokenAccount>,

    /// CHECK: PDA authority for insurance_vault
    #[account(
        seeds = [INSURANCE_AUTH_SEED, INSURANCE_SEED],
        bump
    )]
    pub insurance_authority: UncheckedAccount<'info>,

    #[account(mut)]
    pub payer: Signer<'info>,

    /// CHECK: mint for token (e.g., USDC)
    pub mint: UncheckedAccount<'info>,

    pub system_program: Program<'info, System>,
    pub token_program: Program<'info, Token>,
    pub rent: Sysvar<'info, Rent>,
}




#[derive(Accounts)]
pub struct CreatePosition<'info> {
    #[account(init , payer = owner , space = Position::LEN)]
    pub position: Account<'info,Position>,

    #[account(mut)]
    pub owner: Signer<'info>,
    pub system_program: Program<'info,System>,
}





#[derive(Accounts)]
pub struct LiquidatePartial<'info> {

    #[account(mut)]
    pub position: Account<'info, Position>,

    #[account(mut)]
    pub insurance_fund: Account<'info, InsuranceFund>,

    // SPL token vault owned by PDA
    #[account(
        mut,
        seeds = [VAULT_SEED],
        bump,
    )]
    pub protocol_vault: Account<'info, TokenAccount>,

    // PDA that signs all SPL CPI transfers from protocol_vault
    #[account(
        seeds = [VAULT_SEED],
        bump
    )]
    pub vault_authority: UncheckedAccount<'info>,

    // Liquidator receives rewards
    #[account(mut)]
    pub liquidator_token_account: Account<'info, TokenAccount>,

    // Trader receives leftover margin
    #[account(mut)]
    pub trader_token_account: Account<'info, TokenAccount>,

    #[account(signer)]
    pub liquidator: Signer<'info>,

    /// CHECK: Oracle account; validated in logic
    pub oracle: UncheckedAccount<'info>,

    pub token_program: Program<'info, Token>,

    pub liquidation_record: Account<'info, LiquidationRecord>,

    #[account()]
    pub system_program: Program<'info, System>,
}




#[derive(Accounts)]
pub struct LiquidateFull<'info> {
    #[account(mut)]
    pub position: Account<'info, Position>,

    #[account(mut)]
    pub insurance_fund: Account<'info, InsuranceFund>,

    // SPL token vault owned by PDA (holds protocol collateral + insurance)
    #[account(
        mut,
        seeds = [VAULT_SEED],
        bump,
    )]
    pub protocol_vault: Account<'info, TokenAccount>,

    // PDA that signs SPL CPI transfers from protocol_vault
    #[account(
        seeds = [VAULT_SEED],
        bump
    )]
    pub vault_authority: UncheckedAccount<'info>,

    // Liquidator receives rewards
    #[account(mut)]
    pub liquidator_token_account: Account<'info, TokenAccount>,

    // Trader receives leftover margin (if any)
    #[account(mut)]
    pub trader_token_account: Account<'info, TokenAccount>,

    #[account(signer)]
    pub liquidator: Signer<'info>,

    /// CHECK: Oracle account (Pyth). validated in program logic
    pub oracle: UncheckedAccount<'info>,

    pub token_program: Program<'info, Token>,

    pub liquidation_record: Account<'info, LiquidationRecord>,
}


#[derive(Accounts)]
pub struct WriteLiquidationRecord<'info> {
    #[account(
        init,
        payer = payer,
        space = LiquidationRecord::LEN,
        seeds = [
            b"record",
            position_owner.key().as_ref(),
            &timestamp.to_le_bytes(),
        ],
        bump
    )]
    pub liquidation_record: Account<'info, LiquidationRecord>,

    #[account(mut)]
    pub payer: Signer<'info>,

    pub system_program: Program<'info, System>,
}




#[account]
pub struct Position {
    // Trader who owns the position
    pub owner: Pubkey,

    // Base asset size (e.g., number of contracts)
    pub size: u64,          // 0 means closed

    // Entry price of the position (fixed point => multiplied by 1e6)
    pub entry_price: u64,   // PRICE_PRECISION = 1e6

    // Collateral associated with this position.
    // Signed because negative margin can occur before liquidation.
    pub collateral: i64,

    // True if long, false if short
    pub is_long: bool,

    // Timestamp of last update (important for cooldown)
    pub last_update_ts: i64,

    // Cached leverage for easier margin checks
    pub leverage: u16,      // e.g., 100 = 100x

    // Padding for account alignment (optional)
    pub padding: [u8; 5],
}

impl Position {
    pub const LEN: usize = 8 + 32 + 8 + 8 + 8 + 1 + 8 + 2 + 5;
}




#[account]
pub struct LiquidationRecord {
    pub position_owner: Pubkey,       // 32
    pub liquidator: Pubkey,           // 32
    pub symbol: [u8; 16],            // 16
    pub liquidated_size: u64,         // 8
    pub liquidation_price: u64,       // 8
    pub margin_before: i64,           // 8 (should be signed!)
    pub margin_after: i64,            // 8
    pub liquidator_reward: u64,       // 8
    pub bad_debt: u64,                // 8
    pub timestamp: i64,               // 8
}
impl LiquidationRecord {
    pub const LEN: usize = 8 + 32 + 32 + 16 + 8 + 8 + 8 + 8 + 8 + 8 + 8;
}



#[account]
pub struct InsuranceFund {
    pub authority: Pubkey,             // 32
    pub balance: u64,                  // 8
    pub total_contributions: u64,      // 8
    pub total_bad_debt_covered: u64,   // 8
    pub utilization_ratio: u64,        // 8
}

impl InsuranceFund {
    pub const LEN: usize = 8 + 32 + 8 + 8 + 8 + 8;
}
