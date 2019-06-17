//! An unbounded set of futures.
//!
//! This module is only available when the `std` or `alloc` feature of this
//! library is activated, and it is activated by default.

use crate::task::{AtomicWaker};
use futures_core::future::{Future, FutureObj, LocalFutureObj};
use futures_core::stream::{FusedStream, Stream};
use futures_core::task::{Context, Poll, Spawn, LocalSpawn, SpawnError};
use core::cell::UnsafeCell;
use core::fmt::{self, Debug};
use core::iter::FromIterator;
use core::marker::PhantomData;
use core::mem;
use core::pin::Pin;
use core::ptr;
use core::sync::atomic::Ordering::SeqCst;
use core::sync::atomic::{AtomicPtr, AtomicBool};
use alloc::sync::{Arc, Weak};

mod abort;

mod iter;
pub use self::iter::{IterMut, IterPinMut};

mod task;
use self::task::{
    Task,
    QUESTATE_UNQUEUED,
    QUESTATE_QUEUED,
    QUESTATE_POLLING,
    QUESTATE_REPOLL,
};

mod ready_to_run_queue;
use self::ready_to_run_queue::{ReadyToRunQueue, Dequeue};

/// Constant used for a `FuturesUnordered` to indicate we are empty and have
/// yielded a `None` element so can return `true` from
/// `FusedStream::is_terminated`
///
/// It is safe to not check for this when incrementing as even a ZST future will
/// have a `Task` allocated for it, so we cannot ever reach usize::max_value()
/// without running out of ram.
const TERMINATED_SENTINEL_LENGTH: usize = usize::max_value();

/// A set of futures which may complete in any order.
///
/// This structure is optimized to manage a large number of futures.
/// Futures managed by [`FuturesUnordered`] will only be polled when they
/// generate wake-up notifications. This reduces the required amount of work
/// needed to poll large numbers of futures.
///
/// [`FuturesUnordered`] can be filled by [`collect`](Iterator::collect)ing an
/// iterator of futures into a [`FuturesUnordered`], or by
/// [`push`](FuturesUnordered::push)ing futures onto an existing
/// [`FuturesUnordered`]. When new futures are added,
/// [`poll_next`](Stream::poll_next) must be called in order to begin receiving
/// wake-ups for new futures.
///
/// Note that you can create a ready-made [`FuturesUnordered`] via the
/// [`collect`](Iterator::collect) method, or you can start with an empty set
/// with the [`FuturesUnordered::new`] constructor.
///
/// This type is only available when the `std` or `alloc` feature of this
/// library is activated, and it is activated by default.
#[must_use = "streams do nothing unless polled"]
pub struct FuturesUnordered<Fut> {
    ready_to_run_queue: Arc<ReadyToRunQueue<Fut>>,
    len: usize,
    head_active: *const Task<Fut>,
    head_empty: *const Task<Fut>,
}

// Link a task into the head of a task list
unsafe fn link<Fut>(task: Arc<Task<Fut>>, head: &mut *const Task<Fut>) -> *const Task<Fut> {
    let ptr = Arc::into_raw(task);
    *(*ptr).next_all.get() = *head;
    if !(*head).is_null() {
        *(*(*head)).prev_all.get() = ptr;
    }
    *head = ptr;
    ptr
}


unsafe impl<Fut: Send> Send for FuturesUnordered<Fut> {}
unsafe impl<Fut: Sync> Sync for FuturesUnordered<Fut> {}
impl<Fut> Unpin for FuturesUnordered<Fut> {}

impl Spawn for FuturesUnordered<FutureObj<'_, ()>> {
    fn spawn_obj(&mut self, future_obj: FutureObj<'static, ()>)
        -> Result<(), SpawnError>
    {
        self.push(future_obj);
        Ok(())
    }
}

impl LocalSpawn for FuturesUnordered<LocalFutureObj<'_, ()>> {
    fn spawn_local_obj(&mut self, future_obj: LocalFutureObj<'static, ()>)
        -> Result<(), SpawnError>
    {
        self.push(future_obj);
        Ok(())
    }
}

