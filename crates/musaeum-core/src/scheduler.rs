//! The tile scheduler: a pure state machine with no threads, clocks or I/O.
//!
//! The compositor sends its whole wish list, best tile first, and **the latest list
//! wins**. Work that fell off the list is cancelled, finished tiles nobody wants any
//! more are dropped, and work still on the list keeps going rather than restarting.
//!
//! Every slot is always in exactly one place: the free list, a worker's job, the ready
//! queue, or out for delivery. The tests check that this holds under random event
//! sequences, because a leaked slot would slowly starve the whole pipeline.

use std::collections::{HashMap, HashSet, VecDeque};

use musaeum_shm::TileKey;
use thiserror::Error;

/// A tile that crashes this many workers stops being retried.
const MAX_TILE_CRASHES: u8 = 2;
/// How long after delivery a tile counts as still on its way to the compositor. Until it
/// arrives, the compositor's lists keep asking for it, and rendering it again would be
/// wasted work. Long enough for the slowest path, and harmless if a tile really was lost:
/// it just gets rendered again once this expires.
const IN_TRANSIT_GRACE_US: u64 = 500_000;

/// Sends a tile to a worker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DispatchOrder {
    /// Worker to send it to.
    pub worker: usize,
    /// Unique job number.
    pub job_seq: u64,
    /// Slot to render into.
    pub slot: u32,
    /// The tile.
    pub tile: TileKey,
}

/// Stops a job that nobody wants any more.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CancelOrder {
    /// Worker running it.
    pub worker: usize,
    /// The job.
    pub job_seq: u64,
}

/// A finished tile waiting to be delivered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReadyTile {
    /// Slot holding the pixels.
    pub slot: u32,
    /// The tile.
    pub tile: TileKey,
    /// Demand epoch current when the job was dispatched.
    pub epoch: u32,
    /// Microseconds the worker spent on it.
    pub render_us: u32,
    /// Host clock when it finished, in microseconds.
    pub done_us: u32,
}

/// How a job ended, as reported by the worker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TileOutcome {
    /// The slot holds the finished tile.
    Done {
        /// Microseconds the worker spent on it.
        render_us: u32,
        /// Host clock when the report arrived, in microseconds.
        done_us: u32,
    },
    /// The worker stopped because it was asked to.
    Cancelled,
    /// The worker could not render this tile.
    Failed,
}

/// A report that does not match the scheduler's state. Only a buggy or compromised
/// worker can cause one, so the caller should kill that worker.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum SchedulerError {
    /// The worker reported on a job it is not running.
    #[error("worker {worker} reported job {job_seq}, which it is not running")]
    UnknownJob {
        /// Reporting worker.
        worker: usize,
        /// Reported job.
        job_seq: u64,
    },
    /// The worker reported the wrong slot for its job.
    #[error("worker {worker} reported slot {reported} for a job using slot {expected}")]
    WrongSlot {
        /// Reporting worker.
        worker: usize,
        /// Slot in the report.
        reported: u32,
        /// Slot the job was given.
        expected: u32,
    },
    /// The worker claims a cancellation nobody requested.
    #[error("worker {worker} cancelled job {job_seq} without being asked to")]
    UnsolicitedCancel {
        /// Reporting worker.
        worker: usize,
        /// Reported job.
        job_seq: u64,
    },
    /// A slot was released that is not out for delivery.
    #[error("slot {0} was released but is not out for delivery")]
    NotDelivering(u32),
}

#[derive(Clone, Copy, Debug)]
struct Job {
    job_seq: u64,
    slot: u32,
    tile: TileKey,
    epoch: u32,
    cancelled: bool,
}

#[derive(Clone, Copy, Debug)]
enum WorkerState {
    /// Not running: starting up, or lost and waiting to be replaced.
    Absent,
    Idle,
    Busy(Job),
}

