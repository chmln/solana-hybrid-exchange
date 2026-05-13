//! CLOB book: slab-backed orders, intrusive doubly-linked FIFO per price level,
//! per-side `BTreeMap` of levels, and a cached top-of-book on each side so the
//! steady-state match walk never touches the BTreeMap.

use std::cmp::Reverse;
use std::collections::BTreeMap;

use crate::types::{
    BookSnapshot, CancelInfo, Fill, NewOrder, OrderId, OrderView, Price, PriceLevel, Side, Size,
    SubmitError,
};

const NIL: u32 = u32::MAX;

/// 40 bytes — one cache line per node. No `user` / `nonce` / `expiry`: those live in the
/// gateway. Empty slots hold their `generation` plus a free-list link in `next`.
#[derive(Copy, Clone, Debug)]
struct OrderNode {
    limit_price: Price,
    max_size: Size,
    remaining: Size,
    prev: u32,
    next: u32,
    generation: u32,
    side: Side,
    occupied: bool,
}

#[derive(Copy, Clone, Debug)]
struct Level {
    head: u32,
    tail: u32,
    total_size: Size,
}

pub struct Book {
    slab: Vec<OrderNode>,
    free_head: u32,
    bids: BTreeMap<Reverse<Price>, Level>,
    asks: BTreeMap<Price, Level>,
    best_bid: Option<Price>,
    best_ask: Option<Price>,
    price_scale: u64,
}

impl Book {
    /// `price_scale` is the market's price floor. Every admitted order must satisfy
    /// `limit_price >= price_scale`, which guarantees every fill produces
    /// `quote_amount = fill_price * fill_size / price_scale >= 1` and the on-chain
    /// `ZeroQuoteAmount` check is impossible to trip. Operator picks `price_scale`
    /// per market to balance price granularity against minimum tradeable size.
    pub fn new(price_scale: u64) -> Self {
        Self::with_capacity(price_scale, 0)
    }

    pub fn with_capacity(price_scale: u64, orders: usize) -> Self {
        assert!(price_scale > 0, "price_scale must be > 0");
        Self {
            slab: Vec::with_capacity(orders),
            free_head: NIL,
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
            best_bid: None,
            best_ask: None,
            price_scale,
        }
    }

    #[inline]
    pub fn best_bid(&self) -> Option<Price> {
        self.best_bid
    }

    #[inline]
    pub fn best_ask(&self) -> Option<Price> {
        self.best_ask
    }

    /// Match `order` against the book, invoking `on_fill` for each fill emitted.
    /// Unfilled remainder rests on the book at `order.limit_price`. Returns the
    /// `OrderId` assigned to the order whether it rested or was fully consumed.
    ///
    /// Rejects orders that violate the matcher's no-zero-quote invariant:
    /// `max_size > 0` and `limit_price >= price_scale`.
    pub fn submit(
        &mut self,
        order: NewOrder,
        mut on_fill: impl FnMut(Fill),
    ) -> Result<OrderId, SubmitError> {
        if order.max_size.is_zero() {
            return Err(SubmitError::ZeroSize);
        }
        if order.limit_price.0 < self.price_scale {
            return Err(SubmitError::PriceBelowScale);
        }

        let taker_idx = self.allocate_slot();
        let taker_gen = self.slab[taker_idx as usize].generation;
        let taker_id = OrderId::pack(taker_idx, taker_gen);

        {
            let n = &mut self.slab[taker_idx as usize];
            n.limit_price = order.limit_price;
            n.max_size = order.max_size;
            n.remaining = order.max_size;
            n.side = order.side;
        }

        let opp = order.side.opposite();
        loop {
            let taker_remaining = self.slab[taker_idx as usize].remaining;
            if taker_remaining.is_zero() {
                break;
            }
            let Some(best) = self.cached_best(opp) else {
                break;
            };
            if !crosses(order.side, order.limit_price, best) {
                break;
            }

            let (maker_idx, maker_remaining, maker_gen) = {
                let level = self
                    .level_get(opp, best)
                    .expect("cached best with no level");
                let head = level.head;
                let n = &self.slab[head as usize];
                (head, n.remaining, n.generation)
            };

            let fill_size = std::cmp::min(taker_remaining, maker_remaining);
            let fill_price = best;

            // `submit`'s precondition (`limit_price >= price_scale`) guarantees both sides
            // satisfy `limit >= price_scale`, hence `fill_price * fill_size / price_scale
            // >= fill_size >= 1`. No zero-quote check needed in the walk.

            on_fill(Fill {
                maker_id: OrderId::pack(maker_idx, maker_gen),
                taker_id,
                fill_price,
                fill_size,
            });

            self.slab[taker_idx as usize].remaining = Size(taker_remaining.0 - fill_size.0);

            if maker_remaining == fill_size {
                self.pop_front(opp, best);
                self.free_slot(maker_idx);
            } else {
                self.slab[maker_idx as usize].remaining = Size(maker_remaining.0 - fill_size.0);
                let level = self.level_get_mut(opp, best).unwrap();
                level.total_size = Size(level.total_size.0 - fill_size.0);
            }
        }

        if self.slab[taker_idx as usize].remaining.is_zero() {
            self.free_slot(taker_idx);
        } else {
            self.append_to_level(order.side, order.limit_price, taker_idx);
        }

        Ok(taker_id)
    }

