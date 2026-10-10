//! Tokio/reqwest streaming driver shared by the native provider leaves.
//!
//! A leaf builds one [`BoundedRequest`] per model call, runs its media
//! resolution and HTTP send through [`BoundedRequest::guard`] /
//! [`BoundedRequest::send`], and hands the response body to
//! [`BoundedRequest::spawn`] together with its [`StreamProtocol`] framer and
//! [`StreamAssembler`]. The driver task reads the body, frames it, assembles
//! items, and forwards them through a bounded channel; dropping the returned
//! stream aborts the task.
//!
//! Every wait (`guard`, `send`, and each body read) races the host
//! cancellation signal and the whole-request deadline, mapping them to the
//! leaf's `cancelled` and `timeout` codes. Body transport failures map to the
//! leaf's `transport_error` code (or `timeout` when reqwest itself timed out);
//! framing and assembly failures go through
//! [`stream_norm_error`](crate::stream_norm_error).

use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use finstack_ai_kernel::ErrorCategory;
use finstack_ai_runtime::ports::model::{
    CancellationSignal, ModelError, ModelEventStream, ModelStreamItem,
};
use futures_util::{Stream, StreamExt};
use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::StreamNormError;
use crate::adapter_error::{
    ProviderErrorCodes, adapter_error, cancelled_error, http_error, stream_norm_error,
    timeout_error,
};
use crate::stream::{StreamAssembler, StreamProtocol};

/// Bounded capacity of the item channel between the body task and the
/// returned stream; the body is not read further while it is full.
pub const STREAM_CHANNEL_CAPACITY: usize = 32;

/// Map a reqwest failure to the leaf's stable adapter error.
///
/// A client-side timeout becomes the retryable `timeout` error under
/// [`ErrorCategory::Deadline`]; any other failure becomes the retryable
/// `transport_error` under [`ErrorCategory::Model`] with the message
/// `"{vendor} transport failed"`. The source is never rendered.
#[must_use]
pub fn transport_error(
    codes: &ProviderErrorCodes,
    vendor: &'static str,
    source: &reqwest::Error,
) -> ModelError {
    if source.is_timeout() {
        timeout_error(codes, vendor)
    } else {
        adapter_error(
            codes.transport_error,
            ErrorCategory::Model,
            true,
            format!("{vendor} transport failed"),
        )
    }
}

/// One in-flight provider request bounded by host cancellation and a
/// whole-request deadline.
///
/// The deadline is fixed at construction (`now + timeout`) so media
/// resolution, the HTTP exchange, and the streamed body share one budget.
#[derive(Debug, Clone)]
pub struct BoundedRequest {
    codes: ProviderErrorCodes,
    vendor: &'static str,
    cancellation: CancellationSignal,
    timeout: Duration,
    deadline: Instant,
}

impl BoundedRequest {
    /// Start the request clock for one model call.
    #[must_use]
    pub fn new(
        codes: ProviderErrorCodes,
        vendor: &'static str,
        cancellation: CancellationSignal,
        timeout: Duration,
    ) -> Self {
        Self {
            codes,
            vendor,
            cancellation,
            timeout,
            deadline: Instant::now() + timeout,
        }
    }

    /// Await `future` unless the host cancels or the deadline elapses first.
    ///
    /// # Errors
    ///
    /// Returns the leaf's `cancelled` error on cancellation and its
    /// retryable `timeout` error once the deadline passes.
    pub async fn guard<T>(&self, future: impl Future<Output = T>) -> Result<T, ModelError> {
        tokio::select! {
            () = self.cancellation.cancelled() => Err(cancelled_error(&self.codes, self.vendor)),
            () = tokio::time::sleep_until(self.deadline) => Err(timeout_error(&self.codes, self.vendor)),
            value = future => Ok(value),
        }
    }

    /// Send a request under the remaining budget and reject non-2xx statuses.
    ///
    /// The request timeout is set to the whole-request timeout; `endpoint`
    /// is the label rendered into the HTTP-status error.
    ///
    /// # Errors
    ///
    /// Returns the cancellation, deadline, or [`transport_error`] failure, or
    /// the leaf's `http_error` carrying only the status (see
    /// [`http_error`]).
    pub async fn send(
        &self,
        request: reqwest::RequestBuilder,
        endpoint: &'static str,
    ) -> Result<reqwest::Response, ModelError> {
        let response = self
            .guard(request.timeout(self.timeout).send())
            .await?
            .map_err(|source| transport_error(&self.codes, self.vendor, &source))?;
        if !response.status().is_success() {
            return Err(http_error(
                &self.codes,
                self.vendor,
                endpoint,
                response.status().as_u16(),
            ));
        }
        Ok(response)
    }

