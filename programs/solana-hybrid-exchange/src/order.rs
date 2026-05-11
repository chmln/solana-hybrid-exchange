use anchor_lang::prelude::*;

use crate::constants::{CANONICAL_ORDER_LEN, ORDER_DOMAIN_SEPARATOR};
use crate::error::ExchangeError;

pub const ED25519_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("Ed25519SigVerify111111111111111111111111111");
pub const IX_SYSVAR_ID: Pubkey =
    Pubkey::from_str_const("Sysvar1nstructions1111111111111111111111111");

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[borsh(use_discriminant = true)]
#[repr(u8)]
pub enum Side {
    Bid = 0,
    Ask = 1,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug)]
pub struct SignedOrderArgs {
    pub user: Pubkey,
    pub market: Pubkey,
    pub side: Side,
    pub limit_price: u64,
    pub max_size: u64,
    pub nonce: u64,
    pub expiry: i64,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OrderHash(pub [u8; 32]);

impl OrderHash {
    pub fn of(order: &SignedOrderArgs) -> Self {
        Self(solana_sha256_hasher::hashv(&[&canonical_serialize(order)]).to_bytes())
    }
}

impl AsRef<[u8]> for OrderHash {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

/// Fixed-layout canonical bytes the user signs. 113 bytes:
/// 16 domain | 32 user | 32 market | 1 side | 8 price | 8 size | 8 nonce | 8 expiry
pub fn canonical_serialize(o: &SignedOrderArgs) -> [u8; CANONICAL_ORDER_LEN] {
    let mut out = [0u8; CANONICAL_ORDER_LEN];
    out[0..16].copy_from_slice(ORDER_DOMAIN_SEPARATOR);
    out[16..48].copy_from_slice(o.user.as_ref());
    out[48..80].copy_from_slice(o.market.as_ref());
    out[80] = o.side as u8;
    out[81..89].copy_from_slice(&o.limit_price.to_le_bytes());
    out[89..97].copy_from_slice(&o.max_size.to_le_bytes());
    out[97..105].copy_from_slice(&o.nonce.to_le_bytes());
    out[105..113].copy_from_slice(&o.expiry.to_le_bytes());
    out
}

const SIGNATURE_OFFSETS_START: usize = 2;
const SIGNATURE_OFFSETS_LEN: usize = 14;
const DATA_START: usize = SIGNATURE_OFFSETS_START + SIGNATURE_OFFSETS_LEN;
const PUBKEY_LEN: usize = 32;
const SIG_LEN: usize = 64;

/// Parse a single-signature Ed25519 precompile ix's data. Asserts the precompile verified
/// exactly one signature with pubkey + message contained in its own data buffer, and returns
/// `(pubkey, message)`.
///
/// The precompile would have aborted the tx if the signature didn't verify, so any data we
/// pull out here is already cryptographically authenticated.
pub fn parse_ed25519_precompile_ix(data: &[u8]) -> Result<(Pubkey, &[u8])> {
    require!(
        data.len() >= DATA_START,
        ExchangeError::MissingEd25519Verify
    );
    require_eq!(data[0], 1u8, ExchangeError::MissingEd25519Verify);

    let read_u16 = |off: usize| u16::from_le_bytes([data[off], data[off + 1]]);
    let sig_offset = read_u16(SIGNATURE_OFFSETS_START) as usize;
    let sig_ix_index = read_u16(SIGNATURE_OFFSETS_START + 2);
    let pk_offset = read_u16(SIGNATURE_OFFSETS_START + 4) as usize;
    let pk_ix_index = read_u16(SIGNATURE_OFFSETS_START + 6);
    let msg_offset = read_u16(SIGNATURE_OFFSETS_START + 8) as usize;
    let msg_size = read_u16(SIGNATURE_OFFSETS_START + 10) as usize;
    let msg_ix_index = read_u16(SIGNATURE_OFFSETS_START + 12);

    // Require all three references point into this same precompile ix's data buffer.
    require_eq!(sig_ix_index, u16::MAX, ExchangeError::MissingEd25519Verify);
    require_eq!(pk_ix_index, u16::MAX, ExchangeError::MissingEd25519Verify);
    require_eq!(msg_ix_index, u16::MAX, ExchangeError::MissingEd25519Verify);

    let pk_end = pk_offset
        .checked_add(PUBKEY_LEN)
        .ok_or(ExchangeError::MissingEd25519Verify)?;
    let sig_end = sig_offset
        .checked_add(SIG_LEN)
        .ok_or(ExchangeError::MissingEd25519Verify)?;
    let msg_end = msg_offset
        .checked_add(msg_size)
        .ok_or(ExchangeError::MissingEd25519Verify)?;
    require!(
        pk_end <= data.len() && sig_end <= data.len() && msg_end <= data.len(),
        ExchangeError::MissingEd25519Verify
    );

    let mut pk = [0u8; PUBKEY_LEN];
    pk.copy_from_slice(&data[pk_offset..pk_end]);
    Ok((Pubkey::new_from_array(pk), &data[msg_offset..msg_end]))
}
