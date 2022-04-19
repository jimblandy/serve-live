use futures_util::Stream;
use std::marker::Unpin;
use std::pin::Pin;
use std::task::{Context, Poll};

/// A wrapper for a stream that drops some data when the stream ends or is dropped.
pub struct StreamOwns<S, T> {
    stream: S,
    owned: Option<T>,
}

pub fn own<S, T>(stream: S, owned: T) -> StreamOwns<S, T>
where
    S: Stream + Unpin,
{
    StreamOwns {
        stream,
        owned: Some(owned),
    }
}

impl<S, T> Unpin for StreamOwns<S, T> {}

impl<S, T> Stream for StreamOwns<S, T>
where
    S: Stream + Unpin,
{
    type Item = S::Item;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match Pin::new(&mut self.stream).poll_next(cx) {
            Poll::Ready(None) => {
                self.owned = None;
                Poll::Ready(None)
            }
            other => other,
        }
    }
}
