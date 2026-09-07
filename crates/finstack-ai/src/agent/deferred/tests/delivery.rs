use super::*;
use crate::{ChildEventContext, ChildEventSink};
use finstack_ai_runtime::events::EventBatch;

#[derive(Default)]
struct Sink {
    started: Arc<AtomicUsize>,
    active: Arc<AtomicUsize>,
    panic_on_create: bool,
}

struct Active(Arc<AtomicUsize>);
impl Drop for Active {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl ChildEventSink for Sink {
    fn on_batch(&self, _: &ChildEventContext, _: &EventBatch) -> PortFuture<Result<(), ()>> {
        assert!(!self.panic_on_create, "test sink construction panic");
        let active = Arc::clone(&self.active);
        let started = Arc::clone(&self.started);
        Box::pin(async move {
            started.fetch_add(1, Ordering::SeqCst);
            active.fetch_add(1, Ordering::SeqCst);
            let _guard = Active(active);
            std::future::pending().await
        })
    }
}

async fn bridge(sink: Arc<Sink>) -> (ChildRunBridge, AgentRun, AgentRun) {
    let (parent, store) = deferred_tool_parent().await;
    let child = cancellable_child(Arc::clone(&store)).await;
    let request = isolated_child_request(store, &parent).await;
    let bridge = ChildRunBridge::new(
        vec![Arc::new(CountingPlanner {
            hits: AtomicUsize::new(0),
            claim: Some(request),
        })],
        Arc::new(RecordingInvoker),
        Arc::new(FixedResolver {
            child: child.clone(),
        }),
    )
    .with_event_sink(sink);
    (bridge, parent, child)
}

#[tokio::test]
async fn completed_bridge_drops_its_single_stalled_callback() {
    let sink = Arc::new(Sink::default());
    let weak = Arc::downgrade(&sink);
    let (bridge, parent, child) = bridge(Arc::clone(&sink)).await;
    child.cancel().await.expect("cancel child");
    assert_eq!(
        bridge.recover(&parent).await.expect("settle"),
        vec![ChildSettleOutcome::Failed]
    );
    assert_eq!(sink.started.load(Ordering::SeqCst), 1);
    assert_eq!(sink.active.load(Ordering::SeqCst), 0);
    drop(bridge);
    drop(sink);
    assert!(
        weak.upgrade().is_none(),
        "no detached callback retains the sink"
    );
}

#[tokio::test]
async fn dropping_settlement_drops_its_pending_callback() {
    let sink = Arc::new(Sink::default());
    let (bridge, parent, child) = bridge(Arc::clone(&sink)).await;
    let mut settle = Box::pin(bridge.recover(&parent));
    std::future::poll_fn(|cx| {
        assert!(settle.as_mut().poll(cx).is_pending());
        if sink.active.load(Ordering::SeqCst) == 1 {
            std::task::Poll::Ready(())
        } else {
            std::task::Poll::Pending
        }
    })
    .await;
    drop(settle);
    assert_eq!(sink.active.load(Ordering::SeqCst), 0);
    child.cancel().await.expect("cancel child");
    parent.close_events();
    parent.cancel().await.expect("cancel parent");
}

#[tokio::test]
async fn sink_construction_panics_do_not_fail_parent_settlement() {
    let sink = Arc::new(Sink {
        panic_on_create: true,
        ..Sink::default()
    });
    let (bridge, parent, child) = bridge(sink).await;
    child.cancel().await.expect("cancel child");
    assert_eq!(
        bridge.recover(&parent).await.expect("settle"),
        vec![ChildSettleOutcome::Failed]
    );
}
