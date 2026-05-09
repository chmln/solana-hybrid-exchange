//! Tests for the `deposit` and `withdraw` instructions.

mod common;

use {
    anchor_lang::{
        prelude::Pubkey, solana_program::instruction::Instruction, AccountDeserialize,
        InstructionData, ToAccountMetas,
    },
    common::*,
    litesvm::{types::FailedTransactionMetadata, LiteSVM},
    solana_hybrid_exchange::{
        accounts as program_accounts, instruction as program_ix,
        state::{Market, UserAccount},
    },
    solana_keypair::Keypair,
    solana_signer::Signer,
};

const RENT_SYSVAR_ID: Pubkey = anchor_lang::pubkey!("SysvarRent111111111111111111111111111111111");

// Anchor user error codes start at 6000; ordering must match `src/error.rs`.
const ERR_WRONG_MINT: u32 = 6002;
const ERR_INSUFFICIENT_FREE_BALANCE: u32 = 6003;

fn fresh_svm() -> LiteSVM {
    let mut svm = LiteSVM::new();
    svm.add_program(
        solana_hybrid_exchange::id(),
        include_bytes!("../../../target/deploy/solana_hybrid_exchange.so"),
    )
    .unwrap();
    svm
}

fn has_custom_error(meta: &FailedTransactionMetadata, code: u32) -> bool {
    let dbg = format!("{:?}", meta.err);
    if dbg.contains(&format!("Custom({code})")) {
        return true;
    }
    let needle = format!("Error Number: {code}");
    meta.meta.logs.iter().any(|l| l.contains(&needle))
}

#[allow(dead_code)]
struct MarketCtx {
    svm: LiteSVM,
    admin: Keypair,
    mint_authority: Keypair,
    base_mint: Pubkey,
    quote_mint: Pubkey,
    market: Pubkey,
    base_vault: Pubkey,
    quote_vault: Pubkey,
}

fn setup_market() -> MarketCtx {
    let mut svm = fresh_svm();

    let admin = Keypair::new();
    svm.airdrop(&admin.pubkey(), 100_000_000_000).unwrap();
    let mint_authority = Keypair::new();
    svm.airdrop(&mint_authority.pubkey(), 100_000_000_000)
        .unwrap();

    let base_mint = create_mint(&mut svm, &admin, 9, &mint_authority.pubkey(), &[]);
    let quote_mint = create_mint(&mut svm, &admin, 6, &mint_authority.pubkey(), &[]);

    let (market, _bump) = market_pda(&base_mint, &quote_mint);
    let base_vault_kp = Keypair::new();
    let quote_vault_kp = Keypair::new();

    let accounts = program_accounts::InitMarket {
        admin: admin.pubkey(),
        base_mint,
        quote_mint,
        market,
        base_vault: base_vault_kp.pubkey(),
        quote_vault: quote_vault_kp.pubkey(),
        token_program: spl_token_2022::id(),
        system_program: SYSTEM_PROGRAM_ID,
        rent: RENT_SYSVAR_ID,
    };
    let ix = Instruction::new_with_bytes(
        solana_hybrid_exchange::id(),
        &program_ix::InitMarket {}.data(),
        accounts.to_account_metas(None),
    );
    send_tx(&mut svm, &[ix], &[&admin, &base_vault_kp, &quote_vault_kp])
        .expect("init_market failed");

    // Sanity-check vaults match what we passed in.
    let market_acc = svm.get_account(&market).expect("market missing");
    let parsed = Market::try_deserialize(&mut market_acc.data.as_slice()).unwrap();
    assert_eq!(parsed.base_vault, base_vault_kp.pubkey());
    assert_eq!(parsed.quote_vault, quote_vault_kp.pubkey());

    MarketCtx {
        svm,
        admin,
        mint_authority,
        base_mint,
        quote_mint,
        market,
        base_vault: base_vault_kp.pubkey(),
        quote_vault: quote_vault_kp.pubkey(),
    }
}

