use anchor_lang::prelude::*;

declare_id!("3cVSJYSXY3yscUwcxrWR5sqoJ4Mcbu1qrQKRjgXbi5AS");

#[program]
pub mod liquidation_program {
    use super::*;

    pub fn initialize(ctx: Context<Initialize>) -> Result<()> {
        Ok(())
    }
}

#[derive(Accounts)]
pub struct Initialize {}
