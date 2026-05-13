//! CLOB book: per-side `BTreeMap` of price levels, each holding an intrusive
//! doubly-linked FIFO of slab-backed orders. A cached top-of-book per side
//! gives `best_bid()` / `best_ask()` O(1) and lets the submit loop bail in
//! O(1) when the taker doesn't cross. The match path itself goes through
//! `BTreeMap::first_entry()` and operates on the entry handle directly — the
//! cache is only ever read as a "should I even probe the map" hint, never
//! used as a key-lookup oracle, so cache/map drift can never trigger a panic.

use std::cmp::Reverse;
use std::collections::btree_map::{BTreeMap, Entry};
use std::num::NonZeroU64;

use crate::slab::{Slab, SlotIdx};
use crate::types::{
    BookSnapshot, CancelInfo, Fill, NewOrder, OrderId, OrderView, Price, PriceLevel, Side, Size,
    SubmitError,
};

#[derive(Copy, Clone, Debug)]
struct Level {
    head: Option<SlotIdx>,
    tail: Option<SlotIdx>,
    total_size: Size,
}

pub struct Book {
    slab: Slab,
    bids: BTreeMap<Reverse<Price>, Level>,
    asks: BTreeMap<Price, Level>,
    best_bid: Option<Price>,
    best_ask: Option<Price>,
    price_scale: NonZeroU64,
}

impl Book {
    /// `price_scale` is the market's price floor. Every admitted order must satisfy
    /// `limit_price >= price_scale`, which guarantees every fill produces
    /// `quote_amount = fill_price * fill_size / price_scale >= 1` and the on-chain
    /// `ZeroQuoteAmount` check is impossible to trip. Operator picks `price_scale`
    /// per market to balance price granularity against minimum tradeable size.
    pub fn new(price_scale: NonZeroU64) -> Self {
        Self::with_capacity(price_scale, 0)
    }