/// See the [module documentation](self).
#[derive(Debug)]
pub struct Scheduler {
    workers: Vec<WorkerState>,
    free_slots: Vec<u32>,
    next_job_seq: u64,
    epoch: Option<u32>,
    /// Everything on the latest demand list.
    wanted: HashSet<TileKey>,
    /// Wanted tiles nobody has started on, best first.
    queue: VecDeque<TileKey>,
    /// Tiles with a live job, a finished result, or a delivery under way.
    active: HashSet<TileKey>,
    /// Finished tiles waiting for the delivery side to have room.
    ready: VecDeque<ReadyTile>,
    /// Slots handed to the delivery side, until it releases them.
    delivering: HashMap<u32, TileKey>,
    /// Tiles that failed or crashed workers; asking again will not help.
    poisoned: HashSet<TileKey>,
    crashes: HashMap<TileKey, u8>,
    /// Tiles recently released by the delivery side, and when.
    in_transit: HashMap<TileKey, u64>,
    clock_us: u64,
    discarded_results: u64,
}

impl Scheduler {
    /// Creates a scheduler for `worker_count` workers, all absent, and `slot_count` free slots.
    pub fn new(worker_count: usize, slot_count: u32) -> Self {
        Self {
            workers: vec![WorkerState::Absent; worker_count],
            // Reversed so slot 0 is used first; low slots stay warm in the CPU cache.
            free_slots: (0..slot_count).rev().collect(),
            next_job_seq: 1,
            epoch: None,
            wanted: HashSet::new(),
            queue: VecDeque::new(),
            active: HashSet::new(),
            ready: VecDeque::new(),
            delivering: HashMap::new(),
            poisoned: HashSet::new(),
            crashes: HashMap::new(),
            in_transit: HashMap::new(),
            clock_us: 0,
            discarded_results: 0,
        }
    }

    /// Tells the scheduler what time it is, which is the only way it ever finds out.
    pub fn set_clock(&mut self, now_us: u64) {
        self.clock_us = now_us;
    }

    /// Finished results thrown away because nobody wanted them any more.
    pub fn discarded_results(&self) -> u64 {
        self.discarded_results
    }

    /// A worker finished starting up and can take jobs.
    pub fn on_worker_ready(&mut self, worker: usize) {
        if let Some(state @ WorkerState::Absent) = self.workers.get_mut(worker) {
            *state = WorkerState::Idle;
        }
    }

    /// Replaces the demand list. Out-of-order lists are ignored.
    pub fn on_demand(&mut self, epoch: u32, tiles: &[TileKey], cancels: &mut Vec<CancelOrder>) {
        if let Some(current) = self.epoch
            && (epoch.wrapping_sub(current) as i32) <= 0
        {
            return;
        }
        self.epoch = Some(epoch);

        self.wanted.clear();
        self.wanted.extend(tiles.iter().copied());

        for (worker, state) in self.workers.iter_mut().enumerate() {
            if let WorkerState::Busy(job) = state
                && !job.cancelled
                && !self.wanted.contains(&job.tile)
            {
                job.cancelled = true;
                self.active.remove(&job.tile);
                cancels.push(CancelOrder {
                    worker,
                    job_seq: job.job_seq,
                });
            }
        }

        // Tiles still waiting for delivery are the only finished work that can be
        // recalled; tiles already handed over reach the compositor regardless.
        let wanted = &self.wanted;
        let active = &mut self.active;
        let free_slots = &mut self.free_slots;
        self.ready.retain(|tile| {
            let keep = wanted.contains(&tile.tile);
            if !keep {
                active.remove(&tile.tile);
                free_slots.push(tile.slot);
            }
            keep
        });

        let now = self.clock_us;
        self.in_transit
            .retain(|_, released_at| now.saturating_sub(*released_at) < IN_TRANSIT_GRACE_US);

        self.queue.clear();
        let mut queued = HashSet::with_capacity(tiles.len());
        for &tile in tiles {
            if !self.active.contains(&tile)
                && !self.poisoned.contains(&tile)
                && !self.in_transit.contains_key(&tile)
                && queued.insert(tile)
            {
                self.queue.push_back(tile);
            }
        }
    }

