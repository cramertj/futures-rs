use core::mem::PinMut;

use {PinMutExt, OptionExt};

use futures_core::{Future, Poll, Stream};
use futures_core::task;

/// Creates a `Stream` from a seed and a closure returning a `Future`.
///
/// This function is the dual for the `Stream::fold()` adapter: while
/// `Stream::fold()` reduces a `Stream` to one single value, `unfold()` creates a
/// `Stream` from a seed value.
///
/// `unfold()` will call the provided closure with the provided seed, then wait
/// for the returned `Future` to complete with `(a, b)`. It will then yield the
/// value `a`, and use `b` as the next internal state.
///
/// If the closure returns `None` instead of `Some(Future)`, then the `unfold()`
/// will stop producing items and return `Ok(Async::Ready(None))` in future
/// calls to `poll()`.
///
/// In case of error generated by the returned `Future`, the error will be
/// returned by the `Stream`.  The `Stream` will then yield
/// `Ok(Async::Ready(None))` in future calls to `poll()`.
///
/// This function can typically be used when wanting to go from the "world of
/// futures" to the "world of streams": the provided closure can build a
/// `Future` using other library functions working on futures, and `unfold()`
/// will turn it into a `Stream` by repeating the operation.
///
/// # Example
///
/// ```rust
/// # extern crate futures;
/// # extern crate futures_executor;
///
/// use futures::prelude::*;
/// use futures::stream;
/// use futures::future;
/// use futures_executor::block_on;
///
/// # fn main() {
/// let mut stream = stream::unfold(0, |state| {
///     if state <= 2 {
///         let next_state = state + 1;
///         let yielded = state  * 2;
///         let fut = future::ok::<_, u32>((yielded, next_state));
///         Some(fut)
///     } else {
///         None
///     }
/// });
///
/// let result = block_on(stream.collect());
/// assert_eq!(result, Ok(vec![0, 2, 4]));
/// # }
/// ```
pub fn unfold<'any, F, State, It>(init: State, f: F) -> Unfold<'any, F, State, It>
    where F: UnfoldFn<State, It> + 'any,
          State: 'any,
{
    Unfold {
        f: f,
        state: init,
        fut: None,
    }
}

pub trait UnfoldFnLt<'a, State, It> {
    type Future: Future<Output = Option<It>>;
    fn apply(&mut self, state: &'a mut State) -> Self::Future;
}

pub trait UnfoldFn<State, It>: for<'a> UnfoldFnLt<'a, State, It> {}
impl<State, It, T: for<'a> UnfoldFnLt<'a, State, It>> UnfoldFn<State, It> for T {}

/// A stream which creates futures, polls them and return their result
///
/// This stream is returned by the `futures::stream::unfold` method
#[allow(missing_debug_implementations)]
#[must_use = "streams do nothing unless polled"]
pub struct Unfold<'any, F, State, It>
    where F: UnfoldFn<State, It> + 'any,
          State: 'any,
{
    f: F,
    fut: Option<<F as UnfoldFnLt<'any, State, It>>::Future>,
    // The state, which may be borrowed by the other fields, must be dropped last
    state: State,
}

// TODO: make `Unfold` *always* !Unpin-- where do we get that marker type?

impl<'any, F, State, It> Unfold<'any, F, State, It>
    where F: UnfoldFn<State, It> + 'any,
          State: 'any,
{
    unsafe_unpinned!(f -> F);
    unsafe_pinned!(fut -> Option<<F as UnfoldFnLt<'any, State, It>>::Future>);
}

unsafe fn transmute_lt<'input, 'output, T>(x: &'input mut T) -> &'output mut T {
    ::std::mem::transmute(x)
}

impl<'any, F, State, It> Stream for Unfold<'any, F, State, It>
    where F: UnfoldFn<State, It> + 'any,
          State: 'any,
{
    type Item = It;

    fn poll_next(mut self: PinMut<Self>, cx: &mut task::Context) -> Poll<Option<It>> {
        if self.fut.is_none() {
            let state_ref_mut: &'any mut State = {
                unsafe { transmute_lt(&mut PinMut::get_mut(self.reborrow()).state) }
            };
            let fut = (self.f()).apply(state_ref_mut);
            self.fut().assign(Some(fut));
        }

        let next = try_ready!(self.fut().as_pin_mut().unwrap().poll(cx));
        self.fut().assign(None);
        Poll::Ready(next)
    }
}