    /// Drive a response body on a detached task and return its item stream.
    ///
    /// Items flow through a channel of [`STREAM_CHANNEL_CAPACITY`]. The task
    /// stops after forwarding a [`ModelStreamItem::Completed`] item, once
    /// [`StreamAssembler::is_done`] turns true (then forwarding the
    /// assembler's terminal item), at end of body (forwarding the assembler's
    /// early-end failure), on the first error, when the host cancels, when
    /// the deadline elapses, or when the returned stream is dropped.
    #[must_use]
    pub fn spawn<B, P, A>(self, body: B, protocol: P, assembler: A) -> ModelEventStream
    where
        B: Stream<Item = reqwest::Result<Bytes>> + Send + 'static,
        P: StreamProtocol + Send + 'static,
        P::Frame: Send,
        A: StreamAssembler<Frame = P::Frame> + Send + 'static,
    {
        let (sender, receiver) = mpsc::channel(STREAM_CHANNEL_CAPACITY);
        let task = tokio::spawn(drive(self, body, protocol, assembler, sender));
        Box::pin(ReceiverModelStream { receiver, task })
    }
}

type ItemSender = mpsc::Sender<Result<ModelStreamItem, ModelError>>;

struct ReceiverModelStream {
    receiver: mpsc::Receiver<Result<ModelStreamItem, ModelError>>,
    task: tokio::task::JoinHandle<()>,
}

impl Stream for ReceiverModelStream {
    type Item = Result<ModelStreamItem, ModelError>;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.receiver.poll_recv(context)
    }
}