fn build_deposit_ix(
    user: &Pubkey,
    market: &Pubkey,
    user_account: &Pubkey,
    user_token_account: &Pubkey,
    vault: &Pubkey,
    mint: &Pubkey,
    amount: u64,
) -> Instruction {
    let accounts = program_accounts::Deposit {
        user: *user,
        market: *market,
        user_account: *user_account,
        user_token_account: *user_token_account,
        vault: *vault,
        mint: *mint,
        token_program: spl_token_2022::id(),
        system_program: SYSTEM_PROGRAM_ID,
    };
    Instruction::new_with_bytes(
        solana_hybrid_exchange::id(),
        &program_ix::Deposit { amount }.data(),
        accounts.to_account_metas(None),
    )
}

fn build_withdraw_ix(
    user: &Pubkey,
    market: &Pubkey,
    user_account: &Pubkey,
    user_token_account: &Pubkey,
    vault: &Pubkey,
    mint: &Pubkey,
    amount: u64,
) -> Instruction {
    let accounts = program_accounts::Withdraw {
        user: *user,
        market: *market,
        user_account: *user_account,
        user_token_account: *user_token_account,
        vault: *vault,
        mint: *mint,
        token_program: spl_token_2022::id(),
    };
    Instruction::new_with_bytes(
        solana_hybrid_exchange::id(),
        &program_ix::Withdraw { amount }.data(),
        accounts.to_account_metas(None),
    )
}

fn fund_user(
    ctx: &mut MarketCtx,
    mint: &Pubkey,
    lamports_airdrop: u64,
    mint_amount: u64,
) -> (Keypair, Pubkey) {
    let user = Keypair::new();
    ctx.svm.airdrop(&user.pubkey(), lamports_airdrop).unwrap();
    let user_ta = create_token_account(&mut ctx.svm, &ctx.admin, mint, &user.pubkey());
    if mint_amount > 0 {
        mint_to(
            &mut ctx.svm,
            &ctx.admin,
            &ctx.mint_authority,
            mint,
            &user_ta,
            mint_amount,
        );
    }
    (user, user_ta)
}

fn read_user_account(svm: &LiteSVM, user_account: &Pubkey) -> UserAccount {
    let acc = svm.get_account(user_account).expect("user_account missing");
    UserAccount::try_deserialize(&mut acc.data.as_slice()).expect("deserialize UserAccount")
}

#[test]
fn first_deposit_creates_user_account() {
    let mut ctx = setup_market();
    let base_mint = ctx.base_mint;
    let (user, user_ta) = fund_user(&mut ctx, &base_mint, 100_000_000_000, 1_000);
    let (user_account, _) = user_account_pda(&ctx.market, &user.pubkey());

    let ix = build_deposit_ix(
        &user.pubkey(),
        &ctx.market,
        &user_account,
        &user_ta,
        &ctx.base_vault,
        &ctx.base_mint,
        100,
    );
    send_tx(&mut ctx.svm, &[ix], &[&user]).expect("deposit failed");

    let parsed = read_user_account(&ctx.svm, &user_account);
    assert_eq!(parsed.owner, user.pubkey());
    assert_eq!(parsed.market, ctx.market);
    assert_eq!(parsed.base_free, 100);
    assert_eq!(parsed.quote_free, 0);
}

#[test]
fn second_deposit_increments_balance() {
    let mut ctx = setup_market();
    let base_mint = ctx.base_mint;
    let (user, user_ta) = fund_user(&mut ctx, &base_mint, 100_000_000_000, 1_000);
    let (user_account, _) = user_account_pda(&ctx.market, &user.pubkey());

    for amount in [100u64, 50u64] {
        let ix = build_deposit_ix(
            &user.pubkey(),
            &ctx.market,
            &user_account,
            &user_ta,
            &ctx.base_vault,
            &ctx.base_mint,
            amount,
        );
        send_tx(&mut ctx.svm, &[ix], &[&user]).expect("deposit failed");
    }

    let parsed = read_user_account(&ctx.svm, &user_account);
    assert_eq!(parsed.base_free, 150);
    assert_eq!(parsed.quote_free, 0);
}

