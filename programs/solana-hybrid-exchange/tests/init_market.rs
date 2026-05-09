//! Tests for the `init_market` instruction.

mod common;

use {
    anchor_lang::{
        prelude::Pubkey, solana_program::instruction::Instruction, AccountDeserialize,
        InstructionData, ToAccountMetas,
    },
    common::*,
    litesvm::{types::FailedTransactionMetadata, LiteSVM},
    solana_hybrid_exchange::{
        accounts as program_accounts, instruction as program_ix, state::Market,
    },
    solana_keypair::Keypair,
    solana_signer::Signer,
};

const RENT_SYSVAR_ID: Pubkey = anchor_lang::pubkey!("SysvarRent111111111111111111111111111111111");

/// Anchor user error codes start at 6000.
const ERR_MINT_HAS_DISALLOWED_EXTENSION: u32 = 6000;

fn fresh_svm() -> LiteSVM {
    let mut svm = LiteSVM::new();
    svm.add_program(
        solana_hybrid_exchange::id(),
        include_bytes!("../../../target/deploy/solana_hybrid_exchange.so"),
    )
    .unwrap();
    svm
}

fn build_init_market_ix(
    admin: &Pubkey,
    base_mint: &Pubkey,
    quote_mint: &Pubkey,
    market: &Pubkey,
    base_vault: &Pubkey,
    quote_vault: &Pubkey,
) -> Instruction {
    let accounts = program_accounts::InitMarket {
        admin: *admin,
        base_mint: *base_mint,
        quote_mint: *quote_mint,
        market: *market,
        base_vault: *base_vault,
        quote_vault: *quote_vault,
        token_program: spl_token_2022::id(),
        system_program: SYSTEM_PROGRAM_ID,
        rent: RENT_SYSVAR_ID,
    };
    Instruction::new_with_bytes(
        solana_hybrid_exchange::id(),
        &program_ix::InitMarket {}.data(),
        accounts.to_account_metas(None),
    )
}

/// Inspect a failed tx for the embedded Anchor custom error code.
fn has_custom_error(meta: &FailedTransactionMetadata, code: u32) -> bool {
    let dbg = format!("{:?}", meta.err);
    if dbg.contains(&format!("Custom({code})")) {
        return true;
    }
    let needle = format!("Error Number: {code}");
    meta.meta.logs.iter().any(|l| l.contains(&needle))
}

#[test]
fn init_market_happy_path() {
    let mut svm = fresh_svm();
    let admin = Keypair::new();
    svm.airdrop(&admin.pubkey(), 100_000_000_000).unwrap();

    let base_decimals = 9u8;
    let quote_decimals = 6u8;
    let base_mint = create_mint(&mut svm, &admin, base_decimals, &admin.pubkey(), &[]);
    let quote_mint = create_mint(&mut svm, &admin, quote_decimals, &admin.pubkey(), &[]);

    let (market, bump) = market_pda(&base_mint, &quote_mint);
    let base_vault_kp = Keypair::new();
    let quote_vault_kp = Keypair::new();

    let ix = build_init_market_ix(
        &admin.pubkey(),
        &base_mint,
        &quote_mint,
        &market,
        &base_vault_kp.pubkey(),
        &quote_vault_kp.pubkey(),
    );

    send_tx(&mut svm, &[ix], &[&admin, &base_vault_kp, &quote_vault_kp])
        .expect("init_market happy-path tx failed");

    let market_acc = svm.get_account(&market).expect("market account missing");
    let parsed = Market::try_deserialize(&mut market_acc.data.as_slice())
        .expect("failed to deserialize Market");

    assert_eq!(parsed.admin, admin.pubkey());
    assert_eq!(parsed.base_mint, base_mint);
    assert_eq!(parsed.quote_mint, quote_mint);
    assert_eq!(parsed.base_vault, base_vault_kp.pubkey());
    assert_eq!(parsed.quote_vault, quote_vault_kp.pubkey());
    assert_eq!(parsed.price_scale, 10u64.pow(quote_decimals as u32));
    assert_eq!(parsed.bump, bump);
}

/// Run init_market with `base_mint` carrying `bad_ext`; expect failure with disallowed-extension error.
fn assert_rejects_with_extension(bad_ext: ExtensionInit) {
    let mut svm = fresh_svm();
    let admin = Keypair::new();
    svm.airdrop(&admin.pubkey(), 100_000_000_000).unwrap();

    let base_mint = create_mint(&mut svm, &admin, 9, &admin.pubkey(), &[bad_ext]);
    let quote_mint = create_mint(&mut svm, &admin, 6, &admin.pubkey(), &[]);

    let (market, _) = market_pda(&base_mint, &quote_mint);
    let base_vault_kp = Keypair::new();
    let quote_vault_kp = Keypair::new();

    let ix = build_init_market_ix(
        &admin.pubkey(),
        &base_mint,
        &quote_mint,
        &market,
        &base_vault_kp.pubkey(),
        &quote_vault_kp.pubkey(),
    );

    let err = send_tx(&mut svm, &[ix], &[&admin, &base_vault_kp, &quote_vault_kp])
        .expect_err("init_market should have rejected disallowed extension");

    assert!(
        has_custom_error(&err, ERR_MINT_HAS_DISALLOWED_EXTENSION),
        "expected Anchor error {ERR_MINT_HAS_DISALLOWED_EXTENSION}, got err={:?} logs={:?}",
        err.err,
        err.meta.logs,
    );
}

#[test]
fn init_market_rejects_transfer_fee_mint() {
    let auth = Pubkey::new_unique();
    assert_rejects_with_extension(ExtensionInit::TransferFeeConfig {
        transfer_fee_config_authority: auth,
        withdraw_withheld_authority: auth,
        transfer_fee_basis_points: 50,
        maximum_fee: 1_000_000,
    });
}

#[test]
fn init_market_rejects_transfer_hook_mint() {
    assert_rejects_with_extension(ExtensionInit::TransferHook {
        authority: Pubkey::new_unique(),
        program_id: Pubkey::new_unique(),
    });
}

#[test]
fn init_market_rejects_non_transferable_mint() {
    assert_rejects_with_extension(ExtensionInit::NonTransferable);
}

#[test]
fn init_market_rejects_permanent_delegate_mint() {
    assert_rejects_with_extension(ExtensionInit::PermanentDelegate(Pubkey::new_unique()));
}

#[test]
fn init_market_rejects_pausable_mint() {
    assert_rejects_with_extension(ExtensionInit::Pausable(Pubkey::new_unique()));
}

#[test]
fn init_market_rejects_mint_close_authority_mint() {
    assert_rejects_with_extension(ExtensionInit::MintCloseAuthority(Pubkey::new_unique()));
}