    /// Records how a job ended.
    pub fn on_outcome(
        &mut self,
        worker: usize,
        job_seq: u64,
        slot: u32,
        outcome: TileOutcome,
    ) -> Result<(), SchedulerError> {
        let job = match self.workers.get(worker) {
            Some(WorkerState::Busy(job)) if job.job_seq == job_seq => *job,
            _ => return Err(SchedulerError::UnknownJob { worker, job_seq }),
        };
        if job.slot != slot {
            return Err(SchedulerError::WrongSlot {
                worker,
                reported: slot,
                expected: job.slot,
            });
        }
        if outcome == TileOutcome::Cancelled && !job.cancelled {
            return Err(SchedulerError::UnsolicitedCancel { worker, job_seq });
        }

        self.workers[worker] = WorkerState::Idle;
        match outcome {
            TileOutcome::Done { render_us, done_us } => {
                // A job cancelled too late to stop may still be useful: its tile could
                // have been demanded again in the meantime, with no newer job yet.
                let usable = !job.cancelled
                    || (self.wanted.contains(&job.tile) && !self.active.contains(&job.tile));
                if usable {
                    self.active.insert(job.tile);
                    self.queue.retain(|tile| *tile != job.tile);
                    self.ready.push_back(ReadyTile {
                        slot,
                        tile: job.tile,
                        epoch: job.epoch,
                        render_us,
                        done_us,
                    });
                } else {
                    self.discarded_results += 1;
                    self.free_slots.push(slot);
                }
            }
            TileOutcome::Cancelled => self.free_slots.push(slot),
            TileOutcome::Failed => {
                if !job.cancelled {
                    self.active.remove(&job.tile);
                }
                self.poisoned.insert(job.tile);
                self.queue.retain(|tile| *tile != job.tile);
                self.free_slots.push(slot);
            }
        }
        Ok(())
    }

    /// The delivery side has finished with a slot.
    pub fn on_released(&mut self, slot: u32) -> Result<(), SchedulerError> {
        let tile = self
            .delivering
            .remove(&slot)
            .ok_or(SchedulerError::NotDelivering(slot))?;
        self.active.remove(&tile);
        self.in_transit.insert(tile, self.clock_us);
        self.free_slots.push(slot);
        Ok(())
    }

    /// A worker process is gone. Its slot can no longer be written, so it is free again.
    pub fn on_worker_lost(&mut self, worker: usize) {
        let Some(state) = self.workers.get_mut(worker) else {
            return;
        };
        if let WorkerState::Busy(job) = *state {
            self.free_slots.push(job.slot);
            if !job.cancelled {
                self.active.remove(&job.tile);
                let crashes = self.crashes.entry(job.tile).or_insert(0);
                *crashes += 1;
                if *crashes >= MAX_TILE_CRASHES {
                    self.poisoned.insert(job.tile);
                } else if self.wanted.contains(&job.tile) {
                    self.queue.push_front(job.tile);
                }
            }
        }
        *state = WorkerState::Absent;
    }

    /// The next job to start, if a worker, a slot and a wanted tile are all available.
    pub fn next_dispatch(&mut self) -> Option<DispatchOrder> {
        if self.queue.is_empty() || self.free_slots.is_empty() {
            return None;
        }
        let worker = self
            .workers
            .iter()
            .position(|state| matches!(state, WorkerState::Idle))?;
        let tile = self.queue.pop_front()?;
        let slot = self.free_slots.pop()?;
        let job_seq = self.next_job_seq;
        self.next_job_seq += 1;

        self.active.insert(tile);
        self.workers[worker] = WorkerState::Busy(Job {
            job_seq,
            slot,
            tile,
            epoch: self.epoch.unwrap_or(0),
            cancelled: false,
        });
        Some(DispatchOrder {
            worker,
            job_seq,
            slot,
            tile,
        })
    }