// FuturesUnordered is implemented using two linked lists. One which links all
// futures managed by a `FuturesUnordered` and one that tracks futures that have
// been scheduled for polling. The first linked list is not thread safe and is
// only accessed by the thread that owns the `FuturesUnordered` value. The
// second linked list is an implementation of the intrusive MPSC queue algorithm
// described by 1024cores.net.
//
// When a future is submitted to the set, a task is allocated and inserted in
// both linked lists. The next call to `poll_next` will (eventually) see this
// task and call `poll` on the future.
//
// Before a managed future is polled, the current context's waker is replaced
// with one that is aware of the specific future being run. This ensures that
// wake-up notifications generated by that specific future are visible to
// `FuturesUnordered`. When a wake-up notification is received, the task is
// inserted into the ready to run queue, so that its future can be polled later.
//
// Each task is wrapped in an `Arc` and thereby atomically reference counted.
// Also, each task contains an `AtomicBool` which acts as a flag that indicates
// whether the task is currently inserted in the atomic queue. When a wake-up
// notifiaction is received, the task will only be inserted into the ready to
// run queue if it isn't inserted already.

impl<Fut: Future> FuturesUnordered<Fut> {
    /// Constructs a new, empty [`FuturesUnordered`].
    ///
    /// The returned [`FuturesUnordered`] does not contain any futures.
    /// In this state, [`FuturesUnordered::poll_next`](Stream::poll_next) will
    /// return [`Poll::Ready(None)`](Poll::Ready).
    pub fn new() -> FuturesUnordered<Fut> {
        let stub = Arc::new(Task {
            future: UnsafeCell::new(None),
            next_all: UnsafeCell::new(ptr::null()),
            prev_all: UnsafeCell::new(ptr::null()),
            next_ready_to_run: AtomicPtr::new(ptr::null_mut()),
            queued: AtomicBool::new(QUESTATE_QUEUED),
            ready_to_run_queue: Weak::new(),
        });
        let stub_ptr = &*stub as *const Task<Fut>;
        let ready_to_run_queue = Arc::new(ReadyToRunQueue {
            waker: AtomicWaker::new(),
            head: AtomicPtr::new(stub_ptr as *mut _),
            tail: UnsafeCell::new(stub_ptr),
            stub,
        });

        FuturesUnordered {
            len: 0,
            head_active: ptr::null_mut(),
            head_empty: ptr::null_mut(),
            ready_to_run_queue,
        }
    }
}

impl<Fut: Future> Default for FuturesUnordered<Fut> {
    fn default() -> FuturesUnordered<Fut> {
        FuturesUnordered::new()
    }
}

impl<Fut> FuturesUnordered<Fut> {
    /// Returns the number of futures contained in the set.
    ///
    /// This represents the total number of in-flight futures.
    pub fn len(&self) -> usize {
        if self.len == TERMINATED_SENTINEL_LENGTH { 0 } else { self.len }
    }

    /// Returns `true` if the set contains no futures.
    pub fn is_empty(&self) -> bool {
        self.len == 0 || self.len == TERMINATED_SENTINEL_LENGTH
    }

    fn unlink_empty(&mut self) -> Option<Arc<Task<Fut>>> {
        if self.head_empty.is_null() { return None }
        let old_head = self.head_empty;
        self.head_empty = unsafe { *(*self.head_empty).next_all.get() };
        Some(unsafe { Arc::from_raw(old_head) })
    }

    /// Push a future into the set.
    ///
    /// This method adds the given future to the set. This method will not
    /// call [`poll`](core::future::Future::poll) on the submitted future. The caller must
    /// ensure that [`FuturesUnordered::poll_next`](Stream::poll_next) is called
    /// in order to receive wake-up notifications for the given future.
    pub fn push(&mut self, future: Fut) {
        let task = match self.unlink_empty() {
            Some(task) => {
                task.questate.store(QUESTATE_QUEUED, SeqCst);
                unsafe {
                    *task.future.get() = Some(future);
                }
                task
            },
            None => {
                Arc::new(Task {
                    future: UnsafeCell::new(Some(future)),
                    next_all: UnsafeCell::new(ptr::null_mut()),
                    prev_all: UnsafeCell::new(ptr::null_mut()),
                    next_ready_to_run: AtomicPtr::new(ptr::null_mut()),
                    questate: AtomicUsize::new(QUESTATE_QUEUED),
                    ready_to_run_queue: Arc::downgrade(&self.ready_to_run_queue),
                })
            },
        };

        // If we've previously marked ourselves as terminated we need to reset
        // len to 0 to track it correctly
        if self.len == TERMINATED_SENTINEL_LENGTH {
            self.len = 0;
        }

        // Right now our task has a strong reference count of 1. We transfer
        // ownership of this reference count to our internal linked list
        // and we'll reclaim ownership through the `unlink` method below.
        let ptr = self.link(task);

        // We'll need to get the future "into the system" to start tracking it,
        // e.g. getting its wake-up notifications going to us tracking which
        // futures are ready. To do that we enqueue it for polling here.
        self.ready_to_run_queue.enqueue(ptr);
    }

