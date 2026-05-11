use anchor_lang::prelude::*;
use anchor_spl::token_2022::Token2022;
use anchor_spl::token_interface::{transfer_checked, Mint, TokenAccount, TransferChecked};

use crate::constants::{MARKET_SEED, USER_ACCOUNT_SEED};
use crate::error::ExchangeError;
use crate::state::{Market, UserAccount};

#[derive(Accounts)]
pub struct Withdraw<'info> {
    #[account(mut)]
    pub user: Signer<'info>,

    #[account(
        seeds = [MARKET_SEED, market.load()?.base_mint.as_ref(), market.load()?.quote_mint.as_ref()],
        bump = market.load()?.bump,
    )]
    pub market: AccountLoader<'info, Market>,

    #[account(
        mut,
        seeds = [USER_ACCOUNT_SEED, market.key().as_ref(), user.key().as_ref()],
        bump = user_account.load()?.bump,
    )]
    pub user_account: AccountLoader<'info, UserAccount>,

    #[account(
        mut,
        token::mint = mint,
        token::authority = user,
    )]
    pub user_token_account: InterfaceAccount<'info, TokenAccount>,

    #[account(mut)]
    pub vault: InterfaceAccount<'info, TokenAccount>,

    pub mint: InterfaceAccount<'info, Mint>,

    pub token_program: Program<'info, Token2022>,
}

pub(crate) fn handler(ctx: Context<Withdraw>, amount: u64) -> Result<()> {
    let mint_key = ctx.accounts.mint.key();
    let vault_key = ctx.accounts.vault.key();

    let (base_mint, quote_mint, base_vault, quote_vault, market_bump) = {
        let market = ctx.accounts.market.load()?;
        (
            market.base_mint,
            market.quote_mint,
            market.base_vault,
            market.quote_vault,
            market.bump,
        )
    };

    let (expected_vault, is_base) = if mint_key == base_mint {
        (base_vault, true)
    } else if mint_key == quote_mint {
        (quote_vault, false)
    } else {
        return err!(ExchangeError::WrongMint);
    };
    require_keys_eq!(vault_key, expected_vault, ExchangeError::WrongMint);

    {
        let mut user_account = ctx.accounts.user_account.load_mut()?;
        let balance = if is_base {
            &mut user_account.base_free
        } else {
            &mut user_account.quote_free
        };
        *balance = balance
            .checked_sub(amount)
            .ok_or(ExchangeError::InsufficientFreeBalance)?;
    }

    let signer_seeds: &[&[&[u8]]] = &[&[
        MARKET_SEED,
        base_mint.as_ref(),
        quote_mint.as_ref(),
        std::slice::from_ref(&market_bump),
    ]];

    let cpi_accounts = TransferChecked {
        from: ctx.accounts.vault.to_account_info(),
        mint: ctx.accounts.mint.to_account_info(),
        to: ctx.accounts.user_token_account.to_account_info(),
        authority: ctx.accounts.market.to_account_info(),
    };
    let cpi_ctx =
        CpiContext::new_with_signer(ctx.accounts.token_program.key(), cpi_accounts, signer_seeds);
    transfer_checked(cpi_ctx, amount, ctx.accounts.mint.decimals)?;

    Ok(())
}
