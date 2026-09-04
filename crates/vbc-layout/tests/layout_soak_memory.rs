//! The memory soak: that half a million cases of the invariant search leave the heap where they
//! found it.
//!
//! A search that kept a little of every case would still satisfy every invariant and would still
//! finish at the scale a default run can afford. It would be met later, as a soak that runs out of
//! memory on a machine nobody can reproduce, so it is looked for here instead: the live heap is
//! read between batches of cases, and a run whose heap climbs with the work behind it fails.
//!
//! The heap is read through an allocator this target installs over its whole binary, which is why
//! the memory soak is a target of its own rather than another test beside the search: it counts
//! the bytes a run is still holding rather than the bytes the process was given, so a run is
//! judged on what it kept and not on what the system allocator chose not to hand back.
//!
//! What keeps the check from passing everything is read here too. A run that keeps a megabyte a
//! batch has to fail it, and a run that allocates a great deal and frees all of it has to pass,
//! since a counter that never subtracted would call the second one a leak and a check that never
//! complained would call the first one clean.

// The fuzz harness is shared with the invariant search next door, which is written against all of
// it; a memory soak needs the search, the layout it searches, and nothing else.
#[allow(dead_code)]
mod fuzz;

use std::alloc::{GlobalAlloc, Layout as Allocation, System};
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use fuzz::harness::{sharded_search, Seed};
use fuzz::reference::WholeDocumentLayout;

/// The allocator the whole binary runs on, which is what makes the live heap readable.
#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// The bytes this process has allocated and not yet freed.
static LIVE: AtomicUsize = AtomicUsize::new(0);

/// Held for as long as a run is being measured, so that two measured runs never overlap and read
/// each other's allocations as their own growth.
static MEASURING: Mutex<()> = Mutex::new(());

/// The seed the measured search runs from.
const SEED: Seed = Seed::new(0x6D65_6D6F_7279_0001);

/// The shards each measured batch is split across, which is the way the soak next door runs the
/// same search, so that what is measured is what continuous integration actually runs.
const SHARDS: NonZeroU32 = NonZeroU32::new(16).expect("a search runs on at least one shard");

/// The batches the heap is read between, and the cases each batch searches. Long enough that a
/// search keeping even a few bytes a case would be holding megabytes by the last reading.
const BATCHES: usize = 20;
const CASES_PER_BATCH: u32 = 25_000;

/// The most the live heap may sit above where the first batch left it. It is a fixed number of
/// bytes rather than a share of anything, because a search that keeps nothing per case leaves the
/// heap where it found it however many cases it is given.
const ALLOWANCE: usize = 256 * 1024;

/// The bytes the leaking run keeps for every batch it runs, far enough past [`ALLOWANCE`] that a
/// check which caught it by luck would have to be lucky twenty times over.
const KEPT_PER_BATCH: usize = 1024 * 1024;

/// An allocator that counts the bytes live, leaving the allocation itself to the system.
struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, allocation: Allocation) -> *mut u8 {
        let block = unsafe { System.alloc(allocation) };
        if !block.is_null() {
            LIVE.fetch_add(allocation.size(), Ordering::Relaxed);
        }

        block
    }

    unsafe fn dealloc(&self, block: *mut u8, allocation: Allocation) {
        unsafe { System.dealloc(block, allocation) };
        LIVE.fetch_sub(allocation.size(), Ordering::Relaxed);
    }

    unsafe fn realloc(&self, block: *mut u8, allocation: Allocation, new_size: usize) -> *mut u8 {
        let moved = unsafe { System.realloc(block, allocation, new_size) };
        if !moved.is_null() {
            LIVE.fetch_add(new_size, Ordering::Relaxed);
            LIVE.fetch_sub(allocation.size(), Ordering::Relaxed);
        }

        moved
    }
}

/// Runs `work` once for each of `batches` batches, reading the live heap after every one.
///
/// The first reading is the baseline the rest are judged against, so whatever a run allocates once
/// and keeps is already counted before anything is compared.
///
/// # Returns
///
/// The live heap after each batch, in the order the batches ran.
///
/// # Panics
///
/// Panics if a measured run panicked while holding the measurement to itself.
fn heap_after_each_batch(batches: usize, mut work: impl FnMut(usize)) -> Vec<usize> {
    let _measuring = MEASURING
        .lock()
        .expect("a measured run does not panic while holding the measurement");

    (0..batches)
        .map(|batch| {
            work(batch);
            LIVE.load(Ordering::Relaxed)
        })
        .collect()
}

/// # Returns
///
/// How far the live heap climbed past the first reading of `samples`, one complaint for each
/// reading that climbed, and empty if none did.
fn growth(samples: &[usize]) -> Vec<String> {
    let Some(&baseline) = samples.first() else {
        return vec!["nothing was measured at all".to_owned()];
    };

    samples
        .iter()
        .enumerate()
        .skip(1)
        .filter(|(_, &live)| baseline + ALLOWANCE < live)
        .map(|(batch, &live)| {
            format!(
                "batch {batch} left {live} bytes live, {} past the {baseline} the first batch left",
                live - baseline
            )
        })
        .collect()
}

#[test]
#[ignore = "minutes of search; continuous integration runs it on every pull request"]
fn the_long_run_leaves_the_heap_where_it_found_it() {
    let samples = heap_after_each_batch(BATCHES, |batch| {
        let seed = SEED.shard(u32::try_from(batch).expect("a batch is counted in tens"));
        if let Err(failure) = sharded_search(&WholeDocumentLayout, seed, CASES_PER_BATCH, SHARDS) {
            panic!("the reference layout broke an invariant:\n{failure}");
        }
    });

    assert_eq!(Vec::<String>::new(), growth(&samples));
}

#[test]
fn a_run_that_keeps_a_little_of_every_batch_is_caught() {
    let mut kept: Vec<Box<[u8]>> = Vec::new();
    let samples = heap_after_each_batch(BATCHES, |_| {
        kept.push(vec![0_u8; KEPT_PER_BATCH].into_boxed_slice());
    });

    assert_ne!(
        Vec::<String>::new(),
        growth(&samples),
        "a run that kept {KEPT_PER_BATCH} bytes a batch was read as leaving the heap alone"
    );
}

#[test]
fn a_run_that_frees_everything_it_takes_is_passed() {
    let samples = heap_after_each_batch(BATCHES, |batch| {
        drop(vec![0_u8; KEPT_PER_BATCH * (1 + batch)]);
    });

    assert_eq!(Vec::<String>::new(), growth(&samples));
}