    /// The next finished tile to deliver. It stays accounted for until
    /// [`on_released`](Self::on_released).
    pub fn next_ready(&mut self) -> Option<ReadyTile> {
        let tile = self.ready.pop_front()?;
        self.delivering.insert(tile.slot, tile.tile);
        Some(tile)
    }

    /// Number of jobs currently running.
    pub fn busy_workers(&self) -> usize {
        self.workers
            .iter()
            .filter(|state| matches!(state, WorkerState::Busy(_)))
            .count()
    }

    /// Whether anything is queued, running, or still on its way out.
    pub fn has_outstanding_work(&self) -> bool {
        !self.queue.is_empty()
            || !self.ready.is_empty()
            || !self.delivering.is_empty()
            || self.busy_workers() > 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tile(page: u32) -> TileKey {
        TileKey {
            page,
            lod: 0,
            x: 0,
            y: 0,
        }
    }

    fn done() -> TileOutcome {
        TileOutcome::Done {
            render_us: 10,
            done_us: 20,
        }
    }

    fn scheduler(workers: usize, slots: u32) -> Scheduler {
        let mut scheduler = Scheduler::new(workers, slots);
        for worker in 0..workers {
            scheduler.on_worker_ready(worker);
        }
        scheduler
    }

    fn demand(scheduler: &mut Scheduler, epoch: u32, pages: &[u32]) -> Vec<CancelOrder> {
        let tiles: Vec<_> = pages.iter().map(|&page| tile(page)).collect();
        let mut cancels = Vec::new();
        scheduler.on_demand(epoch, &tiles, &mut cancels);
        cancels
    }

    fn assert_slots_conserved(scheduler: &Scheduler, total: u32) {
        let running = scheduler
            .workers
            .iter()
            .filter(|state| matches!(state, WorkerState::Busy(_)))
            .count();
        let accounted = scheduler.free_slots.len()
            + running
            + scheduler.ready.len()
            + scheduler.delivering.len();
        assert_eq!(accounted, total as usize, "a slot leaked or was duplicated");

        let mut seen = HashSet::new();
        let running = scheduler.workers.iter().filter_map(|state| match state {
            WorkerState::Busy(job) => Some(job.slot),
            _ => None,
        });
        for slot in scheduler
            .free_slots
            .iter()
            .copied()
            .chain(running)
            .chain(scheduler.ready.iter().map(|tile| tile.slot))
            .chain(scheduler.delivering.keys().copied())
        {
            assert!(seen.insert(slot), "slot {slot} is in two places at once");
        }
    }

    #[test]
    fn dispatches_in_demand_order_up_to_the_available_workers() {
        let mut s = scheduler(2, 8);
        demand(&mut s, 1, &[10, 11, 12]);

        let first = s.next_dispatch().unwrap();
        let second = s.next_dispatch().unwrap();
        assert_eq!((first.tile, first.worker), (tile(10), 0));
        assert_eq!((second.tile, second.worker), (tile(11), 1));
        assert!(second.job_seq > first.job_seq);
        assert_eq!(s.next_dispatch(), None, "both workers are busy");
    }

    #[test]
    fn absent_workers_receive_nothing() {
        let mut s = Scheduler::new(1, 8);
        demand(&mut s, 1, &[1]);
        assert_eq!(s.next_dispatch(), None);
        s.on_worker_ready(0);
        assert!(s.next_dispatch().is_some());
    }

    #[test]
    fn runs_out_of_slots_gracefully_and_recovers_on_release() {
        let mut s = scheduler(4, 1);
        demand(&mut s, 1, &[1, 2]);

        let job = s.next_dispatch().unwrap();
        assert_eq!(s.next_dispatch(), None, "the only slot is taken");

        s.on_outcome(job.worker, job.job_seq, job.slot, done())
            .unwrap();
        assert_eq!(
            s.next_dispatch(),
            None,
            "the finished tile still holds the slot"
        );

        let ready = s.next_ready().unwrap();
        s.on_released(ready.slot).unwrap();
        assert_eq!(s.next_dispatch().unwrap().tile, tile(2));
    }

    #[test]
    fn a_new_demand_cancels_only_work_that_fell_off_the_list() {
        let mut s = scheduler(2, 8);
        demand(&mut s, 1, &[1, 2]);
        let kept = s.next_dispatch().unwrap();
        let dropped = s.next_dispatch().unwrap();

        let cancels = demand(&mut s, 2, &[1, 3]);
        assert_eq!(
            cancels,
            vec![CancelOrder {
                worker: dropped.worker,
                job_seq: dropped.job_seq
            }]
        );

        s.on_outcome(kept.worker, kept.job_seq, kept.slot, done())
            .unwrap();
        assert_eq!(s.next_ready().unwrap().tile, tile(1));
    }

    #[test]
    fn work_in_progress_is_not_restarted_when_it_stays_wanted() {
        let mut s = scheduler(2, 8);
        demand(&mut s, 1, &[1]);
        s.next_dispatch().unwrap();

        assert!(demand(&mut s, 2, &[1, 2]).is_empty());
        assert_eq!(s.next_dispatch().unwrap().tile, tile(2));
        assert_eq!(
            s.next_dispatch(),
            None,
            "tile 1 must not start a second job"
        );
    }

    #[test]
    fn stale_demands_are_ignored_even_across_the_epoch_wrap() {
        let mut s = scheduler(1, 8);
        demand(&mut s, u32::MAX, &[1]);
        demand(&mut s, u32::MAX - 1, &[2]);
        assert_eq!(s.next_dispatch().unwrap().tile, tile(1));

        let mut s = scheduler(1, 8);
        demand(&mut s, u32::MAX, &[1]);
        demand(&mut s, 0, &[2]);
        assert_eq!(
            s.next_dispatch().unwrap().tile,
            tile(2),
            "epoch 0 follows u32::MAX"
        );
    }

    #[test]
    fn cancelled_job_frees_its_slot_only_when_the_worker_confirms() {
        let mut s = scheduler(1, 1);
        demand(&mut s, 1, &[1]);
        let job = s.next_dispatch().unwrap();
        demand(&mut s, 2, &[2]);

        // The worker may still be writing, so the slot must not be reused yet.
        assert_eq!(s.next_dispatch(), None);
        s.on_outcome(job.worker, job.job_seq, job.slot, TileOutcome::Cancelled)
            .unwrap();
        assert_eq!(s.next_dispatch().unwrap().tile, tile(2));
    }

    #[test]
    fn late_result_of_a_cancelled_job_is_used_if_the_tile_is_wanted_again() {
        let mut s = scheduler(1, 8);
        demand(&mut s, 1, &[1]);
        let job = s.next_dispatch().unwrap();
        demand(&mut s, 2, &[2]);
        demand(&mut s, 3, &[1]);

        // The cancel lost the race and the worker finished the tile anyway.
        s.on_outcome(job.worker, job.job_seq, job.slot, done())
            .unwrap();
        assert_eq!(s.next_ready().unwrap().tile, tile(1));
        assert_eq!(s.next_dispatch(), None, "no second render of the same tile");
    }

    #[test]
    fn late_result_of_a_cancelled_job_is_dropped_if_nobody_wants_it() {
        let mut s = scheduler(1, 1);
        demand(&mut s, 1, &[1]);
        let job = s.next_dispatch().unwrap();
        demand(&mut s, 2, &[]);

        s.on_outcome(job.worker, job.job_seq, job.slot, done())
            .unwrap();
        assert_eq!(s.next_ready(), None);
        assert_slots_conserved(&s, 1);
        assert_eq!(s.free_slots.len(), 1);
    }

    #[test]
    fn finished_tiles_nobody_wants_are_recalled_before_delivery() {
        let mut s = scheduler(1, 1);
        demand(&mut s, 1, &[1]);
        let job = s.next_dispatch().unwrap();
        s.on_outcome(job.worker, job.job_seq, job.slot, done())
            .unwrap();

        demand(&mut s, 2, &[2]);
        assert_eq!(s.next_ready(), None);
        assert_eq!(s.next_dispatch().unwrap().tile, tile(2));
    }

    #[test]
    fn tiles_on_their_way_out_are_not_rendered_twice() {
        let mut s = scheduler(1, 8);
        demand(&mut s, 1, &[1]);
        let job = s.next_dispatch().unwrap();
        s.on_outcome(job.worker, job.job_seq, job.slot, done())
            .unwrap();
        s.next_ready().unwrap();

        demand(&mut s, 2, &[1]);
        assert_eq!(s.next_dispatch(), None);
    }

    #[test]
    fn tiles_still_on_their_way_to_the_compositor_are_not_rendered_again() {
        let mut s = scheduler(1, 8);
        s.set_clock(1_000);
        demand(&mut s, 1, &[1]);
        let job = s.next_dispatch().unwrap();
        s.on_outcome(job.worker, job.job_seq, job.slot, done())
            .unwrap();
        let ready = s.next_ready().unwrap();
        s.on_released(ready.slot).unwrap();

        // The compositor has not received it yet, so its next list still asks for it.
        s.set_clock(1_000 + IN_TRANSIT_GRACE_US - 1);
        demand(&mut s, 2, &[1]);
        assert_eq!(s.next_dispatch(), None);

        // If it really got lost, or was evicted later, it is rendered again.
        s.set_clock(1_000 + IN_TRANSIT_GRACE_US);
        demand(&mut s, 3, &[1]);
        assert_eq!(s.next_dispatch().unwrap().tile, tile(1));
    }

    #[test]
    fn discarded_results_are_counted() {
        let mut s = scheduler(1, 8);
        demand(&mut s, 1, &[1]);
        let job = s.next_dispatch().unwrap();
        demand(&mut s, 2, &[]);
        s.on_outcome(job.worker, job.job_seq, job.slot, done())
            .unwrap();
        assert_eq!(s.discarded_results(), 1);
    }

    #[test]
    fn failed_tiles_are_not_retried() {
        let mut s = scheduler(1, 8);
        demand(&mut s, 1, &[1, 2]);
        let job = s.next_dispatch().unwrap();
        s.on_outcome(job.worker, job.job_seq, job.slot, TileOutcome::Failed)
            .unwrap();

        demand(&mut s, 2, &[1, 2]);
        assert_eq!(s.next_dispatch().unwrap().tile, tile(2));
        assert_eq!(s.next_dispatch(), None);
    }

    #[test]
    fn a_tile_is_retried_after_one_crash_and_abandoned_after_two() {
        let mut s = scheduler(1, 8);
        demand(&mut s, 1, &[1, 2]);

        let first = s.next_dispatch().unwrap();
        assert_eq!(first.tile, tile(1));
        s.on_worker_lost(0);
        assert_slots_conserved(&s, 8);
        s.on_worker_ready(0);

        let second = s.next_dispatch().unwrap();
        assert_eq!(second.tile, tile(1), "retried first, ahead of the queue");
        s.on_worker_lost(0);
        s.on_worker_ready(0);

        assert_eq!(
            s.next_dispatch().unwrap().tile,
            tile(2),
            "tile 1 is poisoned"
        );
    }

    #[test]
    fn lost_worker_with_a_cancelled_job_does_not_count_as_a_crash_of_that_tile() {
        let mut s = scheduler(1, 8);
        demand(&mut s, 1, &[1]);
        s.next_dispatch().unwrap();
        demand(&mut s, 2, &[]);
        s.on_worker_lost(0);
        assert!(s.crashes.is_empty());
        assert_slots_conserved(&s, 8);
    }

    #[test]
    fn misbehaving_workers_are_detected_without_corrupting_state() {
        let mut s = scheduler(2, 8);
        demand(&mut s, 1, &[1]);
        let job = s.next_dispatch().unwrap();

        assert_eq!(
            s.on_outcome(1, job.job_seq, job.slot, done()),
            Err(SchedulerError::UnknownJob {
                worker: 1,
                job_seq: job.job_seq
            })
        );
        assert_eq!(
            s.on_outcome(job.worker, job.job_seq + 1, job.slot, done()),
            Err(SchedulerError::UnknownJob {
                worker: job.worker,
                job_seq: job.job_seq + 1
            })
        );
        assert_eq!(
            s.on_outcome(job.worker, job.job_seq, job.slot + 1, done()),
            Err(SchedulerError::WrongSlot {
                worker: job.worker,
                reported: job.slot + 1,
                expected: job.slot
            })
        );
        assert_eq!(
            s.on_outcome(job.worker, job.job_seq, job.slot, TileOutcome::Cancelled),
            Err(SchedulerError::UnsolicitedCancel {
                worker: job.worker,
                job_seq: job.job_seq
            })
        );
        assert_eq!(
            s.on_outcome(99, job.job_seq, job.slot, done()),
            Err(SchedulerError::UnknownJob {
                worker: 99,
                job_seq: job.job_seq
            })
        );
        assert_eq!(
            s.on_released(job.slot),
            Err(SchedulerError::NotDelivering(job.slot))
        );
        assert_slots_conserved(&s, 8);

        s.on_outcome(job.worker, job.job_seq, job.slot, done())
            .unwrap();
        assert_eq!(s.next_ready().unwrap().tile, tile(1));
    }

    /// Small deterministic generator, so a failure reproduces exactly.
    struct XorShift(u64);

    impl XorShift {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn below(&mut self, limit: u64) -> u64 {
            self.next() % limit
        }
    }

    /// Throws random, legal event sequences at the scheduler and checks its invariants.
    #[test]
    fn invariants_hold_under_random_event_sequences() {
        const WORKERS: usize = 4;
        const SLOTS: u32 = 6;

        for seed in 1..=64u64 {
            let mut rng = XorShift(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
            let mut s = scheduler(WORKERS, SLOTS);
            let mut running: Vec<DispatchOrder> = Vec::new();
            let mut out_for_delivery: Vec<u32> = Vec::new();
            let mut epoch = 0u32;

            for _ in 0..4_000 {
                match rng.below(7) {
                    0 => {
                        epoch += 1;
                        let count = rng.below(9);
                        let pages: Vec<u32> = (0..count).map(|_| rng.below(16) as u32).collect();
                        demand(&mut s, epoch, &pages);
                    }
                    1 | 2 => {
                        if let Some(order) = s.next_dispatch() {
                            assert!(
                                !running.iter().any(|job| job.worker == order.worker),
                                "a worker was given two jobs"
                            );
                            running.push(order);
                        }
                    }
                    3 if !running.is_empty() => {
                        let job = running.swap_remove(rng.below(running.len() as u64) as usize);
                        let state = s.workers[job.worker];
                        let cancelled = matches!(state, WorkerState::Busy(j) if j.cancelled);
                        let outcome = match rng.below(4) {
                            0 if cancelled => TileOutcome::Cancelled,
                            1 => TileOutcome::Failed,
                            _ => done(),
                        };
                        s.on_outcome(job.worker, job.job_seq, job.slot, outcome)
                            .unwrap();
                    }
                    4 => {
                        if let Some(ready) = s.next_ready() {
                            out_for_delivery.push(ready.slot);
                        }
                    }
                    5 if !out_for_delivery.is_empty() => {
                        let at = rng.below(out_for_delivery.len() as u64) as usize;
                        s.on_released(out_for_delivery.swap_remove(at)).unwrap();
                    }
                    6 if rng.below(16) == 0 => {
                        let worker = rng.below(WORKERS as u64) as usize;
                        running.retain(|job| job.worker != worker);
                        s.on_worker_lost(worker);
                        s.on_worker_ready(worker);
                    }
                    _ => {}
                }
                assert_slots_conserved(&s, SLOTS);
            }
        }
    }
}
