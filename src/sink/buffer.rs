use std::collections::VecDeque;

use {Poll, Async};
use {StartSend, AsyncSink};
use sink::{Sink, SinkBase};
use stream::Stream;

/// Sink for the `Sink::buffer` combinator, which buffers up to some fixed
/// number of values when the underlying sink is unable to accept them.
#[derive(Debug)]
#[must_use = "sinks do nothing unless polled"]
pub struct Buffer<S, SinkItem>
    where S: Sink<SinkItem>
{
    sink: S,
    buf: VecDeque<SinkItem>,

    // Track capacity separately from the `VecDeque`, which may be rounded up
    cap: usize,
}

pub fn new<S, SinkItem>(sink: S, amt: usize) -> Buffer<S, SinkItem>
    where S: Sink<SinkItem>
{
    Buffer {
        sink: sink,
        buf: VecDeque::with_capacity(amt),
        cap: amt,
    }
}

impl<S, SinkItem> Buffer<S, SinkItem>
    where S: Sink<SinkItem>
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

    fn try_empty_buffer(&mut self) -> Poll<(), S::SinkError> {
        while let Some(item) = self.buf.pop_front() {
            if let AsyncSink::NotReady(item) = self.sink.start_send(item)? {
                self.buf.push_front(item);

                // ensure that we attempt to complete any pushes we've started
                self.sink.poll_complete()?;

                return Ok(Async::NotReady);
            }
        }

        Ok(Async::Ready(()))
    }
}

// Forwarding impl of Stream from the underlying sink
impl<S, SinkItem> Stream for Buffer<S, SinkItem>
    where S: Sink<SinkItem> + Stream
{
    type Item = S::Item;
    type Error = S::Error;

    fn poll(&mut self) -> Poll<Option<S::Item>, S::Error> {
        self.sink.poll()
    }
}

impl<S, SinkItem> Sink<SinkItem> for Buffer<S, SinkItem>
    where S: Sink<SinkItem>
{
    fn start_send(&mut self, item: SinkItem) -> StartSend<SinkItem, Self::SinkError> {
        if self.cap == 0 {
            return self.sink.start_send(item);
        }

        self.try_empty_buffer()?;
        if self.buf.len() == self.cap {
            return Ok(AsyncSink::NotReady(item));
        }
        self.buf.push_back(item);
        Ok(AsyncSink::Ready)
    }
}

impl<S, SinkItem> SinkBase for Buffer<S, SinkItem>
    where S: Sink<SinkItem>
{
    type SinkError = S::SinkError;

    fn poll_complete(&mut self) -> Poll<(), Self::SinkError> {
        if self.cap == 0 {
            return self.sink.poll_complete();
        }

        try_ready!(self.try_empty_buffer());
        debug_assert!(self.buf.is_empty());
        self.sink.poll_complete()
    }

    fn close(&mut self) -> Poll<(), Self::SinkError> {
        if self.cap == 0 {
            return self.sink.close();
        }

        if self.buf.len() > 0 {
            try_ready!(self.try_empty_buffer());
        }
        assert_eq!(self.buf.len(), 0);
        self.sink.close()
    }
}