#[test]
fn wrong_mint_rejected() {
    let mut ctx = setup_market();

    // A third, unrelated mint. Use same mint_authority for convenience.
    let other_mint = create_mint(
        &mut ctx.svm,
        &ctx.admin,
        9,
        &ctx.mint_authority.pubkey(),
        &[],
    );

    let user = Keypair::new();
    ctx.svm.airdrop(&user.pubkey(), 100_000_000_000).unwrap();
    let user_ta = create_token_account(&mut ctx.svm, &ctx.admin, &other_mint, &user.pubkey());
    mint_to(
        &mut ctx.svm,
        &ctx.admin,
        &ctx.mint_authority,
        &other_mint,
        &user_ta,
        1_000,
    );

    let (user_account, _) = user_account_pda(&ctx.market, &user.pubkey());

    // Use the base_vault but mismatched mint; the program's first check is
    // `mint != base_mint && mint != quote_mint` -> WrongMint.
    let ix = build_deposit_ix(
        &user.pubkey(),
        &ctx.market,
        &user_account,
        &user_ta,
        &ctx.base_vault,
        &other_mint,
        100,
    );
    let err =
        send_tx(&mut ctx.svm, &[ix], &[&user]).expect_err("deposit with wrong mint should fail");
    assert!(
        has_custom_error(&err, ERR_WRONG_MINT),
        "expected WrongMint ({ERR_WRONG_MINT}), got err={:?} logs={:?}",
        err.err,
        err.meta.logs,
    );
}

#[test]
fn deposit_withdraw_roundtrip_zero() {
    let mut ctx = setup_market();
    let base_mint = ctx.base_mint;
    let start_balance = 5_000u64;
    let (user, user_ta) = fund_user(&mut ctx, &base_mint, 100_000_000_000, start_balance);
    let (user_account, _) = user_account_pda(&ctx.market, &user.pubkey());

    let deposit_ix = build_deposit_ix(
        &user.pubkey(),
        &ctx.market,
        &user_account,
        &user_ta,
        &ctx.base_vault,
        &ctx.base_mint,
        1_000,
    );
    send_tx(&mut ctx.svm, &[deposit_ix], &[&user]).expect("deposit failed");

    assert_eq!(token_balance(&ctx.svm, &user_ta), start_balance - 1_000);

    let withdraw_ix = build_withdraw_ix(
        &user.pubkey(),
        &ctx.market,
        &user_account,
        &user_ta,
        &ctx.base_vault,
        &ctx.base_mint,
        1_000,
    );
    send_tx(&mut ctx.svm, &[withdraw_ix], &[&user]).expect("withdraw failed");

    let parsed = read_user_account(&ctx.svm, &user_account);
    assert_eq!(parsed.base_free, 0);
    assert_eq!(parsed.quote_free, 0);
    assert_eq!(token_balance(&ctx.svm, &user_ta), start_balance);
}

#[test]
fn overdraw_rejected() {
    let mut ctx = setup_market();
    let base_mint = ctx.base_mint;
    let (user, user_ta) = fund_user(&mut ctx, &base_mint, 100_000_000_000, 1_000);
    let (user_account, _) = user_account_pda(&ctx.market, &user.pubkey());

    let deposit_ix = build_deposit_ix(
        &user.pubkey(),
        &ctx.market,
        &user_account,
        &user_ta,
        &ctx.base_vault,
        &ctx.base_mint,
        100,
    );
    send_tx(&mut ctx.svm, &[deposit_ix], &[&user]).expect("deposit failed");

    let withdraw_ix = build_withdraw_ix(
        &user.pubkey(),
        &ctx.market,
        &user_account,
        &user_ta,
        &ctx.base_vault,
        &ctx.base_mint,
        200,
    );
    let err =
        send_tx(&mut ctx.svm, &[withdraw_ix], &[&user]).expect_err("over-withdraw should fail");
    assert!(
        has_custom_error(&err, ERR_INSUFFICIENT_FREE_BALANCE),
        "expected InsufficientFreeBalance ({ERR_INSUFFICIENT_FREE_BALANCE}), got err={:?} logs={:?}",
        err.err,
        err.meta.logs,
    );
}