impl Drop for ReceiverModelStream {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Why forwarding stopped before the body was exhausted.
enum Stop {
    /// A `Completed` item was forwarded; nothing more may follow.
    Completed,
    /// The assembler reported its terminal frame; its `finish` item follows.
    Done,
    /// The consumer dropped the stream.
    Closed,
}

async fn drive<B, P, A>(
    bounds: BoundedRequest,
    body: B,
    mut protocol: P,
    mut assembler: A,
    sender: ItemSender,
) where
    B: Stream<Item = reqwest::Result<Bytes>>,
    P: StreamProtocol,
    A: StreamAssembler<Frame = P::Frame>,
{
    let mut body = core::pin::pin!(body);
    let norm = |error: &StreamNormError| stream_norm_error(&bounds.codes, bounds.vendor, error);
    let outcome = loop {
        let chunk = tokio::select! {
            () = bounds.cancellation.cancelled() => {
                let _ = sender.send(Err(cancelled_error(&bounds.codes, bounds.vendor))).await;
                return;
            }
            () = tokio::time::sleep_until(bounds.deadline) => {
                let _ = sender.send(Err(timeout_error(&bounds.codes, bounds.vendor))).await;
                return;
            }
            () = sender.closed() => return,
            chunk = body.next() => chunk,
        };
        let chunk = match chunk {
            None => {
                break match protocol.finish() {
                    Ok(trailing) => forward(&mut assembler, trailing, &sender).await,
                    Err(error) => Err(error),
                };
            }
            Some(Err(source)) => {
                let _ = sender
                    .send(Err(transport_error(&bounds.codes, bounds.vendor, &source)))
                    .await;
                return;
            }
            Some(Ok(chunk)) => chunk,
        };
        let frames = match protocol.push(&chunk) {
            Ok(frames) => frames,
            Err(error) => break Err(error),
        };
        match forward(&mut assembler, frames, &sender).await {
            Ok(None) => {}
            stopped => break stopped,
        }
    };
    let terminal = match outcome {
        Ok(Some(Stop::Completed | Stop::Closed)) => return,
        Ok(Some(Stop::Done) | None) => match assembler.finish() {
            Ok(Some(item)) => Ok(item),
            Ok(None) => return,
            Err(error) => Err(norm(&error)),
        },
        Err(error) => Err(norm(&error)),
    };
    let _ = sender.send(terminal).await;
}

/// Assemble and forward `frames`; `Ok(None)` means keep reading.
async fn forward<A: StreamAssembler>(
    assembler: &mut A,
    frames: Vec<A::Frame>,
    sender: &ItemSender,
) -> Result<Option<Stop>, StreamNormError> {
    for frame in frames {
        let items = assembler.consume(&frame)?;
        let mut completed = false;
        for item in items {
            completed |= matches!(item, ModelStreamItem::Completed(_));
            if sender.send(Ok(item)).await.is_err() {
                return Ok(Some(Stop::Closed));
            }
        }
        if completed {
            return Ok(Some(Stop::Completed));
        }
        if assembler.is_done() {
            return Ok(Some(Stop::Done));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use finstack_ai_kernel::{ContentBlock, ProviderIds, Usage};
    use finstack_ai_runtime::ports::model::{ModelResponse, TextDelta};
    use futures_util::stream;

    use super::*;
    use crate::ndjson::NdjsonParser;
    use crate::ollama_chat::OllamaChatAssembly;

    const CODES: ProviderErrorCodes = ProviderErrorCodes {
        config_invalid: "test_config_invalid",
        request_invalid: "test_request_invalid",
        http_error: "test_http_error",
        transport_error: "test_transport_error",
        timeout: "test_timeout",
        cancelled: "test_cancelled",
        stream_invalid: "test_stream_invalid",
        stream_limit_exceeded: "test_stream_limit_exceeded",
        response_invalid: "test_response_invalid",
    };

    /// Line protocol: `text` lines become deltas, `end` completes.
    struct LineAssembler;

    impl StreamAssembler for LineAssembler {
        type Frame = String;

        fn consume(&mut self, frame: &String) -> Result<Vec<ModelStreamItem>, StreamNormError> {
            match frame.as_str() {
                "end" => Ok(vec![ModelStreamItem::Completed(ModelResponse {
                    assistant_content: Arc::from([]),
                    tool_calls: Arc::from([]),
                    usage: Usage::empty(),
                    provider_ids: ProviderIds::empty(),
                    completion_id: Arc::from("request-1"),
                    continuation_state: None,
                })]),
                "bad" => Err(StreamNormError::response("bad line")),
                text => Ok(vec![ModelStreamItem::TextDelta(TextDelta {
                    text: Arc::from(text),
                })]),
            }
        }

        fn finish(self) -> Result<Option<ModelStreamItem>, StreamNormError> {
            Err(StreamNormError::stream("line stream ended before end"))
        }
    }

    fn request_bounds(timeout: Duration) -> (BoundedRequest, CancellationSignal) {
        let cancellation = CancellationSignal::new();
        (
            BoundedRequest::new(CODES, "Test", cancellation.clone(), timeout),
            cancellation,
        )
    }

    fn chunks(parts: &[&str]) -> impl Stream<Item = reqwest::Result<Bytes>> + Send + 'static {
        stream::iter(
            parts
                .iter()
                .map(|part| Ok(Bytes::copy_from_slice(part.as_bytes())))
                .collect::<Vec<_>>(),
        )
    }

    async fn collect(mut stream: ModelEventStream) -> Vec<Result<ModelStreamItem, ModelError>> {
        let mut items = Vec::new();
        while let Some(item) = stream.next().await {
            items.push(item);
        }
        items
    }

    #[tokio::test]
    async fn items_flow_and_completion_stops_reading() {
        let (bounds, _) = request_bounds(Duration::from_secs(5));
        let body = chunks(&["hel", "lo\nwor", "ld\nend\nignored\n"]);
        let items = collect(bounds.spawn(body, NdjsonParser::new(64, 1_024), LineAssembler)).await;
        assert_eq!(items.len(), 3);
        assert!(
            matches!(&items[0], Ok(ModelStreamItem::TextDelta(delta)) if delta.text.as_ref() == "hello")
        );
        assert!(
            matches!(&items[1], Ok(ModelStreamItem::TextDelta(delta)) if delta.text.as_ref() == "world")
        );
        assert!(matches!(&items[2], Ok(ModelStreamItem::Completed(_))));
    }

    #[tokio::test]
    async fn trailing_frame_and_early_end_are_reported() {
        let (bounds, _) = request_bounds(Duration::from_secs(5));
        let body = chunks(&["hello\nend"]);
        let items = collect(bounds.spawn(body, NdjsonParser::new(64, 1_024), LineAssembler)).await;
        assert_eq!(items.len(), 2);
        assert!(matches!(&items[1], Ok(ModelStreamItem::Completed(_))));

        let (bounds, _) = request_bounds(Duration::from_secs(5));
        let body = chunks(&["hello\n"]);
        let items = collect(bounds.spawn(body, NdjsonParser::new(64, 1_024), LineAssembler)).await;
        let Err(error) = &items[1] else {
            panic!("early end must fail");
        };
        assert_eq!(error.code().as_str(), "test_stream_invalid");
        assert_eq!(error.category(), ErrorCategory::Model);
        assert_eq!(error.message(), "line stream ended before end");
    }

    #[tokio::test]
    async fn framing_and_assembly_failures_use_the_leaf_codes() {
        let (bounds, _) = request_bounds(Duration::from_secs(5));
        let body = chunks(&["0123456789\n"]);
        let items = collect(bounds.spawn(body, NdjsonParser::new(4, 1_024), LineAssembler)).await;
        let Err(error) = &items[0] else {
            panic!("limit must fail");
        };
        assert_eq!(error.code().as_str(), "test_stream_limit_exceeded");
        assert_eq!(error.category(), ErrorCategory::Limit);
        assert_eq!(
            error.message(),
            "Test response exceeded a configured stream limit"
        );

        let (bounds, _) = request_bounds(Duration::from_secs(5));
        let body = chunks(&["ok\nbad\nend\n"]);
        let items = collect(bounds.spawn(body, NdjsonParser::new(64, 1_024), LineAssembler)).await;
        assert_eq!(items.len(), 2);
        let Err(error) = &items[1] else {
            panic!("bad line must fail");
        };
        assert_eq!(error.code().as_str(), "test_response_invalid");
    }

    #[tokio::test]
    async fn done_flag_forwards_the_assembler_terminal_item() {
        let (bounds, _) = request_bounds(Duration::from_secs(5));
        let body = chunks(&[
            "{\"message\":{\"content\":\"hi\"},\"done\":false}\n",
            "{\"message\":{\"content\":\"\"},\"done\":true,\"prompt_eval_count\":1,\"eval_count\":1}\n{\"ignored\":true}\n",
        ]);
        let assembly = OllamaChatAssembly::new("request-1".to_owned(), false, None);
        let items = collect(bounds.spawn(body, NdjsonParser::new(4_096, 65_536), assembly)).await;
        assert_eq!(items.len(), 3);
        assert!(matches!(&items[0], Ok(ModelStreamItem::TextDelta(_))));
        assert!(matches!(&items[1], Ok(ModelStreamItem::Usage(_))));
        let Ok(ModelStreamItem::Completed(response)) = &items[2] else {
            panic!("done must complete");
        };
        assert!(matches!(
            response.assistant_content[0],
            ContentBlock::Text(_)
        ));
    }

    #[tokio::test]
    async fn cancellation_ends_the_stream_with_the_cancelled_code() {
        let (bounds, cancellation) = request_bounds(Duration::from_secs(5));
        let body = chunks(&["hello\n"]).chain(stream::pending());
        let mut stream = bounds.spawn(body, NdjsonParser::new(64, 1_024), LineAssembler);
        assert!(matches!(
            stream.next().await,
            Some(Ok(ModelStreamItem::TextDelta(_)))
        ));
        cancellation.cancel();
        let Some(Err(error)) = stream.next().await else {
            panic!("cancellation must fail the stream");
        };
        assert_eq!(error.code().as_str(), "test_cancelled");
        assert_eq!(error.category(), ErrorCategory::Cancellation);
        assert!(!error.retryable());
        assert!(stream.next().await.is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn deadline_ends_the_stream_with_the_timeout_code() {
        let (bounds, _) = request_bounds(Duration::from_millis(50));
        let body = chunks(&["hello\n"]).chain(stream::pending());
        let mut stream = bounds.spawn(body, NdjsonParser::new(64, 1_024), LineAssembler);
        assert!(matches!(
            stream.next().await,
            Some(Ok(ModelStreamItem::TextDelta(_)))
        ));
        let Some(Err(error)) = stream.next().await else {
            panic!("deadline must fail the stream");
        };
        assert_eq!(error.code().as_str(), "test_timeout");
        assert_eq!(error.category(), ErrorCategory::Deadline);
        assert!(error.retryable());
        assert_eq!(error.message(), "Test request timed out");
        assert!(stream.next().await.is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn guard_and_send_honour_cancellation_and_deadline() {
        let (bounds, cancellation) = request_bounds(Duration::from_secs(1));
        cancellation.cancel();
        let error = bounds
            .guard(std::future::pending::<()>())
            .await
            .expect_err("cancelled");
        assert_eq!(error.code().as_str(), "test_cancelled");

        let (bounds, _) = request_bounds(Duration::from_millis(10));
        let error = bounds
            .guard(std::future::pending::<()>())
            .await
            .expect_err("deadline");
        assert_eq!(error.code().as_str(), "test_timeout");
        assert_eq!(error.category(), ErrorCategory::Deadline);

        let (bounds, _) = request_bounds(Duration::from_secs(1));
        assert_eq!(bounds.guard(async { 7 }).await.expect("ready"), 7);
    }

    #[tokio::test]
    async fn transport_failures_map_to_the_transport_code() {
        let source = reqwest::Client::new()
            .get("http://127.0.0.1:9/")
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .expect_err("connection refused");
        assert!(!source.is_timeout());
        let error = transport_error(&CODES, "Test", &source);
        assert_eq!(error.code().as_str(), "test_transport_error");
        assert_eq!(error.category(), ErrorCategory::Model);
        assert!(error.retryable());
        assert_eq!(error.message(), "Test transport failed");

        let (bounds, _) = request_bounds(Duration::from_secs(5));
        let body = chunks(&["hello\n"]).chain(stream::iter([Err(source)]));
        let items = collect(bounds.spawn(body, NdjsonParser::new(64, 1_024), LineAssembler)).await;
        assert_eq!(items.len(), 2);
        let Err(error) = &items[1] else {
            panic!("mid-stream transport failure must fail");
        };
        assert_eq!(error.code().as_str(), "test_transport_error");
    }

    #[tokio::test]
    async fn send_maps_connection_failures_to_the_transport_code() {
        let (bounds, _) = request_bounds(Duration::from_secs(5));
        let request = reqwest::Client::new()
            .get("http://127.0.0.1:9/")
            .timeout(Duration::from_secs(5));
        let error = bounds
            .send(request, "test endpoint")
            .await
            .expect_err("connection refused");
        assert_eq!(error.code().as_str(), "test_transport_error");
    }

    /// Body stream that counts polls and records its own drop.
    struct CountingBody {
        remaining: usize,
        pulls: Arc<AtomicUsize>,
        dropped: Arc<AtomicBool>,
    }

    impl Stream for CountingBody {
        type Item = reqwest::Result<Bytes>;

        fn poll_next(
            mut self: Pin<&mut Self>,
            _context: &mut Context<'_>,
        ) -> Poll<Option<Self::Item>> {
            if self.remaining == 0 {
                return Poll::Ready(None);
            }
            self.remaining -= 1;
            self.pulls.fetch_add(1, Ordering::SeqCst);
            Poll::Ready(Some(Ok(Bytes::from_static(b"x\n"))))
        }
    }

    impl Drop for CountingBody {
        fn drop(&mut self) {
            self.dropped.store(true, Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn bounded_channel_applies_backpressure_and_drop_aborts_the_task() {
        let pulls = Arc::new(AtomicUsize::new(0));
        let dropped = Arc::new(AtomicBool::new(false));
        let (bounds, _) = request_bounds(Duration::from_secs(5));
        let body = CountingBody {
            remaining: 10 * STREAM_CHANNEL_CAPACITY,
            pulls: Arc::clone(&pulls),
            dropped: Arc::clone(&dropped),
        };
        let mut stream = bounds.spawn(body, NdjsonParser::new(64, 1_024 * 1_024), LineAssembler);
        for _ in 0..16 {
            tokio::task::yield_now().await;
        }
        let parked = pulls.load(Ordering::SeqCst);
        assert!(
            parked <= STREAM_CHANNEL_CAPACITY + 1,
            "pulled {parked} chunks unpolled"
        );

        assert!(matches!(
            stream.next().await,
            Some(Ok(ModelStreamItem::TextDelta(_)))
        ));
        for _ in 0..16 {
            tokio::task::yield_now().await;
        }
        assert!(pulls.load(Ordering::SeqCst) > parked);
        assert!(!dropped.load(Ordering::SeqCst));

        drop(stream);
        for _ in 0..16 {
            tokio::task::yield_now().await;
        }
        assert!(dropped.load(Ordering::SeqCst), "abort must drop the body");
        assert!(pulls.load(Ordering::SeqCst) < 10 * STREAM_CHANNEL_CAPACITY);
    }
}
