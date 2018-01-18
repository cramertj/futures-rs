use sink::{Sink, SinkBase};

use {Poll, StartSend, Stream};

/// Sink for the `Sink::sink_map_err` combinator.
#[derive(Debug)]
#[must_use = "sinks do nothing unless polled"]
pub struct SinkMapErr<S, F> {
    sink: S,
    f: Option<F>,
}

pub fn new<S, F>(s: S, f: F) -> SinkMapErr<S, F> {
    SinkMapErr { sink: s, f: Some(f) }
}

impl<S, E> SinkMapErr<S, E> {
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
}

impl<S, SinkItem, F, E> Sink<SinkItem> for SinkMapErr<S, F>
    where S: Sink<SinkItem>,
          F: FnOnce(S::SinkError) -> E,
{
    fn start_send(&mut self, item: SinkItem) -> StartSend<SinkItem, Self::SinkError> {
        self.sink.start_send(item).map_err(|e| self.f.take().expect("cannot use MapErr after an error")(e))
    }
}

impl<S, F, E> SinkBase for SinkMapErr<S, F>
    where S: SinkBase,
          F: FnOnce(S::SinkError) -> E,
{
    type SinkError = E;

    fn poll_complete(&mut self) -> Poll<(), Self::SinkError> {
        self.sink.poll_complete().map_err(|e| self.f.take().expect("cannot use MapErr after an error")(e))
    }

    fn close(&mut self) -> Poll<(), Self::SinkError> {
        self.sink.close().map_err(|e| self.f.take().expect("cannot use MapErr after an error")(e))
    }
}

impl<S: Stream, F> Stream for SinkMapErr<S, F> {
    type Item = S::Item;
    type Error = S::Error;

    fn poll(&mut self) -> Poll<Option<S::Item>, S::Error> {
        self.sink.poll()
    }
}