    pub fn with_capacity(price_scale: NonZeroU64, orders: usize) -> Self {
        Self {
            slab: Slab::with_capacity(orders),
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
    pub fn submit(
        &mut self,
        order: NewOrder,
        mut on_fill: impl FnMut(Fill),
    ) -> Result<OrderId, SubmitError> {
        if order.max_size.is_zero() {
            return Err(SubmitError::ZeroSize);
        }
        if order.limit_price.0 < self.price_scale.get() {
            return Err(SubmitError::PriceBelowScale);
        }

        let taker_idx = self.slab.allocate().map_err(|_| SubmitError::BookFull)?;
        let taker_gen = {
            let node = &mut self.slab[taker_idx];
            node.limit_price = order.limit_price;
            node.max_size = order.max_size;
            node.remaining = order.max_size;
            node.side = order.side;
            node.generation
        };
        let taker_id = OrderId::pack(taker_idx.raw(), taker_gen);

        let opp = order.side.opposite();
        loop {
            let taker_remaining = self.slab[taker_idx].remaining;
            if taker_remaining.is_zero() {
                break;
            }
            // Fast-bail via the cached best: avoids any BTreeMap touch on the
            // common "doesn't cross, just rest" path.
            let Some(best_opp) = self.best_for(opp) else {
                break;
            };
            if !crosses(order.side, order.limit_price, best_opp) {
                break;
            }

            let outcome = match opp {
                Side::Bid => match_against_top(
                    &mut self.bids,
                    &mut self.slab,
                    |k| k.0,
                    order.side,
                    order.limit_price,
                    taker_idx,
                    taker_id,
                    taker_remaining,
                    &mut on_fill,
                ),
                Side::Ask => match_against_top(
                    &mut self.asks,
                    &mut self.slab,
                    |k| *k,
                    order.side,
                    order.limit_price,
                    taker_idx,
                    taker_id,
                    taker_remaining,
                    &mut on_fill,
                ),
            };
            match outcome {
                MatchOutcome::NoMatch => break,
                MatchOutcome::Partial => {}
                MatchOutcome::FullyConsumed {
                    maker_idx,
                    level_emptied,
                } => {
                    self.slab.free(maker_idx);
                    if level_emptied {
                        // The emptied level was the top — `best_opp` reflected
                        // its price — so we always need to recompute.
                        self.recompute_best(opp);
                    }
                }
            }
        }

        if self.slab[taker_idx].remaining.is_zero() {
            self.slab.free(taker_idx);
        } else {
            self.append_to_level(order.side, order.limit_price, taker_idx);
        }

        Ok(taker_id)
    }

    /// Cancel `id` if it currently rests. Returns `None` for stale or fully-consumed ids.
    pub fn cancel(&mut self, id: OrderId) -> Option<CancelInfo> {
        let idx = self.slab.lookup_raw(id.idx())?;
        let node = &self.slab[idx];
        if !node.occupied || node.generation != id.generation() {
            return None;
        }
        let side = node.side;
        let price = node.limit_price;
        let remaining = node.remaining;
        let prev = node.prev;
        let next = node.next;

        let result = match side {
            Side::Bid => unlink_from_level(
                &mut self.bids,
                Reverse(price),
                idx,
                prev,
                next,
                remaining,
                &mut self.slab,
            ),
            Side::Ask => unlink_from_level(
                &mut self.asks,
                price,
                idx,
                prev,
                next,
                remaining,
                &mut self.slab,
            ),
        };
        match result {
            UnlinkResult::Missing => return None,
            UnlinkResult::Kept => self.slab.free(idx),
            UnlinkResult::Removed => {
                self.slab.free(idx);
                self.maybe_recompute_best_on_remove(side, price);
            }
        }

        Some(CancelInfo {
            side,
            limit_price: price,
            remaining,
        })
    }

    /// Read-only view of `id` if it currently rests.
    pub fn get(&self, id: OrderId) -> Option<OrderView> {
        let idx = self.slab.lookup_raw(id.idx())?;
        let node = &self.slab[idx];
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

    fn append_to_level(&mut self, side: Side, price: Price, idx: SlotIdx) {
        let remaining = self.slab[idx].remaining;
        {
            let node = &mut self.slab[idx];
            node.prev = None;
            node.next = None;
        }
        let result = match side {
            Side::Bid => append_to_map_level(&mut self.bids, Reverse(price), idx, remaining),
            Side::Ask => append_to_map_level(&mut self.asks, price, idx, remaining),
        };
        if let Some(tail) = result.prev_tail {
            self.slab[tail].next = Some(idx);
            self.slab[idx].prev = Some(tail);
        }
        if result.level_created {
            self.maybe_update_best_on_insert(side, price);
        }
    }

    #[inline]
    fn best_for(&self, side: Side) -> Option<Price> {
        match side {
            Side::Bid => self.best_bid,
            Side::Ask => self.best_ask,
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

    fn recompute_best(&mut self, side: Side) {
        match side {
            Side::Bid => self.best_bid = self.bids.keys().next().map(|r| r.0),
            Side::Ask => self.best_ask = self.asks.keys().next().copied(),
        }
    }
}

enum MatchOutcome {
    NoMatch,
    Partial,
    FullyConsumed {
        maker_idx: SlotIdx,
        level_emptied: bool,
    },
}

struct AppendResult {
    prev_tail: Option<SlotIdx>,
    level_created: bool,
}

enum UnlinkResult {
    Missing,
    Kept,
    Removed,
}

/// One iteration of the match walk against the top of `map`. The outer caller
/// has already used its cached best to fast-bail on the no-cross case; this
/// function defensively re-checks crosses against the map's actual top, so a
/// stale cache cannot cause incorrect fills — it can only cause a redundant
/// entry into this function which then returns `NoMatch`.
#[allow(clippy::too_many_arguments)]
fn match_against_top<K: Ord>(
    map: &mut BTreeMap<K, Level>,
    slab: &mut Slab,
    key_to_price: impl Fn(&K) -> Price,
    taker_side: Side,
    taker_limit: Price,
    taker_idx: SlotIdx,
    taker_id: OrderId,
    taker_remaining: Size,
    on_fill: &mut impl FnMut(Fill),
) -> MatchOutcome {
    let Some(mut entry) = map.first_entry() else {
        return MatchOutcome::NoMatch;
    };
    let best = key_to_price(entry.key());
    if !crosses(taker_side, taker_limit, best) {
        return MatchOutcome::NoMatch;
    }

    let level = entry.get_mut();
    let Some(head_idx) = level.head else {
        // Empty level shouldn't be in the map (we remove on empty), but if it
        // is, treat as no match.
        return MatchOutcome::NoMatch;
    };
    let head_node = &slab[head_idx];
    let head_remaining = head_node.remaining;
    let head_gen = head_node.generation;
    let head_next = head_node.next;

    let fill_size = taker_remaining.min(head_remaining);

    on_fill(Fill {
        maker_id: OrderId::pack(head_idx.raw(), head_gen),
        taker_id,
        fill_price: best,
        fill_size,
    });

    slab[taker_idx].remaining = Size(taker_remaining.0 - fill_size.0);

    if head_remaining == fill_size {
        if let Some(next) = head_next {
            level.head = Some(next);
            level.total_size = Size(level.total_size.0 - head_remaining.0);
            slab[next].prev = None;
            MatchOutcome::FullyConsumed {
                maker_idx: head_idx,
                level_emptied: false,
            }
        } else {
            entry.remove();
            MatchOutcome::FullyConsumed {
                maker_idx: head_idx,
                level_emptied: true,
            }
        }
    } else {
        slab[head_idx].remaining = Size(head_remaining.0 - fill_size.0);
        level.total_size = Size(level.total_size.0 - fill_size.0);
        MatchOutcome::Partial
    }
}

fn append_to_map_level<K: Ord>(
    map: &mut BTreeMap<K, Level>,
    key: K,
    idx: SlotIdx,
    remaining: Size,
) -> AppendResult {
    match map.entry(key) {
        Entry::Occupied(mut e) => {
            let level = e.get_mut();
            let prev_tail = level.tail;
            level.tail = Some(idx);
            level.total_size = Size(level.total_size.0 + remaining.0);
            AppendResult {
                prev_tail,
                level_created: false,
            }
        }
        Entry::Vacant(e) => {
            e.insert(Level {
                head: Some(idx),
                tail: Some(idx),
                total_size: remaining,
            });
            AppendResult {
                prev_tail: None,
                level_created: true,
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn unlink_from_level<K: Ord>(
    map: &mut BTreeMap<K, Level>,
    key: K,
    idx: SlotIdx,
    prev: Option<SlotIdx>,
    next: Option<SlotIdx>,
    remaining: Size,
    slab: &mut Slab,
) -> UnlinkResult {
    let Entry::Occupied(mut entry) = map.entry(key) else {
        return UnlinkResult::Missing;
    };
    if let Some(p) = prev {
        slab[p].next = next;
    }
    if let Some(n) = next {
        slab[n].prev = prev;
    }
    let level = entry.get_mut();
    if level.head == Some(idx) {
        level.head = next;
    }
    if level.tail == Some(idx) {
        level.tail = prev;
    }
    level.total_size = Size(level.total_size.0 - remaining.0);
    if level.head.is_none() {
        entry.remove();
        UnlinkResult::Removed
    } else {
        UnlinkResult::Kept
    }
}

#[inline]
fn crosses(taker_side: Side, taker_price: Price, opposite_best: Price) -> bool {
    match taker_side {
        Side::Bid => taker_price >= opposite_best,
        Side::Ask => taker_price <= opposite_best,
    }
}