    /// Returns an iterator that allows modifying each future in the set.
    pub fn iter_mut(&mut self) -> IterMut<'_, Fut> where Fut: Unpin {
        IterMut(Pin::new(self).iter_pin_mut())
    }

    /// Returns an iterator that allows modifying each future in the set.
    pub fn iter_pin_mut<'a>(self: Pin<&'a mut Self>) -> IterPinMut<'a, Fut> {
        IterPinMut {
            task: self.head_active,
            len: self.len(),
            _marker: PhantomData
        }
    }

    /// Releases the task. It destorys the future inside.
    ///
    /// If `save`, the `Arc<Task>` will be added to the head_empty list.
    /// Otherwise, either drops the `Arc<Task>`.
    /// The task this method is called on must have been unlinked before,
    /// and must not be on the ready-to-run queue.
    fn release_task(&mut self, task: Arc<Task<Fut>>, save: bool) {
        // `release_task` must only be called on unlinked tasks
        unsafe {
            debug_assert!((*task.next_all.get()).is_null());
            debug_assert!((*task.prev_all.get()).is_null());
        }

        // Drop the future, even if it hasn't finished yet. This is safe
        // because we're dropping the future on the thread that owns
        // `FuturesUnordered`, which correctly tracks `Fut`'s lifetimes and
        // such.
        unsafe {
            // Set to `None` rather than `take()`ing to prevent moving the
            // future.
            *task.future.get() = None;
        }

        if save {
            self.link_empty(task);
        }
    }

    /// Insert a new task into the internal linked list.
    fn link(&mut self, task: Arc<Task<Fut>>) -> *const Task<Fut> {
        self.len += 1;
        unsafe { link(task, &mut self.head_active) }
    }

    /// Insert a task into the empty list.
    fn link_empty(&mut self, task: Arc<Task<Fut>>) {
        let _ptr = unsafe { link(task, &mut self.head_empty) };
    }

    /// Remove the task from the linked list tracking all tasks currently
    /// managed by `FuturesUnordered`.
    /// This method is unsafe because it has be guaranteed that `task` is a
    /// valid pointer.
    unsafe fn unlink(&mut self, task: *const Task<Fut>) -> Arc<Task<Fut>> {
        let task = Arc::from_raw(task);
        let next = *task.next_all.get();
        let prev = *task.prev_all.get();
        *task.next_all.get() = ptr::null_mut();
        *task.prev_all.get() = ptr::null_mut();

        if !next.is_null() {
            *(*next).prev_all.get() = prev;
        }

        if !prev.is_null() {
            *(*prev).next_all.get() = next;
        } else {
            self.head_active = next;
        }
        self.len -= 1;
        task
    }
}

