use anchor_lang::prelude::*;
use anchor_spl::{
    associated_token::AssociatedToken,
    token::{self, CloseAccount, Mint, Token, TokenAccount, Transfer},
};

use crate::state::*;

use gem_common::errors::ErrorCode;
use gem_common::*;

#[derive(Accounts)]
#[instruction(bump: u8)]
pub struct WithdrawGem<'info> {
    // needed for checking flags
    pub bank: Box<Account<'info, Bank>>,
    // needed for seeds derivation
    #[account(mut, has_one = bank, has_one = owner, has_one = authority)]
    pub vault: Account<'info, Vault>,
    // this ensures only the owner can withdraw
    #[account(mut)]
    pub owner: Signer<'info>,
    // needed to sign token transfer
    pub authority: AccountInfo<'info>,
    #[account(mut,
        seeds = [
            b"gem_box".as_ref(),
            vault.key().as_ref(),
            gem_mint.key().as_ref(),
        ],
        bump = bump)]
    pub gem_box: Account<'info, TokenAccount>,
    #[account(mut)]
    pub gem_deposit_receipt: Box<Account<'info, GemDepositReceipt>>,
    #[account(init_if_needed,
        associated_token::mint = gem_mint,
        associated_token::authority = receiver,
        payer = owner)]
    pub gem_destination: Box<Account<'info, TokenAccount>>,
    pub gem_mint: Box<Account<'info, Mint>>,
    #[account(mut)]
    pub receiver: AccountInfo<'info>,
    pub token_program: Program<'info, Token>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}

impl<'info> WithdrawGem<'info> {
    fn transfer_ctx(&self) -> CpiContext<'_, '_, '_, 'info, Transfer<'info>> {
        CpiContext::new(
            self.token_program.to_account_info(),
            Transfer {
                from: self.gem_box.to_account_info(),
                to: self.gem_destination.to_account_info(),
                authority: self.authority.to_account_info(),
            },
        )
    }

    fn close_context(&self) -> CpiContext<'_, '_, '_, 'info, CloseAccount<'info>> {
        CpiContext::new(
            self.token_program.to_account_info(),
            CloseAccount {
                account: self.gem_box.to_account_info(),
                destination: self.receiver.to_account_info(),
                authority: self.authority.clone(),
            },
        )
    }
}

pub fn handler(ctx: Context<WithdrawGem>, amount: u64) -> ProgramResult {
    // verify vault not suspended
    let bank = &*ctx.accounts.bank;
    let vault = &ctx.accounts.vault;

    if vault.access_suspended(bank.flags)? {
        return Err(ErrorCode::VaultAccessSuspended.into());
    }

    // do the transfer
    token::transfer(
        ctx.accounts
            .transfer_ctx()
            .with_signer(&[&vault.vault_seeds()]),
        amount,
    )?;

    // update the gdr
    let gdr = &mut *ctx.accounts.gem_deposit_receipt;
    let gem_box = &ctx.accounts.gem_box;

    gdr.gem_amount.try_self_sub(amount)?;

    // this check is semi-useless but won't hurt
    if gdr.gem_amount != gem_box.amount - amount {
        return Err(ErrorCode::AmountMismatch.into());
    }

    // if gembox empty, close both the box and the GDR, and return funds to user
    if gdr.gem_amount == 0 {
        // close gem box
        token::close_account(
            ctx.accounts
                .close_context()
                .with_signer(&[&vault.vault_seeds()]),
        )?;

        // close GDR
        let receiver = &mut ctx.accounts.receiver;
        let gdr = &mut (*ctx.accounts.gem_deposit_receipt).to_account_info();

        close_account(gdr, receiver)?;

        // decrement gem box count stored in vault's state
        let vault = &mut ctx.accounts.vault;
        vault.gem_box_count.try_self_sub(1)?;
    }

    msg!("{} gems withdrawn from ${} gem box", amount, gem_box.key());
    Ok(())
}