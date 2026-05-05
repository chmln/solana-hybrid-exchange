use anchor_lang::prelude::*;

#[error_code]
pub enum ExchangeError {
    #[msg("Mint uses a disallowed Token-2022 extension")]
    MintHasDisallowedExtension,
    #[msg("Mint is not owned by the expected token program")]
    WrongTokenProgram,
    #[msg("Token account does not match the market's mint")]
    WrongMint,
    #[msg("UserAccount free balance is insufficient")]
    InsufficientFreeBalance,
    #[msg("Settle transaction missing required Ed25519 precompile verify")]
    MissingEd25519Verify,
    #[msg("Ed25519 precompile pubkey or message does not match settle args")]
    Ed25519DataMismatch,
    #[msg("Maker and taker orders reference different markets")]
    WrongMarket,
    #[msg("Maker and taker orders are on the same side")]
    SameSide,
    #[msg("Bid limit price does not cross ask limit price")]
    PriceDoesNotCross,
    #[msg("Fill price is outside the crossing range of the two orders")]
    FillPriceOutOfRange,
    #[msg("Order is past its expiry")]
    OrderExpired,
    #[msg("Order has been filled up to its max_size")]
    OrderOverfilled,
    #[msg("fill_size must be greater than zero")]
    ZeroFillSize,
    #[msg("Arithmetic overflow")]
    Overflow,
}
