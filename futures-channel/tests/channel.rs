extern crate futures;
extern crate futures_channel;
extern crate futures_executor;

use std::sync::atomic::*;
use std::thread;

use futures::prelude::*;
use futures::future::result;
use futures::task;
use futures_executor::current_thread::run;
use futures_channel::mpsc;

#[test]
fn sequence() {
    let (tx, rx) = mpsc::channel(1);

    let amt = 20;
    let t = thread::spawn(move || {
        run(|c| c.block_on(send(amt, tx)))
    });
    let mut list = run(|c| c.block_on(rx.collect())).unwrap().into_iter();
    for i in (1..amt + 1).rev() {
        assert_eq!(list.next(), Some(i));
    }
    assert_eq!(list.next(), None);

    t.join().unwrap().unwrap();

    fn send(n: u32, sender: mpsc::Sender<u32>)
            -> Box<Future<Item=(), Error=()> + Send> {
        if n == 0 {
            return Box::new(result(Ok(())))
        }
        Box::new(sender.send(n).map_err(|_| ()).and_then(move |sender| {
            send(n - 1, sender)
        }))
    }
}

#[test]
fn drop_sender() {
    let (tx, mut rx) = mpsc::channel::<u32>(1);
    drop(tx);
    match rx.poll(&mut task::Context) {
        Ok(Async::Ready(None)) => {}
        _ => panic!("channel should be done"),
    }
}

#[test]
fn drop_rx() {
    let (tx, rx) = mpsc::channel::<u32>(1);
    let tx = run(|c| c.block_on(tx.send(1))).unwrap();
    drop(rx);
    assert!(run(|c| c.block_on(tx.send(1))).is_err());
}

#[test]
fn drop_order() {
    static DROPS: AtomicUsize = ATOMIC_USIZE_INIT;
    let (tx, rx) = mpsc::channel(1);

    struct A;

    impl Drop for A {
        fn drop(&mut self) {
            DROPS.fetch_add(1, Ordering::SeqCst);
        }
    }

    let tx = run(|c| c.block_on(tx.send(A))).unwrap();
    assert_eq!(DROPS.load(Ordering::SeqCst), 0);
    drop(rx);
    assert_eq!(DROPS.load(Ordering::SeqCst), 1);
    assert!(run(|c| c.block_on(tx.send(A))).is_err());
    assert_eq!(DROPS.load(Ordering::SeqCst), 2);
}