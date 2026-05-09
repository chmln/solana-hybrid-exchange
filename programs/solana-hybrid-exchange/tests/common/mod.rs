//! Test harness for the solana-hybrid-exchange program.
//!
//! Program-specific helpers only: Token-2022 mint/account setup, an Ed25519
//! precompile ix builder for off-chain order signatures, and PDA derivation
//! using the program's exported seeds.

#![allow(dead_code)]

use {
    anchor_lang::{prelude::Pubkey, solana_program::instruction::Instruction},
    litesvm::{types::FailedTransactionMetadata, LiteSVM},
    solana_hybrid_exchange::constants::{MARKET_SEED, ORDER_MARKER_SEED, USER_ACCOUNT_SEED},
    solana_keypair::Keypair,
    solana_message::{Message, VersionedMessage},
    solana_signer::Signer,
    solana_transaction::versioned::VersionedTransaction,
};

/// Ed25519 sigverify precompile program id.
pub const ED25519_PROGRAM_ID: Pubkey =
    anchor_lang::pubkey!("Ed25519SigVerify111111111111111111111111111");

/// System program id.
pub const SYSTEM_PROGRAM_ID: Pubkey = anchor_lang::pubkey!("11111111111111111111111111111111");

// ---------------------------------------------------------------------------
// Tx send helper
// ---------------------------------------------------------------------------

/// Build, sign, and submit a tx. The first signer pays fees.
pub fn send_tx(
    svm: &mut LiteSVM,
    ixs: &[Instruction],
    signers: &[&Keypair],
) -> Result<litesvm::types::TransactionMetadata, FailedTransactionMetadata> {
    let payer = signers[0].pubkey();
    let blockhash = svm.latest_blockhash();
    let msg = Message::new_with_blockhash(ixs, Some(&payer), &blockhash);
    let tx = VersionedTransaction::try_new(VersionedMessage::Legacy(msg), signers).unwrap();
    svm.send_transaction(tx)
}

// ---------------------------------------------------------------------------
// Token-2022 mint + account helpers
// ---------------------------------------------------------------------------

/// Which Token-2022 extension(s) to initialize on a mint *before* `InitializeMint2`.
/// Variants line up with the disallowed extensions checked by `init_market`.
pub enum ExtensionInit {
    TransferFeeConfig {
        transfer_fee_config_authority: Pubkey,
        withdraw_withheld_authority: Pubkey,
        transfer_fee_basis_points: u16,
        maximum_fee: u64,
    },
    TransferHook {
        authority: Pubkey,
        program_id: Pubkey,
    },
    NonTransferable,
    PermanentDelegate(Pubkey),
    Pausable(Pubkey),
    MintCloseAuthority(Pubkey),
}

impl ExtensionInit {
    fn extension_type(&self) -> spl_token_2022::extension::ExtensionType {
        use spl_token_2022::extension::ExtensionType as E;
        match self {
            Self::TransferFeeConfig { .. } => E::TransferFeeConfig,
            Self::TransferHook { .. } => E::TransferHook,
            Self::NonTransferable => E::NonTransferable,
            Self::PermanentDelegate(_) => E::PermanentDelegate,
            Self::Pausable(_) => E::Pausable,
            Self::MintCloseAuthority(_) => E::MintCloseAuthority,
        }
    }

