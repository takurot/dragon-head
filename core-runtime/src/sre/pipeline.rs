use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};

use crossbeam_channel::{bounded, Receiver, RecvTimeoutError, Sender, TryRecvError, TrySendError};
use thiserror::Error;

use super::{FastSemanticState, FullSemanticState, SemanticState};

const SRE_QUEUE_POLL_INTERVAL: Duration = Duration::from_millis(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AsyncPipelineConfig {
    pub render_queue_capacity: usize,
    pub sre_queue_capacity: usize,
    pub audit_queue_capacity: usize,
    pub full_stage_delay: Duration,
    pub audit_stage_delay: Duration,
}

impl Default for AsyncPipelineConfig {
    fn default() -> Self {
        Self {
            render_queue_capacity: 64,
            sre_queue_capacity: 64,
            audit_queue_capacity: 128,
            full_stage_delay: Duration::ZERO,
            audit_stage_delay: Duration::ZERO,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueKind {
    Render,
    SreFast,
    SreFull,
    AuditPolicy,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PipelineSubmitError {
    #[error("Backpressure applied on {queue:?} queue")]
    Backpressure { queue: QueueKind },
    #[error("Pipeline queue {queue:?} is closed")]
    Closed { queue: QueueKind },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SreStage {
    Fast,
    Full,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PipelineReceiveError {
    #[error("Timed out while waiting for {stage:?} state")]
    Timeout { stage: SreStage },
    #[error("Pipeline worker closed before delivering {stage:?} state")]
    Closed { stage: SreStage },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AsyncPipelineMetrics {
    pub render_submitted: usize,
    pub render_processed: usize,
    pub render_backpressure: usize,
    pub fast_completed: usize,
    pub full_completed: usize,
    pub audit_submitted: usize,
    pub audit_processed: usize,
    pub audit_backpressure: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditEvent {
    pub kind: AuditEventKind,
    pub name: String,
}

impl AuditEvent {
    pub fn tool_call(name: impl Into<String>) -> Self {
        Self {
            kind: AuditEventKind::ToolCall,
            name: name.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditEventKind {
    ToolCall,
    PolicyDecision,
    StateSnapshot,
}

#[derive(Debug)]
pub struct LayeredStateHandle {
    fast_rx: Receiver<FastSemanticState>,
    full_rx: Receiver<FullSemanticState>,
}

impl LayeredStateHandle {
    pub fn recv_fast(&self, timeout: Duration) -> Result<FastSemanticState, PipelineReceiveError> {
        recv_stage(&self.fast_rx, timeout, SreStage::Fast)
    }

    pub fn recv_full(&self, timeout: Duration) -> Result<FullSemanticState, PipelineReceiveError> {
        recv_stage(&self.full_rx, timeout, SreStage::Full)
    }
}

pub struct AsyncPipeline {
    render_tx: Sender<RenderTask>,
    audit_tx: Sender<AuditEvent>,
    metrics: Arc<PipelineMetricsInner>,
}

impl AsyncPipeline {
    pub fn new(config: AsyncPipelineConfig) -> Self {
        let (render_tx, render_rx) = bounded(capacity_or_default(config.render_queue_capacity));
        let (sre_fast_tx, sre_fast_rx) = bounded(capacity_or_default(config.sre_queue_capacity));
        let (sre_full_tx, sre_full_rx) = bounded(capacity_or_default(config.sre_queue_capacity));
        let (audit_tx, audit_rx) = bounded(capacity_or_default(config.audit_queue_capacity));

        let metrics = Arc::new(PipelineMetricsInner::default());

        spawn_render_worker(render_rx, sre_fast_tx, sre_full_tx, Arc::clone(&metrics));
        spawn_sre_worker(
            sre_fast_rx,
            sre_full_rx,
            Arc::clone(&metrics),
            config.full_stage_delay,
        );
        spawn_audit_worker(audit_rx, Arc::clone(&metrics), config.audit_stage_delay);

        Self {
            render_tx,
            audit_tx,
            metrics,
        }
    }

    pub fn submit_state(
        &self,
        state: SemanticState,
    ) -> Result<LayeredStateHandle, PipelineSubmitError> {
        self.metrics
            .render_submitted
            .fetch_add(1, Ordering::Relaxed);

        let (fast_tx, fast_rx) = bounded(1);
        let (full_tx, full_rx) = bounded(1);
        let task = RenderTask {
            state,
            fast_tx,
            full_tx,
        };

        match self.render_tx.try_send(task) {
            Ok(()) => Ok(LayeredStateHandle { fast_rx, full_rx }),
            Err(TrySendError::Full(_task)) => {
                self.metrics
                    .render_backpressure
                    .fetch_add(1, Ordering::Relaxed);
                Err(PipelineSubmitError::Backpressure {
                    queue: QueueKind::Render,
                })
            }
            Err(TrySendError::Disconnected(_task)) => Err(PipelineSubmitError::Closed {
                queue: QueueKind::Render,
            }),
        }
    }

    pub fn submit_audit_event(&self, event: AuditEvent) -> Result<(), PipelineSubmitError> {
        self.metrics.audit_submitted.fetch_add(1, Ordering::Relaxed);

        match self.audit_tx.try_send(event) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_event)) => {
                self.metrics
                    .audit_backpressure
                    .fetch_add(1, Ordering::Relaxed);
                Err(PipelineSubmitError::Backpressure {
                    queue: QueueKind::AuditPolicy,
                })
            }
            Err(TrySendError::Disconnected(_event)) => Err(PipelineSubmitError::Closed {
                queue: QueueKind::AuditPolicy,
            }),
        }
    }

    pub fn metrics(&self) -> AsyncPipelineMetrics {
        self.metrics.snapshot()
    }
}

#[derive(Default)]
struct PipelineMetricsInner {
    render_submitted: AtomicUsize,
    render_processed: AtomicUsize,
    render_backpressure: AtomicUsize,
    fast_completed: AtomicUsize,
    full_completed: AtomicUsize,
    audit_submitted: AtomicUsize,
    audit_processed: AtomicUsize,
    audit_backpressure: AtomicUsize,
}

impl PipelineMetricsInner {
    fn snapshot(&self) -> AsyncPipelineMetrics {
        AsyncPipelineMetrics {
            render_submitted: self.render_submitted.load(Ordering::Relaxed),
            render_processed: self.render_processed.load(Ordering::Relaxed),
            render_backpressure: self.render_backpressure.load(Ordering::Relaxed),
            fast_completed: self.fast_completed.load(Ordering::Relaxed),
            full_completed: self.full_completed.load(Ordering::Relaxed),
            audit_submitted: self.audit_submitted.load(Ordering::Relaxed),
            audit_processed: self.audit_processed.load(Ordering::Relaxed),
            audit_backpressure: self.audit_backpressure.load(Ordering::Relaxed),
        }
    }
}

struct RenderTask {
    state: SemanticState,
    fast_tx: Sender<FastSemanticState>,
    full_tx: Sender<FullSemanticState>,
}

enum SreTask {
    Fast {
        state: Arc<SemanticState>,
        tx: Sender<FastSemanticState>,
    },
    Full {
        state: Arc<SemanticState>,
        tx: Sender<FullSemanticState>,
    },
}

fn spawn_render_worker(
    render_rx: Receiver<RenderTask>,
    sre_fast_tx: Sender<SreTask>,
    sre_full_tx: Sender<SreTask>,
    metrics: Arc<PipelineMetricsInner>,
) {
    thread::spawn(move || {
        while let Ok(task) = render_rx.recv() {
            metrics.render_processed.fetch_add(1, Ordering::Relaxed);
            let shared_state = Arc::new(task.state);

            if sre_fast_tx
                .send(SreTask::Fast {
                    state: Arc::clone(&shared_state),
                    tx: task.fast_tx,
                })
                .is_err()
            {
                break;
            }

            if sre_full_tx
                .send(SreTask::Full {
                    state: shared_state,
                    tx: task.full_tx,
                })
                .is_err()
            {
                break;
            }
        }
    });
}

fn spawn_sre_worker(
    sre_fast_rx: Receiver<SreTask>,
    sre_full_rx: Receiver<SreTask>,
    metrics: Arc<PipelineMetricsInner>,
    full_stage_delay: Duration,
) {
    thread::spawn(move || {
        let mut fast_open = true;
        let mut full_open = true;

        while fast_open || full_open {
            if fast_open {
                match sre_fast_rx.try_recv() {
                    Ok(task) => {
                        process_sre_task(task, &metrics, full_stage_delay);
                        continue;
                    }
                    Err(TryRecvError::Empty) => {}
                    Err(TryRecvError::Disconnected) => {
                        fast_open = false;
                    }
                }
            }

            if full_open {
                match sre_full_rx.recv_timeout(SRE_QUEUE_POLL_INTERVAL) {
                    Ok(task) => {
                        process_sre_task(task, &metrics, full_stage_delay);
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => {
                        full_open = false;
                    }
                }
                continue;
            }

            match sre_fast_rx.recv_timeout(SRE_QUEUE_POLL_INTERVAL) {
                Ok(task) => process_sre_task(task, &metrics, full_stage_delay),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    fast_open = false;
                }
            }
        }
    });
}

fn process_sre_task(task: SreTask, metrics: &PipelineMetricsInner, full_stage_delay: Duration) {
    match task {
        SreTask::Fast { state, tx } => {
            let _ = tx.send(state.generate_fast_state());
            metrics.fast_completed.fetch_add(1, Ordering::Relaxed);
        }
        SreTask::Full { state, tx } => {
            if !full_stage_delay.is_zero() {
                thread::sleep(full_stage_delay);
            }
            let _ = tx.send(state.generate_full_state());
            metrics.full_completed.fetch_add(1, Ordering::Relaxed);
        }
    }
}

fn spawn_audit_worker(
    audit_rx: Receiver<AuditEvent>,
    metrics: Arc<PipelineMetricsInner>,
    audit_stage_delay: Duration,
) {
    thread::spawn(move || {
        while audit_rx.recv().is_ok() {
            if !audit_stage_delay.is_zero() {
                thread::sleep(audit_stage_delay);
            }
            metrics.audit_processed.fetch_add(1, Ordering::Relaxed);
        }
    });
}

fn recv_stage<T>(
    rx: &Receiver<T>,
    timeout: Duration,
    stage: SreStage,
) -> Result<T, PipelineReceiveError> {
    match rx.recv_timeout(timeout) {
        Ok(value) => Ok(value),
        Err(RecvTimeoutError::Timeout) => Err(PipelineReceiveError::Timeout { stage }),
        Err(RecvTimeoutError::Disconnected) => Err(PipelineReceiveError::Closed { stage }),
    }
}

fn capacity_or_default(capacity: usize) -> usize {
    if capacity == 0 {
        1
    } else {
        capacity
    }
}
