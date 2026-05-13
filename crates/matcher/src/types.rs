//! Public value types crossing the matcher's API boundary.
//!
//! Domain values are newtypes — no `type` aliases — so the compiler stops you mixing a
//! `Price` where a `Size` was wanted.

/// Side of the book.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Side {
    Bid,
    Ask,
}

impl Side {
    #[inline]
    pub fn opposite(self) -> Side {
        match self {
            Side::Bid => Side::Ask,
            Side::Ask => Side::Bid,
        }
    }
}

/// Opaque handle returned by `Book::submit`. Encodes a slab index plus a generation
/// counter so stale ids (referring to recycled slots) are rejected by `cancel` / `get`.
/// The gateway uses the raw `u64` form as its DB primary key.
///
/// The generation field is `u32`. After 2^32 reuses of a single slot it wraps; in
/// practice a single slot won't be recycled that many times in a session (would require
/// ~4 billion fully-consumed orders against that exact slot), so the collision risk is
/// theoretical.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct OrderId(u64);

impl OrderId {
    #[inline]
    pub(crate) fn pack(idx: u32, generation: u32) -> Self {
        Self(((generation as u64) << 32) | (idx as u64))
    }
    #[inline]
    pub(crate) fn idx(self) -> u32 {
        self.0 as u32
    }
    #[inline]
    pub(crate) fn generation(self) -> u32 {
        (self.0 >> 32) as u32
    }
    #[inline]
    pub fn to_u64(self) -> u64 {
        self.0
    }
    #[inline]
    pub fn from_u64(raw: u64) -> Self {
        Self(raw)
    }
}

/// Limit price in raw quote-lamport units.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Price(pub u64);

/// Order size in raw base-lamport units.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Size(pub u64);

impl Size {
    #[inline]
    pub fn is_zero(self) -> bool {
        self.0 == 0
    }
}

/// Input to `Book::submit`. The matcher does not store user identity, nonce, signature,
/// or expiry — those belong to the gateway's persistence layer, keyed by the assigned
/// `OrderId`.
#[derive(Copy, Clone, Debug)]
pub struct NewOrder {
    pub side: Side,
    pub limit_price: Price,
    pub max_size: Size,
}

/// Reasons `Book::submit` may reject an order.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SubmitError {
    /// `max_size == 0` — an empty order cannot match.
    ZeroSize,
    /// `limit_price < price_scale` — would produce zero-quote fills against any
    /// single-lamport counterparty. The operator's `price_scale` choice is the
    /// market's price floor; reject below.
    PriceBelowScale,
    /// Slab is at `u32::MAX - 1` entries; no further slots can be allocated. Cancel
    /// existing orders to free slots before resubmitting.
    BookFull,
}

/// One match output. Emitted via the `on_fill` callback on `Book::submit`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Fill {
    pub maker_id: OrderId,
    pub taker_id: OrderId,
    pub fill_price: Price,
    pub fill_size: Size,
}

/// Read-only view of a resting order.
#[derive(Copy, Clone, Debug)]
pub struct OrderView {
    pub id: OrderId,
    pub side: Side,
    pub limit_price: Price,
    pub max_size: Size,
    pub remaining: Size,
}

/// Returned by `Book::cancel`.
#[derive(Copy, Clone, Debug)]
pub struct CancelInfo {
    pub side: Side,
    pub limit_price: Price,
    pub remaining: Size,
}

/// One price level in a `BookSnapshot`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct PriceLevel {
    pub price: Price,
    pub size: Size,
}

/// Aggregated price-level view, best-first per side.
#[derive(Clone, Debug, Default)]
pub struct BookSnapshot {
    pub bids: Vec<PriceLevel>,
    pub asks: Vec<PriceLevel>,
}
