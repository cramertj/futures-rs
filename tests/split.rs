extern crate futures;

use futures::prelude::*;
use futures::stream::iter_ok;

struct Join<T, U>(T, U);

impl<T: Stream, U> Stream for Join<T, U> {
    type Item = T::Item;
    type Error = T::Error;

    fn poll(&mut self) -> Poll<Option<T::Item>, T::Error> {
        self.0.poll()
    }
}

impl<T, U, SinkItem> Sink<SinkItem> for Join<T, U>
    where U: Sink<SinkItem>
{
    fn start_send(&mut self, item: SinkItem)
        -> StartSend<SinkItem, U::SinkError>
    {
        self.1.start_send(item)
    }
}

impl<T, U: SinkBase> SinkBase for Join<T, U> {
    type SinkError = U::SinkError;

    fn poll_complete(&mut self) -> Poll<(), U::SinkError> {
        self.1.poll_complete()
    }

    fn close(&mut self) -> Poll<(), U::SinkError> {
        self.1.close()
    }
}

#[test]
fn test_split() {
    let mut dest = Vec::new();
    {
        let j = Join(iter_ok(vec![10, 20, 30]), &mut dest);
        let (sink, stream) = j.split();
        let j = sink.reunite(stream).expect("test_split: reunite error");
        let (sink, stream) = j.split();
        sink.send_all(stream).wait().unwrap();
    }
    assert_eq!(dest, vec![10, 20, 30]);
}