    fn build_ix(&self, mint: &Pubkey) -> Instruction {
        let program_id = spl_token_2022::id();
        match self {
            Self::TransferFeeConfig {
                transfer_fee_config_authority,
                withdraw_withheld_authority,
                transfer_fee_basis_points,
                maximum_fee,
            } => spl_token_2022::extension::transfer_fee::instruction::initialize_transfer_fee_config(
                &program_id,
                mint,
                Some(transfer_fee_config_authority),
                Some(withdraw_withheld_authority),
                *transfer_fee_basis_points,
                *maximum_fee,
            )
            .unwrap(),
            Self::TransferHook { authority, program_id: hook_pid } =>
                spl_token_2022::extension::transfer_hook::instruction::initialize(
                    &program_id, mint, Some(*authority), Some(*hook_pid),
                ).unwrap(),
            Self::NonTransferable =>
                spl_token_2022::instruction::initialize_non_transferable_mint(&program_id, mint).unwrap(),
            Self::PermanentDelegate(d) =>
                spl_token_2022::instruction::initialize_permanent_delegate(&program_id, mint, d).unwrap(),
            Self::Pausable(auth) =>
                spl_token_2022::extension::pausable::instruction::initialize(&program_id, mint, auth).unwrap(),
            Self::MintCloseAuthority(auth) =>
                spl_token_2022::instruction::initialize_mint_close_authority(&program_id, mint, Some(auth)).unwrap(),
        }
    }
}

/// Create a Token-2022 mint with the given extensions initialized before `initialize_mint2`.
pub fn create_mint(
    svm: &mut LiteSVM,
    payer: &Keypair,
    decimals: u8,
    mint_authority: &Pubkey,
    extensions: &[ExtensionInit],
) -> Pubkey {
    let mint_kp = Keypair::new();
    let mint = mint_kp.pubkey();
    let token_program = spl_token_2022::id();

    let ext_types: Vec<_> = extensions.iter().map(|e| e.extension_type()).collect();
    let space = spl_token_2022::extension::ExtensionType::try_calculate_account_len::<
        spl_token_2022::state::Mint,
    >(&ext_types)
    .unwrap();
    let rent = svm.minimum_balance_for_rent_exemption(space);

    let mut ixs = vec![system_create_account(
        &payer.pubkey(),
        &mint,
        rent,
        space as u64,
        &token_program,
    )];
    for ext in extensions {
        ixs.push(ext.build_ix(&mint));
    }
    ixs.push(
        spl_token_2022::instruction::initialize_mint2(
            &token_program,
            &mint,
            mint_authority,
            None,
            decimals,
        )
        .unwrap(),
    );

    send_tx(svm, &ixs, &[payer, &mint_kp]).expect("create mint failed");
    mint
}

/// Create a (non-ATA) Token-2022 token account owned by `owner` for `mint`.
pub fn create_token_account(
    svm: &mut LiteSVM,
    payer: &Keypair,
    mint: &Pubkey,
    owner: &Pubkey,
) -> Pubkey {
    let token_program = spl_token_2022::id();
    let acc_kp = Keypair::new();
    let acc = acc_kp.pubkey();

    // Base Token-2022 token account is 165 bytes; user-side accounts don't use extensions.
    let space = 165usize;
    let rent = svm.minimum_balance_for_rent_exemption(space);

    let create = system_create_account(&payer.pubkey(), &acc, rent, space as u64, &token_program);
    let init = spl_token_2022::instruction::initialize_account3(&token_program, &acc, mint, owner)
        .unwrap();

    send_tx(svm, &[create, init], &[payer, &acc_kp]).expect("create token account failed");
    acc
}

/// Mint `amount` of `mint` to `dest`. Signs with `payer` (fees) + `mint_authority`.
pub fn mint_to(
    svm: &mut LiteSVM,
    payer: &Keypair,
    mint_authority: &Keypair,
    mint: &Pubkey,
    dest: &Pubkey,
    amount: u64,
) {
    let ix = spl_token_2022::instruction::mint_to(
        &spl_token_2022::id(),
        mint,
        dest,
        &mint_authority.pubkey(),
        &[],
        amount,
    )
    .unwrap();
    send_tx(svm, &[ix], &[payer, mint_authority]).expect("mint_to failed");
}

/// Read the raw `amount` field (u64 LE at offset 64..72) from a Token-2022 token account.
pub fn token_balance(svm: &LiteSVM, addr: &Pubkey) -> u64 {
    let acc = svm.get_account(addr).expect("token account not found");
    u64::from_le_bytes(acc.data[64..72].try_into().unwrap())
}

