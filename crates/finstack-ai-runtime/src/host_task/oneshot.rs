use std::future::Future;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use crate::host_driver::Signal;

pub(super) fn oneshot<T>() -> (OneshotSender<T>, OneshotReceiver<T>) {
    let inner = Arc::new(OneshotInner {
        value: Mutex::new(None),
        signal: Signal::new(),
    });
    (
        OneshotSender {
            inner: Arc::clone(&inner),
        },
        OneshotReceiver { inner },
    )
}

pub(super) struct OneshotInner<T> {
    value: Mutex<Option<T>>,
    signal: Signal,
}

pub(super) struct OneshotSender<T> {
    inner: Arc<OneshotInner<T>>,
}

impl<T> OneshotSender<T> {
    pub(super) fn send(self, value: T) {
        if let Ok(mut slot) = self.inner.value.lock() {
            *slot = Some(value);
        }
        self.inner.signal.notify_waiters();
    }
}

pub(super) struct OneshotReceiver<T> {
    inner: Arc<OneshotInner<T>>,
}

impl<T> Future for OneshotReceiver<T> {
    type Output = T;

    fn poll(self: std::pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if let Ok(mut value) = self.inner.value.lock()
            && let Some(value) = value.take()
        {
            return Poll::Ready(value);
        }
        let wait = self.inner.signal.notified();
        let mut wait = std::pin::pin!(wait);
        match wait.as_mut().poll(cx) {
            Poll::Ready(()) => {
                if let Ok(mut value) = self.inner.value.lock()
                    && let Some(value) = value.take()
                {
                    Poll::Ready(value)
                } else {
                    cx.waker().wake_by_ref();
                    Poll::Pending
                }
            }
            Poll::Pending => Poll::Pending,
        }
    }
}
