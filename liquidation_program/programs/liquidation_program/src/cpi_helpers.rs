use anchor_lang::prelude::*;
use anchor_spl::token::{self, Token, TokenAccount, Transfer};

pub fn token_transfer<'info>(
    from: AccountInfo<'info>,
    to: AccountInfo<'info>,
    authority: AccountInfo<'info>,
    token_program: AccountInfo<'info>,
    amount: u64,
) -> Result<()> {
    let cpi_accounts = Transfer {
        from,
        to,
        authority,
    };

    let cpi_ctx = CpiContext::new(token_program, cpi_accounts);

    token::transfer(cpi_ctx, amount)?;

    Ok(())
}

pub fn token_transfer_pda<'info>(
    from: AccountInfo<'info>,
    to: AccountInfo<'info>,
    authority: AccountInfo<'info>,   // PDA
    token_program: AccountInfo<'info>,
    amount: u64,
    authority_bump: u8,
    authority_seeds: &[&[u8]],
) -> Result<()> {
    let cpi_accounts = Transfer {
        from,
        to,
        authority,
    };

    let seeds = &[authority_seeds, &[&[authority_bump]]].concat();

    let signer = &[&seeds[..]];

    let cpi_ctx = CpiContext::new_with_signer(token_program, cpi_accounts, signer);

    token::transfer(cpi_ctx, amount)?;

    Ok(())
}


pub fn write_liquidation_record<'info>(
    ctx: Context<'_, '_, '_, 'info, CreateLiquidationRecord<'info>>,
    data: LiquidationRecord,
) -> Result<()> {
    let record = &mut ctx.accounts.record;
    *record = data;
    Ok(())
}