    /// Cancel `id` if it currently rests. Returns `None` for stale or fully-consumed ids.
    pub fn cancel(&mut self, id: OrderId) -> Option<CancelInfo> {
        let idx = id.idx() as usize;
        let node = self.slab.get(idx)?;
        if !node.occupied || node.generation != id.generation() {
            return None;
        }
        let side = node.side;
        let price = node.limit_price;
        let remaining = node.remaining;
        let prev = node.prev;
        let next = node.next;

        if prev != NIL {
            self.slab[prev as usize].next = next;
        }
        if next != NIL {
            self.slab[next as usize].prev = prev;
        }

        let level_empty = {
            let level = self.level_get_mut(side, price).expect("level must exist");
            if level.head == idx as u32 {
                level.head = next;
            }
            if level.tail == idx as u32 {
                level.tail = prev;
            }
            level.total_size = Size(level.total_size.0 - remaining.0);
            level.head == NIL
        };

        if level_empty {
            self.level_remove(side, price);
            self.maybe_recompute_best_on_remove(side, price);
        }

        self.free_slot(idx as u32);

        Some(CancelInfo {
            side,
            limit_price: price,
            remaining,
        })
    }

    /// Read-only view of `id` if it currently rests.
    pub fn get(&self, id: OrderId) -> Option<OrderView> {
        let node = self.slab.get(id.idx() as usize)?;
        if !node.occupied || node.generation != id.generation() {
            return None;
        }
        Some(OrderView {
            id,
            side: node.side,
            limit_price: node.limit_price,
            max_size: node.max_size,
            remaining: node.remaining,
        })
    }

    /// Up to `top_n` price levels per side, best-first.
    pub fn snapshot(&self, top_n: usize) -> BookSnapshot {
        let bids = self
            .bids
            .iter()
            .take(top_n)
            .map(|(p, l)| PriceLevel {
                price: p.0,
                size: l.total_size,
            })
            .collect();
        let asks = self
            .asks
            .iter()
            .take(top_n)
            .map(|(p, l)| PriceLevel {
                price: *p,
                size: l.total_size,
            })
            .collect();
        BookSnapshot { bids, asks }
    }

    // --- Internal helpers --------------------------------------------------

    fn allocate_slot(&mut self) -> u32 {
        if self.free_head != NIL {
            let idx = self.free_head;
            self.free_head = self.slab[idx as usize].next;
            let generation = self.slab[idx as usize].generation;
            self.slab[idx as usize] = OrderNode {
                limit_price: Price(0),
                max_size: Size(0),
                remaining: Size(0),
                prev: NIL,
                next: NIL,
                generation,
                side: Side::Bid,
                occupied: true,
            };
            idx
        } else {
            let idx = self.slab.len() as u32;
            self.slab.push(OrderNode {
                limit_price: Price(0),
                max_size: Size(0),
                remaining: Size(0),
                prev: NIL,
                next: NIL,
                generation: 0,
                side: Side::Bid,
                occupied: true,
            });
            idx
        }
    }

