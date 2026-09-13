//! Engine watchdog, cooperative cancellation, and copied heap telemetry.
use super::*;
use crate::{JscError, TerminationReason};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeapStatistics {
    pub heap_size: u64,
    pub heap_capacity: u64,
    pub extra_memory_size: u64,
}

impl HeapStatistics {
    pub const fn accounted_bytes(self) -> u64 {
        self.heap_size.saturating_add(self.extra_memory_size)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourcePolicy {
    pub execution_timeout: Option<Duration>,
    pub watchdog_interval: Duration,
    pub soft_heap_limit: Option<u64>,
    pub hard_heap_limit: Option<u64>,
}

impl ResourcePolicy {
    fn validate(self) -> Result<Self, JscError> {
        if self.watchdog_interval.is_zero() || !self.watchdog_interval.as_secs_f64().is_finite() {
            return Err(JscError::invalid_input(
                "resource_policy",
                "watchdog interval must be finite and greater than zero",
            ));
        }
        if self
            .execution_timeout
            .is_some_and(|timeout| timeout.is_zero())
        {
            return Err(JscError::invalid_input(
                "resource_policy",
                "execution timeout must be greater than zero",
            ));
        }
        if self.soft_heap_limit == Some(0) || self.hard_heap_limit == Some(0) {
            return Err(JscError::invalid_input(
                "resource_policy",
                "heap limits must be greater than zero",
            ));
        }
        if let (Some(soft), Some(hard)) = (self.soft_heap_limit, self.hard_heap_limit)
            && soft > hard
        {
            return Err(JscError::invalid_input(
                "resource_policy",
                "soft heap limit cannot exceed hard heap limit",
            ));
        }
        Ok(self)
    }
}

#[derive(Debug, Clone)]
struct TerminalState {
    reason: TerminationReason,
    detail: String,
}

#[derive(Debug, Default)]
struct State {
    policy: Option<ResourcePolicy>,
    active_depth: u32,
    deadline: Option<Instant>,
    cancellation_requested: bool,
    terminal: Option<TerminalState>,
}

pub(super) struct ResourceState {
    state: Mutex<State>,
}

impl ResourceState {
    pub(super) fn new() -> Self {
        Self {
            state: Mutex::new(State::default()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn configure(&self, policy: ResourcePolicy) {
        self.lock().policy = Some(policy);
    }

    fn ensure_configurable(&self) -> Result<(), JscError> {
        let state = self.lock();
        if state.active_depth != 0 {
            return Err(JscError::invalid_input(
                "resource_policy",
                "resource policy cannot change during an active execution",
            ));
        }
        if let Some(terminal) = &state.terminal {
            return Err(terminal_error("resource_policy", terminal));
        }
        Ok(())
    }

    fn begin(self: &Arc<Self>, operation: &'static str) -> Result<ExecutionScope, JscError> {
        let mut state = self.lock();
        if let Some(terminal) = &state.terminal {
            return Err(terminal_error(operation, terminal));
        }
        if state.active_depth == 0 {
            state.deadline = match state.policy.and_then(|policy| policy.execution_timeout) {
                Some(timeout) => Some(Instant::now().checked_add(timeout).ok_or_else(|| {
                    JscError::invalid_input(
                        "resource_policy",
                        "execution timeout exceeds the monotonic clock range",
                    )
                })?),
                None => None,
            };
        }
        state.active_depth = state.active_depth.checked_add(1).ok_or_else(|| {
            JscError::invalid_input("execution_scope", "execution nesting overflowed")
        })?;
        drop(state);
        Ok(ExecutionScope {
            state: Arc::clone(self),
            active: true,
        })
    }

    fn finish(&self) {
        let mut state = self.lock();
        debug_assert!(state.active_depth > 0);
        state.active_depth = state.active_depth.saturating_sub(1);
        if state.active_depth == 0 {
            state.deadline = None;
            if state.terminal.is_none() {
                state.cancellation_requested = false;
            }
        }
    }

    fn terminal_error(&self, operation: &'static str) -> Option<JscError> {
        self.lock()
            .terminal
            .as_ref()
            .map(|terminal| terminal_error(operation, terminal))
    }

    fn cancel(&self) {
        let mut state = self.lock();
        if state.active_depth > 0 {
            state.cancellation_requested = true;
            set_terminal(
                &mut state,
                TerminationReason::Cancelled,
                "execution was cancelled by the host".to_owned(),
            );
        }
    }

    fn mark_out_of_memory(&self, operation: &'static str) -> JscError {
        let mut state = self.lock();
        set_terminal(
            &mut state,
            TerminationReason::OutOfMemory,
            "JavaScriptCore reported an out-of-memory failure; discard this isolate".to_owned(),
        );
        terminal_error(
            operation,
            state.terminal.as_ref().expect("terminal state was set"),
        )
    }

    fn check(&self, statistics: Option<HeapStatistics>) -> bool {
        let mut state = self.lock();
        if state.terminal.is_some() {
            return true;
        }
        if state.cancellation_requested {
            set_terminal(
                &mut state,
                TerminationReason::Cancelled,
                "execution was cancelled by the host".to_owned(),
            );
            return true;
        }
        if state
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            set_terminal(
                &mut state,
                TerminationReason::DeadlineExceeded,
                "execution exceeded its monotonic deadline".to_owned(),
            );
            return true;
        }
        if let (Some(policy), Some(statistics)) = (state.policy, statistics)
            && policy
                .hard_heap_limit
                .is_some_and(|limit| statistics.accounted_bytes() > limit)
        {
            let limit = policy.hard_heap_limit.expect("hard limit was checked");
            set_terminal(
                &mut state,
                TerminationReason::MemoryLimitExceeded,
                format!(
                    "heap policy limit {limit} bytes exceeded by {} accounted bytes; discard this isolate",
                    statistics.accounted_bytes()
                ),
            );
            return true;
        }
        false
    }

    fn policy(&self) -> Option<ResourcePolicy> {
        self.lock().policy
    }

    fn poll_interval(&self) -> Option<Duration> {
        self.lock().policy.map(|policy| policy.watchdog_interval)
    }

    fn deadline_remaining(&self) -> Option<Duration> {
        self.lock()
            .deadline
            .map(|deadline| deadline.saturating_duration_since(Instant::now()))
    }
}

fn set_terminal(state: &mut State, reason: TerminationReason, detail: String) {
    if state.terminal.is_none() {
        state.terminal = Some(TerminalState { reason, detail });
    }
}

fn terminal_error(operation: &'static str, terminal: &TerminalState) -> JscError {
    JscError::terminated(operation, terminal.reason, terminal.detail.clone())
}

/// Thread-safe cancellation authority containing no JSC handle.
#[derive(Clone)]
pub struct ExecutionHandle {
    state: Arc<ResourceState>,
}

impl ExecutionHandle {
    pub fn cancel(&self) {
        self.state.cancel();
    }
}

/// RAII execution boundary. Nested JSC calls share one monotonic deadline.
pub struct ExecutionScope {
    state: Arc<ResourceState>,
    active: bool,
}

impl Drop for ExecutionScope {
    fn drop(&mut self) {
        if self.active {
            self.state.finish();
        }
    }
}

impl ContextInner {
    pub(super) fn execution_scope(
        &self,
        operation: &'static str,
    ) -> Result<ExecutionScope, JscError> {
        self._group.resources.begin(operation)
    }

    pub(super) fn resource_error(&self, operation: &'static str) -> Option<JscError> {
        self._group.resources.terminal_error(operation)
    }

    pub(super) fn status_error(
        &self,
        operation: &'static str,
        status: sys::kunlun_jsc_status,
    ) -> JscError {
        if status == sys::KUNLUN_JSC_STATUS_OUT_OF_MEMORY {
            self._group.resources.mark_out_of_memory(operation)
        } else if let Some(error) = self.resource_error(operation) {
            error
        } else {
            JscError::native(operation, status)
        }
    }
}

impl JscVm {
    pub fn set_resource_policy(&self, policy: ResourcePolicy) -> Result<(), JscError> {
        let policy = policy.validate()?;
        let resources = &self.context._group.resources;
        resources.ensure_configurable()?;
        // SAFETY: configuration is isolate-thread-affine. The Arc allocation is
        // stable until after the owning native group clears its watchdog.
        let status = unsafe {
            sys::kunlun_jsc_context_group_set_watchdog(
                self.context._group.handle.as_ptr(),
                policy.watchdog_interval.as_secs_f64(),
                Some(watchdog_callback),
                Arc::as_ptr(resources).cast_mut().cast(),
            )
        };
        expect_status("context_group_set_watchdog", status)?;
        resources.configure(policy);
        Ok(())
    }

    pub fn execution_handle(&self) -> ExecutionHandle {
        ExecutionHandle {
            state: Arc::clone(&self.context._group.resources),
        }
    }

    pub fn execution_scope(&self) -> Result<ExecutionScope, JscError> {
        self.context.execution_scope("execution_scope")
    }

    pub fn heap_statistics(&self) -> Result<HeapStatistics, JscError> {
        let mut raw = sys::kunlun_jsc_heap_statistics {
            heap_size: 0,
            heap_capacity: 0,
            extra_memory_size: 0,
        };
        // SAFETY: output storage and the isolate-thread-affine context are live.
        let status =
            unsafe { sys::kunlun_jsc_context_heap_statistics(self.context.as_context(), &mut raw) };
        expect_status("context_heap_statistics", status)?;
        Ok(HeapStatistics {
            heap_size: raw.heap_size,
            heap_capacity: raw.heap_capacity,
            extra_memory_size: raw.extra_memory_size,
        })
    }

    pub fn enforce_resource_policy(&self) -> Result<Option<HeapStatistics>, JscError> {
        if let Some(error) = self.context.resource_error("resource_policy") {
            return Err(error);
        }
        let Some(policy) = self.context._group.resources.policy() else {
            return Ok(None);
        };
        if !Self::backend_info().supports_heap_telemetry {
            if self.context._group.resources.check(None) {
                return Err(self
                    .context
                    .resource_error("resource_policy")
                    .expect("resource check set a terminal state"));
            }
            return Ok(None);
        }
        let mut statistics = self.heap_statistics()?;
        if policy
            .soft_heap_limit
            .is_some_and(|limit| statistics.accounted_bytes() > limit)
        {
            // SAFETY: collection runs synchronously on the isolate thread.
            let status =
                unsafe { sys::kunlun_jsc_context_collect_garbage(self.context.as_context()) };
            if status != sys::KUNLUN_JSC_STATUS_OK {
                return Err(self.context.status_error("collect_garbage", status));
            }
            statistics = self.heap_statistics()?;
        }
        if self.context._group.resources.check(Some(statistics)) {
            return Err(self
                .context
                .resource_error("resource_policy")
                .expect("resource check set a terminal state"));
        }
        Ok(Some(statistics))
    }

    pub fn resource_poll_interval(&self) -> Option<Duration> {
        self.context._group.resources.poll_interval()
    }

    pub fn deadline_remaining(&self) -> Option<Duration> {
        self.context._group.resources.deadline_remaining()
    }
}

unsafe extern "C" fn watchdog_callback(
    data: *mut std::ffi::c_void,
    statistics: *const sys::kunlun_jsc_heap_statistics,
) -> u32 {
    catch_callback_panic(|| {
        if data.is_null() {
            return 1;
        }
        // SAFETY: the context group owns this Arc allocation until after the
        // native watchdog is cleared. The callback receives borrowed stats.
        let state = unsafe { &*data.cast::<ResourceState>() };
        let statistics = if statistics.is_null() || !JscVm::backend_info().supports_heap_telemetry {
            None
        } else {
            // SAFETY: native lends a fully initialized snapshot for this call.
            let raw = unsafe { &*statistics };
            Some(HeapStatistics {
                heap_size: raw.heap_size,
                heap_capacity: raw.heap_capacity,
                extra_memory_size: raw.extra_memory_size,
            })
        };
        u32::from(state.check(statistics))
    })
    .unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::JscStatus;

    #[test]
    fn rejects_inverted_heap_limits() {
        let vm = JscVm::new("resource-policy-validation").unwrap();
        let error = vm
            .set_resource_policy(ResourcePolicy {
                execution_timeout: None,
                watchdog_interval: Duration::from_millis(10),
                soft_heap_limit: Some(2),
                hard_heap_limit: Some(1),
            })
            .unwrap_err();
        assert_eq!(error.kind(), crate::JscErrorKind::InvalidInput);
    }

    #[test]
    fn cancellation_handle_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ExecutionHandle>();
    }

    #[test]
    fn first_terminal_reason_wins() {
        let state = Arc::new(ResourceState::new());
        state.configure(ResourcePolicy {
            execution_timeout: Some(Duration::from_secs(60)),
            watchdog_interval: Duration::from_millis(10),
            soft_heap_limit: None,
            hard_heap_limit: Some(1),
        });
        let _scope = state.begin("test").unwrap();
        state.cancel();
        assert!(state.check(Some(HeapStatistics {
            heap_size: 2,
            heap_capacity: 2,
            extra_memory_size: 0,
        })));
        assert_eq!(
            state.terminal_error("test").unwrap().termination_reason(),
            Some(TerminationReason::Cancelled)
        );
    }

    #[test]
    fn out_of_memory_is_typed_and_terminal() {
        let state = ResourceState::new();
        let error = state.mark_out_of_memory("allocate");
        assert_eq!(
            error.termination_reason(),
            Some(TerminationReason::OutOfMemory)
        );
        assert_eq!(error.kind(), crate::JscErrorKind::ExecutionTerminated);
        assert_eq!(JscStatus::OutOfMemory.as_raw(), 2);
    }

    #[test]
    fn watchdog_terminates_an_infinite_loop_at_the_monotonic_deadline() {
        let vm = JscVm::new("deadline-watchdog").unwrap();
        vm.set_resource_policy(ResourcePolicy {
            execution_timeout: Some(Duration::from_millis(25)),
            watchdog_interval: Duration::from_millis(2),
            soft_heap_limit: None,
            hard_heap_limit: None,
        })
        .unwrap();

        let started = Instant::now();
        let error = vm
            .evaluate("for (;;) {}", "test:///deadline.js")
            .unwrap_err();
        assert_eq!(
            error.termination_reason(),
            Some(TerminationReason::DeadlineExceeded)
        );
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(
            vm.evaluate("'not-run'", "test:///after-deadline.js")
                .unwrap_err()
                .termination_reason(),
            Some(TerminationReason::DeadlineExceeded)
        );
    }

    #[test]
    fn foreign_thread_cancellation_only_touches_synchronized_host_state() {
        let vm = JscVm::new("cooperative-cancellation").unwrap();
        vm.set_resource_policy(ResourcePolicy {
            execution_timeout: Some(Duration::from_secs(5)),
            watchdog_interval: Duration::from_millis(2),
            soft_heap_limit: None,
            hard_heap_limit: None,
        })
        .unwrap();
        let (started_tx, started_rx) = std::sync::mpsc::sync_channel(0);
        let started = vm
            .host_function("started", move |_| {
                started_tx
                    .send(())
                    .map_err(|_| "cancellation observer stopped".to_owned())?;
                Ok(CallbackReturn::Undefined)
            })
            .unwrap();
        started.set_global("started").unwrap();
        let handle = vm.execution_handle();
        let canceller = std::thread::spawn(move || {
            started_rx.recv().unwrap();
            handle.cancel();
        });

        let error = vm
            .evaluate("started(); for (;;) {}", "test:///cancel.js")
            .unwrap_err();
        canceller.join().unwrap();
        assert_eq!(
            error.termination_reason(),
            Some(TerminationReason::Cancelled)
        );
    }

    #[test]
    fn callback_reentry_keeps_the_outer_monotonic_deadline() {
        let owner = std::rc::Rc::new(std::cell::RefCell::new(std::rc::Weak::<JscVm>::new()));
        let vm = JscVm::new("callback-deadline").unwrap();
        vm.set_resource_policy(ResourcePolicy {
            execution_timeout: Some(Duration::from_millis(25)),
            watchdog_interval: Duration::from_millis(2),
            soft_heap_limit: None,
            hard_heap_limit: None,
        })
        .unwrap();
        let vm = std::rc::Rc::new(vm);
        *owner.borrow_mut() = std::rc::Rc::downgrade(&vm);
        let reentrant_owner = std::rc::Rc::clone(&owner);
        let reenter = vm
            .host_function("reenter", move |_| {
                std::thread::sleep(Duration::from_millis(30));
                let owner = reentrant_owner
                    .borrow()
                    .upgrade()
                    .ok_or_else(|| "callback owner was dropped".to_owned())?;
                let error = owner
                    .evaluate("for (;;) {}", "test:///nested-deadline.js")
                    .unwrap_err();
                assert_eq!(
                    error.termination_reason(),
                    Some(TerminationReason::DeadlineExceeded)
                );
                Err("nested evaluation reached its deadline".to_owned())
            })
            .unwrap();
        reenter.set_global("reenter").unwrap();

        let error = vm
            .evaluate("reenter()", "test:///callback-deadline.js")
            .unwrap_err();
        assert_eq!(
            error.termination_reason(),
            Some(TerminationReason::DeadlineExceeded)
        );
    }

    #[cfg(feature = "bundled-jsc")]
    #[test]
    fn heap_telemetry_is_copied_and_hard_limit_is_terminal() {
        let vm = JscVm::new("heap-policy").unwrap();
        vm.evaluate(
            "globalThis.retained = Array.from({ length: 131072 }, (_, value) => ({ value }));",
            "test:///heap-policy.js",
        )
        .unwrap();
        vm.collect_garbage().unwrap();
        let statistics = vm.heap_statistics().unwrap();
        assert!(statistics.heap_capacity >= statistics.heap_size);
        assert!(statistics.accounted_bytes() > 1);
        let limit = statistics.accounted_bytes() / 2;
        vm.set_resource_policy(ResourcePolicy {
            execution_timeout: Some(Duration::from_secs(1)),
            watchdog_interval: Duration::from_millis(2),
            soft_heap_limit: Some(limit),
            hard_heap_limit: Some(limit),
        })
        .unwrap();
        let _scope = vm.execution_scope().unwrap();
        let error = vm.enforce_resource_policy().unwrap_err();
        assert_eq!(
            error.termination_reason(),
            Some(TerminationReason::MemoryLimitExceeded)
        );
    }
}
