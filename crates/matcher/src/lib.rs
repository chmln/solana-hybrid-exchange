//! Off-chain CLOB matching engine. Pure library, no I/O.
//!
//! Sits on the gateway's hot path: takes signed-and-verified orders in, emits fills out.
//! Persistence, signatures, user identity, expiry tracking, and nonce management all live
//! in the gateway/Postgres layer — the matcher only knows price-time priority and book state.
//!
//! Two responsibilities the matcher delegates to its caller:
//! - **Self-trade prevention.** The matcher has no `user` field; two orders from the same
//!   user will match. If self-trade prevention is desired, the gateway must enforce it
//!   before calling `submit`.
//! - **`Side` discriminant compatibility with the on-chain program.** The matcher's `Side`
//!   has no fixed `#[repr]`; the gateway is responsible for translating to the on-chain
//!   `Side` (which uses `Bid = 0, Ask = 1`) when building settle txs.

mod book;
mod slab;
mod types;

pub use book::Book;
pub use types::{
    BookSnapshot, CancelInfo, Fill, NewOrder, OrderId, OrderView, Price, PriceLevel, Side, Size,
    SubmitError,
};