impl<Fut: Future> Stream for FuturesUnordered<Fut> {
    type Item = Fut::Output;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>)
        -> Poll<Option<Self::Item>>
    {
        // Ensure `parent` is correctly set.
        self.ready_to_run_queue.waker.register(cx.waker());

        loop {
            // Safety: &mut self guarantees the mutual exclusion `dequeue`
            // expects
            let task = match unsafe { self.ready_to_run_queue.dequeue() } {
                Dequeue::Empty => {
                    if self.is_empty() {
                        // We can only consider ourselves terminated once we
                        // have yielded a `None`
                        self.len = TERMINATED_SENTINEL_LENGTH;
                        return Poll::Ready(None);
                    } else {
                        return Poll::Pending;
                    }
                }
                Dequeue::Inconsistent => {
                    // At this point, it may be worth yielding the thread &
                    // spinning a few times... but for now, just yield using the
                    // task system.
                    cx.waker().wake_by_ref();
                    return Poll::Pending;
                }
                Dequeue::Data(task) => task,
            };

            debug_assert!(task != self.ready_to_run_queue.stub());

            // Safety:
            // - `task` is a valid pointer.
            // - We are the only thread that accesses the `UnsafeCell` that
            //   contains the future
            let future = match unsafe { &mut *(*task).future.get() } {
                Some(future) => future,
                None => unreachable!("queued an empty task on ready_to_run"),
            };

            // Safety: `task` is a valid pointer
            let task = unsafe { self.unlink(task) };

            // Unset queued flag: This must be done before polling to ensure
            // that the future's task gets rescheduled if it sends a wake-up
            // notification **during** the call to `poll`.
            let prev = task.queued.swap(QUESTATE_POLLING, SeqCst);
            assert!(prev == QUESTATE_QUEUED);

            // We're going to need to be very careful if the `poll`
            // method below panics. We need to (a) not leak memory and
            // (b) ensure that we still don't have any use-after-frees. To
            // manage this we do a few things:
            //
            // * A "bomb" is created which if dropped abnormally will call
            //   `release_task`. That way we'll be sure the memory management
            //   of the `task` is managed correctly. In particular
            //   `release_task` will drop the future. This ensures that it is
            //   dropped on this thread and not accidentally on a different
            //   thread (bad).
            // * We unlink the task from our internal queue to preemptively
            //   assume it'll panic, in which case we'll want to discard it
            //   regardless.
            struct Bomb<'a, Fut> {
                queue: &'a mut FuturesUnordered<Fut>,
                task: Option<Arc<Task<Fut>>>,
            }

            impl<Fut> Drop for Bomb<'_, Fut> {
                fn drop(&mut self) {
                    if let Some(task) = self.task.take() {
                        self.queue.release_task(task, true);
                    }
                }
            }

            let mut bomb = Bomb {
                task: Some(task),
                queue: &mut *self,
            };

            // Poll the underlying future with the appropriate waker
            // implementation. This is where a large bit of the unsafety
            // starts to stem from internally. The waker is basically just
            // our `Arc<Task<Fut>>` and can schedule the future for polling by
            // enqueuing itself in the ready to run queue.
            //
            // Critically though `Task<Fut>` won't actually access `Fut`, the
            // future, while it's floating around inside of wakers.
            // These structs will basically just use `Fut` to size
            // the internal allocation, appropriately accessing fields and
            // deallocating the task if need be.
            {
                let waker = Task::waker_ref(bomb.task.as_ref().unwrap());
                let mut cx = Context::from_waker(&waker);
                // Safety: We won't move the future ever again
                let future = unsafe { Pin::new_unchecked(future) };

            loop {

                loop {
                    future.poll(&mut cx)
                }
            }

            match res {
                Poll::Pending => {
                    let task = bomb.task.take().unwrap();
                    bomb.queue.link(task);
                    continue
                }
                Poll::Ready(output) => {
                    return Poll::Ready(Some(output))
                }
            }
        }
    }
}

impl<Fut> Debug for FuturesUnordered<Fut> {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(fmt, "FuturesUnordered {{ ... }}")
    }
}

impl<Fut> Drop for FuturesUnordered<Fut> {
    fn drop(&mut self) {
        // When a `FuturesUnordered` is dropped we want to drop all futures
        // associated with it. At the same time though there may be tons of
        // wakers flying around which contain `Task<Fut>` references
        // inside them. We'll let those naturally get deallocated.
        unsafe {
            while !self.head_active.is_null() {
                let head = self.head_active;
                let task = self.unlink(head);
                self.release_task(task, false);
            }
            while let Some(task) = self.unlink_empty() {
                if task.queued.swap(false, SeqCst) {
                    // Release to the "ready to run" queue.
                    mem::forget(task);
                }
            }
        }

        // Note that at this point we could still have a bunch of tasks in the
        // ready to run queue. None of those tasks, however, have futures
        // associated with them so they're safe to destroy on any thread. At
        // this point the `FuturesUnordered` struct, the owner of the one strong
        // reference to the ready to run queue will drop the strong reference.
        // At that point whichever thread releases the strong refcount last (be
        // it this thread or some other thread as part of an `upgrade`) will
        // clear out the ready to run queue and free all remaining tasks.
        //
        // While that freeing operation isn't guaranteed to happen here, it's
        // guaranteed to happen "promptly" as no more "blocking work" will
        // happen while there's a strong refcount held.
    }
}

impl<Fut: Future> FromIterator<Fut> for FuturesUnordered<Fut> {
    fn from_iter<I>(iter: I) -> Self
    where
        I: IntoIterator<Item = Fut>,
    {
        let acc = FuturesUnordered::new();
        iter.into_iter().fold(acc, |mut acc, item| { acc.push(item); acc })
    }
}

impl<Fut: Future> FusedStream for FuturesUnordered<Fut> {
    fn is_terminated(&self) -> bool {
        self.len == TERMINATED_SENTINEL_LENGTH
    }
}
