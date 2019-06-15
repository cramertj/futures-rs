#![feature(async_await, test)]

extern crate test;
use crate::test::Bencher;

use futures::channel::{mpsc, oneshot};
use futures::executor::block_on;
use futures::future;
use futures::stream::{StreamExt, FuturesUnordered};
use futures::task::Poll;
use std::collections::VecDeque;
use std::thread;

#[bench]
fn oneshots(b: &mut Bencher) {
    const NUM: usize = 10_000;

    b.iter(|| {
        let mut txs = VecDeque::with_capacity(NUM);
        let mut rxs = FuturesUnordered::new();

        for _ in 0..NUM {
            let (tx, rx) = oneshot::channel();
            txs.push_back(tx);
            rxs.push(rx);
        }

        thread::spawn(move || {
            while let Some(tx) = txs.pop_front() {
                let _ = tx.send("hello");
            }
        });

        block_on(future::poll_fn(move |cx| {
            loop {
                if let Poll::Ready(None) = rxs.poll_next_unpin(cx) {
                    break
                }
            }
            Poll::Ready(())
        }))
    });
}

#[bench]
fn repeated_single(b: &mut Bencher) {
    const NUM: usize = 10_000;
    b.iter(|| {
        let (tx, mut rx) = mpsc::unbounded();
        let mut rxs = FuturesUnordered::new();
        for _ in 0..NUM {
            let _ = tx.unbounded_send("hello");
        }
        block_on(async move {
            for _ in 0..NUM {
                rxs.push(rx.into_future());
                rx = rxs.next().await.unwrap().1;
            }
        });
    });
}