// ---------------------------------------------------------------------------
// System program: CreateAccount (local builder to avoid an extra dep)
// ---------------------------------------------------------------------------

fn system_create_account(
    from: &Pubkey,
    new_account: &Pubkey,
    lamports: u64,
    space: u64,
    owner: &Pubkey,
) -> Instruction {
    use anchor_lang::solana_program::instruction::AccountMeta;
    // CreateAccount: u32 discriminator (0), lamports (u64), space (u64), owner (32B).
    let mut data = Vec::with_capacity(4 + 8 + 8 + 32);
    data.extend_from_slice(&0u32.to_le_bytes());
    data.extend_from_slice(&lamports.to_le_bytes());
    data.extend_from_slice(&space.to_le_bytes());
    data.extend_from_slice(owner.as_ref());
    Instruction {
        program_id: SYSTEM_PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(*from, true),
            AccountMeta::new(*new_account, true),
        ],
        data,
    }
}

// ---------------------------------------------------------------------------
// Ed25519 precompile ix builder
// ---------------------------------------------------------------------------

// Matches `solana_ed25519_program::new_ed25519_instruction_with_signature` layout.
const SIGNATURE_OFFSETS_START: usize = 2;
const SIGNATURE_OFFSETS_SERIALIZED_SIZE: usize = 14;
const DATA_START: usize = SIGNATURE_OFFSETS_START + SIGNATURE_OFFSETS_SERIALIZED_SIZE;
const PUBKEY_SIZE: usize = 32;
const SIGNATURE_SIZE: usize = 64;

/// Single-signature Ed25519 precompile ix: verifies `signature` of `message` by `pubkey`.
/// Pubkey/signature/message all live inline in this ix's own data buffer.
pub fn ed25519_verify_ix(pubkey: &[u8; 32], signature: &[u8; 64], message: &[u8]) -> Instruction {
    let mut data = Vec::with_capacity(DATA_START + PUBKEY_SIZE + SIGNATURE_SIZE + message.len());

    let public_key_offset = DATA_START;
    let signature_offset = public_key_offset + PUBKEY_SIZE;
    let message_data_offset = signature_offset + SIGNATURE_SIZE;

    // num_signatures (u8) + padding byte for u16 alignment.
    data.extend_from_slice(&[1u8, 0u8]);

    // Ed25519SignatureOffsets, little-endian u16 fields.
    data.extend_from_slice(&(signature_offset as u16).to_le_bytes());
    data.extend_from_slice(&u16::MAX.to_le_bytes()); // signature_instruction_index
    data.extend_from_slice(&(public_key_offset as u16).to_le_bytes());
    data.extend_from_slice(&u16::MAX.to_le_bytes()); // public_key_instruction_index
    data.extend_from_slice(&(message_data_offset as u16).to_le_bytes());
    data.extend_from_slice(&(message.len() as u16).to_le_bytes());
    data.extend_from_slice(&u16::MAX.to_le_bytes()); // message_instruction_index

    data.extend_from_slice(pubkey);
    data.extend_from_slice(signature);
    data.extend_from_slice(message);

    Instruction {
        program_id: ED25519_PROGRAM_ID,
        accounts: vec![],
        data,
    }
}

// ---------------------------------------------------------------------------
// PDA helpers
// ---------------------------------------------------------------------------

pub fn market_pda(base_mint: &Pubkey, quote_mint: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[MARKET_SEED, base_mint.as_ref(), quote_mint.as_ref()],
        &solana_hybrid_exchange::id(),
    )
}

pub fn user_account_pda(market: &Pubkey, user: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[USER_ACCOUNT_SEED, market.as_ref(), user.as_ref()],
        &solana_hybrid_exchange::id(),
    )
}

pub fn order_marker_pda(order_hash: &[u8; 32]) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[ORDER_MARKER_SEED, order_hash],
        &solana_hybrid_exchange::id(),
    )
}