    fn free_slot(&mut self, idx: u32) {
        let next_free = self.free_head;
        let node = &mut self.slab[idx as usize];
        node.limit_price = Price(0);
        node.max_size = Size(0);
        node.remaining = Size(0);
        node.prev = NIL;
        node.next = next_free;
        node.generation = node.generation.wrapping_add(1);
        node.side = Side::Bid;
        node.occupied = false;
        self.free_head = idx;
    }

    fn append_to_level(&mut self, side: Side, price: Price, idx: u32) {
        let remaining = self.slab[idx as usize].remaining;
        self.slab[idx as usize].prev = NIL;
        self.slab[idx as usize].next = NIL;

        let prev_tail = match self.level_get_mut(side, price) {
            Some(level) => {
                let t = level.tail;
                level.tail = idx;
                level.total_size = Size(level.total_size.0 + remaining.0);
                Some(t)
            }
            None => {
                self.level_insert(
                    side,
                    price,
                    Level {
                        head: idx,
                        tail: idx,
                        total_size: remaining,
                    },
                );
                self.maybe_update_best_on_insert(side, price);
                None
            }
        };

        if let Some(tail) = prev_tail {
            self.slab[tail as usize].next = idx;
            self.slab[idx as usize].prev = tail;
        }
    }

    /// Pops head of `(side, price)`. Caller is responsible for freeing the returned slot.
    /// Panics in debug if the level doesn't exist.
    fn pop_front(&mut self, side: Side, price: Price) -> u32 {
        let head_idx = self.level_get(side, price).expect("level must exist").head;

        let (next_idx, head_remaining) = {
            let n = &self.slab[head_idx as usize];
            (n.next, n.remaining)
        };

        {
            let level = self.level_get_mut(side, price).unwrap();
            level.head = next_idx;
            level.total_size = Size(level.total_size.0 - head_remaining.0);
            if next_idx == NIL {
                level.tail = NIL;
            }
        }

        if next_idx != NIL {
            self.slab[next_idx as usize].prev = NIL;
        } else {
            self.level_remove(side, price);
            self.maybe_recompute_best_on_remove(side, price);
        }
        head_idx
    }

    #[inline]
    fn cached_best(&self, side: Side) -> Option<Price> {
        match side {
            Side::Bid => self.best_bid,
            Side::Ask => self.best_ask,
        }
    }

    fn level_get(&self, side: Side, price: Price) -> Option<&Level> {
        match side {
            Side::Bid => self.bids.get(&Reverse(price)),
            Side::Ask => self.asks.get(&price),
        }
    }

    fn level_get_mut(&mut self, side: Side, price: Price) -> Option<&mut Level> {
        match side {
            Side::Bid => self.bids.get_mut(&Reverse(price)),
            Side::Ask => self.asks.get_mut(&price),
        }
    }

    fn level_insert(&mut self, side: Side, price: Price, level: Level) {
        match side {
            Side::Bid => {
                self.bids.insert(Reverse(price), level);
            }
            Side::Ask => {
                self.asks.insert(price, level);
            }
        }
    }

    fn level_remove(&mut self, side: Side, price: Price) {
        match side {
            Side::Bid => {
                self.bids.remove(&Reverse(price));
            }
            Side::Ask => {
                self.asks.remove(&price);
            }
        }
    }

    fn maybe_update_best_on_insert(&mut self, side: Side, new_price: Price) {
        match side {
            Side::Bid => {
                if self.best_bid.is_none_or(|b| new_price > b) {
                    self.best_bid = Some(new_price);
                }
            }
            Side::Ask => {
                if self.best_ask.is_none_or(|a| new_price < a) {
                    self.best_ask = Some(new_price);
                }
            }
        }
    }

    fn maybe_recompute_best_on_remove(&mut self, side: Side, removed_price: Price) {
        match side {
            Side::Bid => {
                if self.best_bid == Some(removed_price) {
                    self.best_bid = self.bids.keys().next().map(|r| r.0);
                }
            }
            Side::Ask => {
                if self.best_ask == Some(removed_price) {
                    self.best_ask = self.asks.keys().next().copied();
                }
            }
        }
    }
}

#[inline]
fn crosses(taker_side: Side, taker_price: Price, opposite_best: Price) -> bool {
    match taker_side {
        Side::Bid => taker_price >= opposite_best,
        Side::Ask => taker_price <= opposite_best,
    }
}
