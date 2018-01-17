use core::marker::PhantomData;

use {Poll, Async, StartSend, AsyncSink};
use sink::{Sink, SinkBase};
use stream::Stream;

/// Sink for the `Sink::with_flat_map` combinator, chaining a computation that returns an iterator
/// to run prior to pushing a value into the underlying sink
#[derive(Debug)]
#[must_use = "sinks do nothing unless polled"]
pub struct WithFlatMap<S, U, F, St>
where
    F: FnMut(U) -> St,
    St: Stream<Error=<S as SinkBase>::SinkError>,
    S: Sink<St::Item>,
{
    sink: S,
    f: F,
    stream: Option<St>,
    buffer: Option<St::Item>,
    _phantom: PhantomData<fn(U)>,
}

pub fn new<S, U, F, St>(sink: S, f: F) -> WithFlatMap<S, U, F, St>
where
    S: Sink<<St as Stream>::Item>,
    F: FnMut(U) -> St,
    St: Stream<Error=S::SinkError>,
{
    WithFlatMap {
        sink: sink,
        f: f,
        stream: None,
        buffer: None,
        _phantom: PhantomData,
    }
}

impl<S, U, F, St> WithFlatMap<S, U, F, St>
where
    S: Sink<<St as Stream>::Item>,
    F: FnMut(U) -> St,
    St: Stream<Error=S::SinkError>,
{
    /// Get a shared reference to the inner sink.
    pub fn get_ref(&self) -> &S {
        &self.sink
    }

    /// Get a mutable reference to the inner sink.
    pub fn get_mut(&mut self) -> &mut S {
        &mut self.sink
    }

    /// Consumes this combinator, returning the underlying sink.
    ///
    /// Note that this may discard intermediate state of this combinator, so
    /// care should be taken to avoid losing resources when this is called.
    pub fn into_inner(self) -> S {
        self.sink
    }

    fn try_empty_stream(&mut self) -> Poll<(), S::SinkError> {
        if let Some(x) = self.buffer.take() {
            if let AsyncSink::NotReady(x) = self.sink.start_send(x)? {
                self.buffer = Some(x);
                return Ok(Async::NotReady);
            }
        }
        if let Some(mut stream) = self.stream.take() {
            while let Some(x) = try_ready!(stream.poll()) {
                if let AsyncSink::NotReady(x) = self.sink.start_send(x)? {
                    self.stream = Some(stream);
                    self.buffer = Some(x);
                    return Ok(Async::NotReady);
                }
            }
        }
        Ok(Async::Ready(()))
    }
}

impl<S, U, F, St> Stream for WithFlatMap<S, U, F, St>
where
    S: Stream + Sink<<St as Stream>::Item>,
    F: FnMut(U) -> St,
    St: Stream<Error=S::SinkError>,
{
    type Item = S::Item;
    type Error = S::Error;
    fn poll(&mut self) -> Poll<Option<S::Item>, S::Error> {
        self.sink.poll()
    }
}

impl<S, U, F, St> Sink<U> for WithFlatMap<S, U, F, St>
where
    S: Sink<<St as Stream>::Item>,
    F: FnMut(U) -> St,
    St: Stream<Error=S::SinkError>,
{
    fn start_send(&mut self, i: U) -> StartSend<U, Self::SinkError> {
        if self.try_empty_stream()?.is_not_ready() {
            return Ok(AsyncSink::NotReady(i));
        }
        assert!(self.stream.is_none());
        self.stream = Some((self.f)(i));
        self.try_empty_stream()?;
        Ok(AsyncSink::Ready)
    }
}

impl<S, U, F, St> SinkBase for WithFlatMap<S, U, F, St>
where
    S: Sink<<St as Stream>::Item>,
    F: FnMut(U) -> St,
    St: Stream<Error=S::SinkError>,
{
    type SinkError = S::SinkError; 
    fn poll_complete(&mut self) -> Poll<(), Self::SinkError> {
        if self.try_empty_stream()?.is_not_ready() {
            return Ok(Async::NotReady);
        }
        self.sink.poll_complete()
    }
    fn close(&mut self) -> Poll<(), Self::SinkError> {
        if self.try_empty_stream()?.is_not_ready() {
            return Ok(Async::NotReady);
        }
        assert!(self.stream.is_none());
        self.sink.close()
    }
}
