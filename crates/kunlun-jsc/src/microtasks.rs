//! Explicit checkpoints and plain-data rejection diagnostics.
use super::*;
use crate::{PromiseRejection, PromiseRejectionTransition};

impl JscVm {
    /// Drains this context's FIFO Promise jobs, including nested jobs, then
    /// snapshots rejection transitions. Ordinary binding calls do not drain
    /// the pinned engine. System JSC returns `Unsupported`.
    ///
    /// Calls from running JS or a rejection conversion return `InvalidState`.
    /// Returns true if callbacks queued work requiring another checkpoint.
    /// Records retrieved with `take_promise_rejections` contain owned text only.
    pub fn microtask_checkpoint(&self) -> Result<bool, JscError> {
        let mut pending = 0;
        let mut sink = RejectionSink {
            vm: self,
            records: Vec::new(),
        };
        // SAFETY: the sink is live on this stack for the synchronous call. The
        // trampoline copies borrowed values and catches all Rust panics.
        let status = unsafe {
            sys::kunlun_jsc_microtask_checkpoint(
                self.context.as_context(),
                Some(collect_rejection),
                (&mut sink as *mut RejectionSink<'_>).cast(),
                &mut pending,
            )
        };
        self.rejections.borrow_mut().extend(sink.records);
        expect_status("microtask_checkpoint", status)?;
        Ok(pending != 0)
    }

    /// Takes accumulated transitions as owned data, without running JS.
    pub fn take_promise_rejections(&self) -> Vec<PromiseRejection> {
        std::mem::take(&mut *self.rejections.borrow_mut())
    }

    /// Stable identity for diagnostics, independent of pointer reuse or names.
    pub fn isolate_id(&self) -> u64 {
        self.context.isolate_id
    }
}

struct RejectionSink<'a> {
    vm: &'a JscVm,
    records: Vec<PromiseRejection>,
}

unsafe extern "C" fn collect_rejection(
    data: *mut std::ffi::c_void,
    id: u64,
    transition: u32,
    reason: ValueRef,
    source: *const sys::kunlun_jsc_string,
) -> sys::kunlun_jsc_status {
    let result = catch_callback_panic(|| {
        // SAFETY: checkpoint passes its live stack sink exclusively to this
        // callback; native reentry protection prevents recursive delivery.
        let sink = unsafe { &mut *data.cast::<RejectionSink<'_>>() };
        let transition = match transition {
            0 => PromiseRejectionTransition::Unhandled,
            1 => PromiseRejectionTransition::Handled,
            _ => return sys::KUNLUN_JSC_STATUS_INVALID_ARGUMENT,
        };
        // SAFETY: source is borrowed until callback return. Copy UTF-8 through
        // the same checked ABI used for owned strings, without releasing it.
        let source_url = match unsafe { borrowed_string(source) } {
            Ok(value) => value,
            Err(_) => return sys::KUNLUN_JSC_STATUS_CALLBACK_ERROR,
        };
        let exception = match transition {
            PromiseRejectionTransition::Unhandled => {
                let exception = sink
                    .vm
                    .module_error("promise_rejection", &source_url, reason);
                sink.vm
                    .reported_rejections
                    .borrow_mut()
                    .insert(id, exception.clone());
                exception
            }
            PromiseRejectionTransition::Handled => {
                let Some(exception) = sink.vm.reported_rejections.borrow_mut().remove(&id) else {
                    return sys::KUNLUN_JSC_STATUS_INVALID_STATE;
                };
                exception
            }
        };
        sink.records.push(PromiseRejection {
            isolate_id: sink.vm.isolate_id(),
            rejection_id: id,
            transition,
            exception,
        });
        sys::KUNLUN_JSC_STATUS_OK
    });
    result.unwrap_or(sys::KUNLUN_JSC_STATUS_CALLBACK_ERROR)
}

unsafe fn borrowed_string(source: *const sys::kunlun_jsc_string) -> Result<String, JscError> {
    let mut capacity = 0;
    // SAFETY: engine lends a live string during rejection delivery.
    expect_status("rejection_source", unsafe {
        sys::kunlun_jsc_string_get_max_utf8_size(source, &mut capacity)
    })?;
    let capacity = usize::try_from(capacity)
        .map_err(|_| JscError::invalid_input("rejection_source", "source URL exceeds usize"))?;
    let mut bytes = vec![0; capacity];
    let mut written = 0;
    // SAFETY: bytes has the advertised capacity and outputs are writable.
    expect_status("rejection_source", unsafe {
        sys::kunlun_jsc_string_write_utf8(source, bytes.as_mut_ptr(), capacity as u64, &mut written)
    })?;
    bytes.truncate(written.saturating_sub(1) as usize);
    String::from_utf8(bytes)
        .map_err(|_| JscError::invalid_input("rejection_source", "source URL is not UTF-8"))
}
