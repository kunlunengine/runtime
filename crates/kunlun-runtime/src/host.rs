use base64::Engine;
use cap_std::ambient_authority;
use cap_std::fs::{Dir, OpenOptions, OpenOptionsExt};
use kunlun_jsc::{DeferredPromise, HostCall, JscError, JscVm};
use reqwest::redirect::Policy;
use serde::{Deserialize, Serialize};
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::future::Future;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::mpsc::{self, Receiver, Sender};
use tokio::sync::{Mutex, Notify, oneshot};
use tokio::task::AbortHandle;

pub(crate) const MAX_HTTP_RESPONSE_BYTES: usize = 1024 * 1024;
pub(crate) const MAX_IN_FLIGHT_HOST_CALLS: usize = 256;
pub(crate) const STREAM_CHUNK_BYTES: usize = 64 * 1024;
pub(crate) const STREAM_BUFFER_CHUNKS: usize = 2;

#[derive(Debug, Clone, Default)]
pub struct HostPermissions {
    read_roots: Vec<ReadRoot>,
    read_bindings: BTreeMap<String, ReadRoot>,
    net_hosts: HashSet<String>,
    fetch_hosts: HashSet<String>,
    active: Option<Arc<AtomicBool>>,
    request_active: Option<Arc<AtomicBool>>,
}

#[derive(Debug, Clone)]
struct ReadRoot {
    path: PathBuf,
    directory: Arc<Dir>,
}

struct AuthorizedRead {
    display_path: PathBuf,
    relative_path: PathBuf,
    directory: Arc<Dir>,
}

impl HostPermissions {
    pub fn none() -> Self {
        Self::default()
    }

    pub fn allow_read_root(mut self, root: impl AsRef<Path>) -> io::Result<Self> {
        let root = root.as_ref();
        let directory = Dir::open_ambient_dir(root, ambient_authority())?;
        self.read_roots.push(ReadRoot {
            path: std::path::absolute(root)?,
            directory: Arc::new(directory),
        });
        Ok(self)
    }

    /// Bind a named M3 filesystem capability to an opened deployment directory.
    /// The manifest can name this binding but cannot choose its host path.
    pub fn bind_read_root(
        mut self,
        name: impl Into<String>,
        root: impl AsRef<Path>,
    ) -> io::Result<Self> {
        let name = name.into();
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid binding name",
            ));
        }
        if self.read_bindings.contains_key(&name) {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "filesystem binding already exists",
            ));
        }
        let root = std::fs::canonicalize(root)?;
        let directory = Dir::open_ambient_dir(&root, ambient_authority())?;
        self.read_bindings.insert(
            name,
            ReadRoot {
                path: root,
                directory: Arc::new(directory),
            },
        );
        Ok(self)
    }

    pub fn allow_net_host(mut self, host: impl Into<String>) -> Self {
        self.net_hosts.insert(host.into().to_ascii_lowercase());
        self
    }

    /// Grant application Fetch access to one exact host for direct trusted
    /// execution. Artifact admission derives Fetch grants from its own scope.
    pub fn allow_fetch_host(mut self, host: impl Into<String>) -> Self {
        self.fetch_hosts.insert(host.into().to_ascii_lowercase());
        self
    }

    pub(crate) fn supports_capability(name: &str) -> bool {
        matches!(name, "http.host" | "fs.binding")
    }

    pub(crate) fn has_capability(&self, capability: &crate::Capability) -> bool {
        match capability.name.as_str() {
            "http.host" => self.net_hosts.contains(&capability.resource),
            "fs.binding" => self.read_bindings.contains_key(&capability.resource),
            _ => false,
        }
    }

    /// Construct the exact manifest/deployment intersection. Legacy M2 read roots
    /// are deliberately excluded from admitted application permissions.
    pub(crate) fn scoped_to(
        &self,
        declarations: &crate::CapabilityRequirements,
        active: Arc<AtomicBool>,
    ) -> (Self, BTreeSet<crate::Capability>) {
        let mut scoped = Self {
            active: Some(active),
            ..Self::none()
        };
        let mut effective = BTreeSet::new();
        for capability in declarations.required.iter().chain(&declarations.optional) {
            if !self.has_capability(capability) {
                continue;
            }
            effective.insert(capability.clone());
            match capability.name.as_str() {
                "http.host" => {
                    scoped.net_hosts.insert(capability.resource.clone());
                    scoped.fetch_hosts.insert(capability.resource.clone());
                }
                "fs.binding" => {
                    let root = self.read_bindings[&capability.resource].clone();
                    scoped.read_roots.push(root.clone());
                    scoped
                        .read_bindings
                        .insert(capability.resource.clone(), root);
                }
                _ => unreachable!("admission rejects unsupported capabilities"),
            }
        }
        (scoped, effective)
    }

    fn ensure_active(&self) -> Result<(), String> {
        if [self.active.as_ref(), self.request_active.as_ref()]
            .into_iter()
            .flatten()
            .any(|active| !active.load(Ordering::Acquire))
        {
            return Err("host capability scope has ended".to_owned());
        }
        Ok(())
    }

    fn scoped_error(&self, message: &str, error: impl std::fmt::Display) -> String {
        if self.active.is_some() {
            message.to_owned()
        } else {
            format!("{message}: {error}")
        }
    }

    pub(crate) fn for_request(&self, active: Arc<AtomicBool>) -> Self {
        let mut scoped = self.clone();
        scoped.request_active = Some(active);
        scoped
    }

    pub(crate) fn authorize_binding_read(&self, binding: &str, path: &Path) -> Result<(), String> {
        self.ensure_active()?;
        let root = self
            .read_bindings
            .get(binding)
            .ok_or_else(|| "filesystem binding is not granted".to_owned())?;
        if path.as_os_str().is_empty()
            || path.is_absolute()
            || path.components().any(|part| {
                matches!(
                    part,
                    std::path::Component::ParentDir | std::path::Component::Prefix(_)
                )
            })
        {
            return Err("expected a path relative to the filesystem binding".to_owned());
        }
        let mut options = OpenOptions::new();
        options.read(true).custom_flags(libc::O_NONBLOCK);
        let file = root
            .directory
            .open_with(path, &options)
            .map_err(|_| "filesystem path is unavailable within the binding".to_owned())?;
        if !file
            .metadata()
            .map_err(|_| "filesystem path is unavailable within the binding".to_owned())?
            .is_file()
        {
            return Err("filesystem binding reads require a regular file".to_owned());
        }
        Ok(())
    }

    pub(crate) fn authorize_scoped_url(&self, host: &str, url: &str) -> Result<(), String> {
        let url = reqwest::Url::parse(url).map_err(|_| "invalid HTTP URL".to_owned())?;
        if url.host_str() != Some(host) {
            return Err("HTTP destination is outside the handle scope".to_owned());
        }
        self.authorize_url(&url)
    }

    fn authorize_read(&self, path: &Path) -> Result<AuthorizedRead, String> {
        self.ensure_active()?;
        let absolute = if self.active.is_some() {
            std::fs::canonicalize(path)
        } else {
            std::path::absolute(path)
        }
        .map_err(|error| {
            if self.active.is_some() {
                "cannot resolve file path".to_owned()
            } else {
                format!("cannot resolve {}: {error}", path.display())
            }
        })?;
        for root in &self.read_roots {
            if let Ok(relative_path) = absolute.strip_prefix(&root.path) {
                let relative_path = relative_path.to_owned();
                return Ok(AuthorizedRead {
                    display_path: absolute,
                    relative_path,
                    directory: Arc::clone(&root.directory),
                });
            }
        }
        if self.active.is_some() {
            Err("read access denied by application capability scope".to_owned())
        } else {
            Err(format!(
                "read access denied for {}; grant a containing root with --allow-read",
                absolute.display()
            ))
        }
    }

    fn authorize_url(&self, url: &reqwest::Url) -> Result<(), String> {
        let host = self.authorized_http_host(url)?;
        if self.net_hosts.contains(&host) {
            Ok(())
        } else {
            Err(format!(
                "network access denied for {host}; grant it with --allow-net {host}"
            ))
        }
    }

    fn authorize_fetch_url(&self, url: &reqwest::Url) -> Result<(), String> {
        let host = self.authorized_http_host(url)?;
        if self.fetch_hosts.contains(&host) {
            Ok(())
        } else {
            Err(format!("Fetch capability denied for {host}"))
        }
    }

    fn authorized_http_host(&self, url: &reqwest::Url) -> Result<String, String> {
        self.ensure_active()?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(format!("unsupported URL scheme: {}", url.scheme()));
        }
        let host = url
            .host_str()
            .ok_or_else(|| "HTTP URL does not contain a host".to_owned())?;
        Ok(host.to_ascii_lowercase())
    }
}

#[derive(Clone, Default)]
struct TaskTracker {
    active: Arc<AtomicUsize>,
    notify: Arc<Notify>,
}

struct TaskGuard(TaskTracker);

impl Drop for TaskGuard {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::AcqRel);
        self.0.notify.notify_waiters();
    }
}

impl TaskTracker {
    fn spawn<F>(&self, future: F) -> AbortHandle
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.active.fetch_add(1, Ordering::AcqRel);
        let guard = TaskGuard(self.clone());
        tokio::spawn(async move {
            let _guard = guard;
            future.await;
        })
        .abort_handle()
    }

    fn spawn_blocking<F>(&self, function: F) -> AbortHandle
    where
        F: FnOnce() + Send + 'static,
    {
        self.active.fetch_add(1, Ordering::AcqRel);
        let guard = TaskGuard(self.clone());
        tokio::task::spawn_blocking(move || {
            let _guard = guard;
            function();
        })
        .abort_handle()
    }

    async fn run_blocking<F, T>(&self, function: F) -> Result<T, String>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        let (sender, receiver) = oneshot::channel();
        self.spawn_blocking(move || {
            let _ = sender.send(function());
        });
        receiver
            .await
            .map_err(|_| "Kunlun blocking host task stopped without a result".to_owned())
    }

    async fn wait_empty(&self, grace: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + grace;
        loop {
            let notified = self.notify.notified();
            if self.active.load(Ordering::Acquire) == 0 {
                return true;
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return self.active.load(Ordering::Acquire) == 0;
            }
        }
    }

    #[cfg(all(test, kunlun_jsc_native))]
    fn active(&self) -> usize {
        self.active.load(Ordering::Acquire)
    }
}

enum StreamMessage {
    Chunk(Vec<u8>),
    Eof,
    Error(String),
}

struct StreamState {
    evaluation_id: Option<u64>,
    receiver: Arc<Mutex<Receiver<StreamMessage>>>,
    producer: AbortHandle,
    pulling: bool,
}

impl Drop for StreamState {
    fn drop(&mut self) {
        self.producer.abort();
    }
}

struct OpenedStream {
    stream_id: u64,
    metadata: String,
    receiver: Receiver<StreamMessage>,
    producer: AbortHandle,
}

enum CompletionValue {
    Value(Result<String, String>),
    Stream(OpenedStream),
}

struct Completion {
    id: u64,
    value: CompletionValue,
    terminal_stream: bool,
}

struct PendingHostCall {
    evaluation_id: Option<u64>,
    request_id: Option<u64>,
    stream_id: Option<u64>,
    upload_id: Option<u64>,
    promise: DeferredPromise,
    task: AbortHandle,
}

struct UploadState {
    evaluation_id: Option<u64>,
    sender: Sender<Vec<u8>>,
    aborted: Arc<AtomicBool>,
}

pub(crate) struct HostDispatcher {
    next_id: Rc<Cell<u64>>,
    next_stream_id: Rc<Cell<u64>>,
    pending: Rc<RefCell<HashMap<u64, PendingHostCall>>>,
    request_ids: Rc<RefCell<HashMap<u64, u64>>>,
    streams: Rc<RefCell<HashMap<u64, StreamState>>>,
    uploads: Rc<RefCell<HashMap<u64, UploadState>>>,
    active_evaluation: Rc<Cell<Option<u64>>>,
    accepting: Rc<Cell<bool>>,
    completion_tx: Sender<Completion>,
    completion_rx: Receiver<Completion>,
    buffered_completions: VecDeque<Completion>,
    permissions: HostPermissions,
    http_client: reqwest::Client,
    fetch_client: reqwest::Client,
    tasks: TaskTracker,
}

impl HostDispatcher {
    pub(crate) fn new(permissions: HostPermissions) -> Result<Self, reqwest::Error> {
        let (completion_tx, completion_rx) = mpsc::channel(MAX_IN_FLIGHT_HOST_CALLS);
        let http_client = reqwest::Client::builder()
            .redirect(Policy::none())
            .build()?;
        let fetch_client = reqwest::Client::builder()
            .redirect(Policy::none())
            .no_proxy()
            .build()?;
        Ok(Self {
            next_id: Rc::new(Cell::new(1)),
            next_stream_id: Rc::new(Cell::new(1)),
            pending: Rc::new(RefCell::new(HashMap::new())),
            request_ids: Rc::new(RefCell::new(HashMap::new())),
            streams: Rc::new(RefCell::new(HashMap::new())),
            uploads: Rc::new(RefCell::new(HashMap::new())),
            active_evaluation: Rc::new(Cell::new(None)),
            accepting: Rc::new(Cell::new(true)),
            completion_tx,
            completion_rx,
            buffered_completions: VecDeque::new(),
            permissions,
            http_client,
            fetch_client,
            tasks: TaskTracker::default(),
        })
    }

    pub(crate) fn install(&self, vm: &JscVm) -> Result<(), JscError> {
        let next_id = Rc::clone(&self.next_id);
        let next_stream_id = Rc::clone(&self.next_stream_id);
        let pending = Rc::clone(&self.pending);
        let request_ids = Rc::clone(&self.request_ids);
        let streams = Rc::clone(&self.streams);
        let uploads = Rc::clone(&self.uploads);
        let active_evaluation = Rc::clone(&self.active_evaluation);
        let accepting = Rc::clone(&self.accepting);
        let completion_tx = self.completion_tx.clone();
        let permissions = self.permissions.clone();
        let http_client = self.http_client.clone();
        let fetch_client = self.fetch_client.clone();
        let tasks = self.tasks.clone();

        vm.install_host_call_scheduler(move |call, promise| {
            if call.operation == "host.cancel" {
                match parse_request_id(&call.payload) {
                    Ok(request_id) => cancel_request(
                        request_id,
                        &pending,
                        &request_ids,
                        &streams,
                        &uploads,
                        "Kunlun host operation aborted",
                    ),
                    Err(error) => {
                        let _ = promise.reject_message(&error);
                        return;
                    }
                }
                let _ = promise.resolve_undefined();
                return;
            }
            if call.operation == "stream.cancel" {
                match parse_stream_id(&call.payload) {
                    Ok(stream_id) => cancel_stream(
                        stream_id,
                        &pending,
                        &request_ids,
                        &streams,
                        "Kunlun stream cancelled",
                    ),
                    Err(error) => {
                        let _ = promise.reject_message(&error);
                        return;
                    }
                }
                let _ = promise.resolve_undefined();
                return;
            }
            if let Err(error) = permissions.ensure_active() {
                let _ = promise.reject_message(&error);
                return;
            }
            if !accepting.get() {
                let _ = promise.reject_message("Kunlun runtime is shutting down");
                return;
            }
            if matches!(
                call.operation.as_str(),
                "fetch.upload.close" | "fetch.upload.abort"
            ) {
                match serde_json::from_str::<UploadIdentity>(&call.payload) {
                    Ok(identity)
                        if uploads
                            .borrow()
                            .get(&identity.upload_id)
                            .is_some_and(|upload| {
                                upload.evaluation_id == active_evaluation.get()
                            }) =>
                    {
                        let upload = uploads.borrow_mut().remove(&identity.upload_id).unwrap();
                        if call.operation == "fetch.upload.abort" {
                            upload.aborted.store(true, Ordering::Release);
                        }
                        let _ = promise.resolve_undefined();
                    }
                    _ => {
                        let _ = promise.reject_message("unknown Fetch upload");
                    }
                }
                return;
            }
            if pending.borrow().len() >= MAX_IN_FLIGHT_HOST_CALLS {
                let _ = promise.reject_message(&format!(
                    "too many in-flight Kunlun host calls; limit is {MAX_IN_FLIGHT_HOST_CALLS}"
                ));
                return;
            }

            let request_id = match optional_request_id(&call.payload) {
                Ok(request_id) => request_id,
                Err(error) => {
                    let _ = promise.reject_message(&error);
                    return;
                }
            };
            if request_id.is_some_and(|id| request_ids.borrow().contains_key(&id)) {
                let _ = promise.reject_message("Kunlun host request ID is already active");
                return;
            }

            let id = take_id(&next_id);
            let evaluation_id = active_evaluation.get();
            let mut upload_receiver = None;
            let mut upload_id = None;
            if call.operation == "fetch.requestStream" {
                match serde_json::from_str::<FetchRequestPayload>(&call.payload) {
                    Ok(request) => {
                        if let Some(candidate) = request.upload_id {
                            if uploads.borrow().contains_key(&candidate)
                                || uploads.borrow().len() >= MAX_IN_FLIGHT_HOST_CALLS
                            {
                                let _ = promise.reject_message(
                                    "Fetch upload ID is already active or limit reached",
                                );
                                return;
                            }
                            let (sender, receiver) = mpsc::channel(STREAM_BUFFER_CHUNKS);
                            let aborted = Arc::new(AtomicBool::new(false));
                            uploads.borrow_mut().insert(
                                candidate,
                                UploadState {
                                    evaluation_id,
                                    sender,
                                    aborted: Arc::clone(&aborted),
                                },
                            );
                            upload_receiver = Some((receiver, aborted));
                            upload_id = Some(candidate);
                        }
                    }
                    Err(error) => {
                        let _ = promise.reject_message(&format!("invalid Fetch payload: {error}"));
                        return;
                    }
                }
            }
            let upload_sender = if call.operation == "fetch.upload.write" {
                match serde_json::from_str::<UploadIdentity>(&call.payload) {
                    Ok(identity) => match uploads.borrow().get(&identity.upload_id) {
                        Some(upload) if upload.evaluation_id == evaluation_id => {
                            Some(upload.sender.clone())
                        }
                        _ => {
                            let _ = promise.reject_message("unknown Fetch upload");
                            return;
                        }
                    },
                    Err(error) => {
                        let _ = promise.reject_message(&format!("invalid Fetch upload: {error}"));
                        return;
                    }
                }
            } else {
                None
            };
            let stream_id = if call.operation == "stream.next" {
                match prepare_stream_pull(&call.payload, &streams) {
                    Ok(stream_id) => Some(stream_id),
                    Err(error) => {
                        let _ = promise.reject_message(&error);
                        return;
                    }
                }
            } else {
                None
            };
            let opened_stream_id = matches!(
                call.operation.as_str(),
                "fs.openReadStream" | "http.requestStream" | "fetch.requestStream"
            )
            .then(|| take_id(&next_stream_id));
            let task = dispatch(
                id,
                call,
                stream_id,
                opened_stream_id,
                upload_receiver,
                upload_sender,
                &streams,
                permissions.clone(),
                http_client.clone(),
                fetch_client.clone(),
                completion_tx.clone(),
                tasks.clone(),
            );
            pending.borrow_mut().insert(
                id,
                PendingHostCall {
                    evaluation_id,
                    request_id,
                    stream_id,
                    upload_id,
                    promise,
                    task,
                },
            );
            if let Some(request_id) = request_id {
                request_ids.borrow_mut().insert(request_id, id);
            }
        })
    }

    pub(crate) fn begin_evaluation(&self, evaluation_id: u64) {
        debug_assert!(self.active_evaluation.get().is_none());
        self.active_evaluation.set(Some(evaluation_id));
    }

    pub(crate) fn admission_flag(&self) -> Rc<Cell<bool>> {
        Rc::clone(&self.accepting)
    }

    pub(crate) fn resource_counts(&self) -> crate::RuntimeResourceCounts {
        crate::RuntimeResourceCounts {
            pending_host_calls: self.pending.borrow().len(),
            request_ids: self.request_ids.borrow().len(),
            streams: self.streams.borrow().len(),
            uploads: self.uploads.borrow().len(),
            active_tasks: self.tasks.active.load(Ordering::Acquire),
            ..crate::RuntimeResourceCounts::default()
        }
    }

    pub(crate) fn finish_evaluation(&self, evaluation_id: u64) {
        if self.active_evaluation.get() == Some(evaluation_id) {
            self.active_evaluation.set(None);
        }
    }

    pub(crate) fn cancel_evaluation(&mut self, evaluation_id: u64) {
        self.finish_evaluation(evaluation_id);
        let stream_ids: Vec<_> = self
            .streams
            .borrow()
            .iter()
            .filter_map(|(id, stream)| (stream.evaluation_id == Some(evaluation_id)).then_some(*id))
            .collect();
        for stream_id in stream_ids {
            cancel_stream(
                stream_id,
                &self.pending,
                &self.request_ids,
                &self.streams,
                "Kunlun evaluation cancelled",
            );
        }
        let ids: Vec<_> = self
            .pending
            .borrow()
            .iter()
            .filter_map(|(id, call)| (call.evaluation_id == Some(evaluation_id)).then_some(*id))
            .collect();
        for id in &ids {
            remove_pending(*id, &self.pending, &self.request_ids, &self.streams, None);
        }
        self.uploads.borrow_mut().retain(|_, upload| {
            let keep = upload.evaluation_id != Some(evaluation_id);
            if !keep {
                upload.aborted.store(true, Ordering::Release);
            }
            keep
        });
        self.discard_completions(&ids.into_iter().collect());
    }

    pub(crate) fn settle_completions(&mut self, vm: &JscVm) -> Result<(), JscError> {
        while let Some(completion) = self
            .buffered_completions
            .pop_front()
            .or_else(|| self.completion_rx.try_recv().ok())
        {
            let call = remove_pending(
                completion.id,
                &self.pending,
                &self.request_ids,
                &self.streams,
                None,
            );
            let Some(call) = call else {
                continue;
            };
            if matches!(&completion.value, CompletionValue::Value(Err(_))) {
                if let Some(upload_id) = call.upload_id {
                    self.uploads.borrow_mut().remove(&upload_id);
                }
            }
            let terminal_stream = completion.terminal_stream;
            let stream_id = call.stream_id;
            match completion.value {
                CompletionValue::Value(Ok(value)) => call.promise.resolve_string(&value)?,
                CompletionValue::Value(Err(message)) => call.promise.reject_message(&message)?,
                CompletionValue::Stream(opened) => {
                    let stream_id = opened.stream_id;
                    self.streams.borrow_mut().insert(
                        stream_id,
                        StreamState {
                            evaluation_id: call.evaluation_id,
                            receiver: Arc::new(Mutex::new(opened.receiver)),
                            producer: opened.producer,
                            pulling: false,
                        },
                    );
                    if let Err(error) = call.promise.resolve_string(&opened.metadata) {
                        self.streams.borrow_mut().remove(&stream_id);
                        return Err(error);
                    }
                }
            }
            if terminal_stream {
                if let Some(stream_id) = stream_id {
                    self.streams.borrow_mut().remove(&stream_id);
                }
            }
            crate::checkpoint(vm)?;
        }
        Ok(())
    }

    pub(crate) async fn wait_for_completion(&mut self) {
        debug_assert!(self.buffered_completions.is_empty());
        if let Some(completion) = self.completion_rx.recv().await {
            self.buffered_completions.push_back(completion);
        }
    }

    pub(crate) async fn shutdown(&mut self, vm: &JscVm, grace: Duration) -> Result<bool, JscError> {
        self.accepting.set(false);
        self.active_evaluation.set(None);
        self.uploads.borrow_mut().clear();
        let stream_ids: Vec<_> = self.streams.borrow().keys().copied().collect();
        for stream_id in stream_ids {
            cancel_stream(
                stream_id,
                &self.pending,
                &self.request_ids,
                &self.streams,
                "Kunlun runtime is shutting down",
            );
        }
        let ids: Vec<_> = self.pending.borrow().keys().copied().collect();
        for id in ids {
            let _ = remove_pending(
                id,
                &self.pending,
                &self.request_ids,
                &self.streams,
                Some("Kunlun runtime is shutting down"),
            );
        }
        self.buffered_completions.clear();
        while self.completion_rx.try_recv().is_ok() {}
        crate::checkpoint(vm)?;
        Ok(self.tasks.wait_empty(grace).await)
    }

    fn discard_completions(&mut self, discarded: &HashSet<u64>) {
        self.buffered_completions
            .retain(|completion| !discarded.contains(&completion.id));
        while let Ok(completion) = self.completion_rx.try_recv() {
            if !discarded.contains(&completion.id) {
                self.buffered_completions.push_back(completion);
            }
        }
    }

    #[cfg(all(test, kunlun_jsc_native))]
    pub(crate) fn pending_count(&self) -> usize {
        self.pending.borrow().len()
    }

    #[cfg(all(test, kunlun_jsc_native))]
    pub(crate) fn stream_count(&self) -> usize {
        self.streams.borrow().len()
    }

    #[cfg(all(test, kunlun_jsc_native))]
    pub(crate) fn active_task_count(&self) -> usize {
        self.tasks.active()
    }
}

impl Drop for HostDispatcher {
    fn drop(&mut self) {
        self.accepting.set(false);
        for upload in self.uploads.borrow().values() {
            upload.aborted.store(true, Ordering::Release);
        }
        self.uploads.borrow_mut().clear();
        for call in self.pending.borrow().values() {
            call.task.abort();
        }
        self.pending.borrow_mut().clear();
        self.request_ids.borrow_mut().clear();
        self.streams.borrow_mut().clear();
    }
}

#[cfg(test)]
mod fetch_grant_tests {
    use super::*;
    use crate::{Capability, CapabilityRequirements};

    #[test]
    fn artifact_fetch_authority_is_the_declaration_grant_intersection() {
        let capability = Capability {
            name: "http.host".into(),
            resource: "api.example.test".into(),
        };
        let declarations = CapabilityRequirements {
            required: vec![capability.clone()],
            optional: vec![Capability {
                name: "http.host".into(),
                resource: "missing.test".into(),
            }],
        };
        let deployment = HostPermissions::none()
            .allow_net_host("api.example.test")
            .allow_net_host("unexpected.test")
            .allow_fetch_host("unexpected.test");
        let active = Arc::new(AtomicBool::new(true));
        let (permissions, effective) = deployment.scoped_to(&declarations, Arc::clone(&active));
        assert!(effective.contains(&capability));
        assert_eq!(effective.len(), 1);
        assert!(
            permissions
                .authorize_fetch_url(&reqwest::Url::parse("https://api.example.test/").unwrap())
                .is_ok()
        );
        assert!(
            permissions
                .authorize_fetch_url(&reqwest::Url::parse("https://unexpected.test/").unwrap())
                .is_err()
        );
        assert!(
            permissions
                .authorize_url(&reqwest::Url::parse("https://api.example.test/").unwrap())
                .is_ok()
        );
        active.store(false, Ordering::Release);
        assert!(
            permissions
                .authorize_fetch_url(&reqwest::Url::parse("https://api.example.test/").unwrap())
                .is_err()
        );
    }
}

fn take_id(next: &Cell<u64>) -> u64 {
    let id = next.get();
    next.set(id.wrapping_add(1).max(1));
    id
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RequestIdentity {
    request_id: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OptionalRequestIdentity {
    request_id: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StreamIdentity {
    stream_id: u64,
}

fn parse_request_id(payload: &str) -> Result<u64, String> {
    serde_json::from_str::<RequestIdentity>(payload)
        .map(|identity| identity.request_id)
        .map_err(|error| format!("invalid cancellation payload: {error}"))
}

fn optional_request_id(payload: &str) -> Result<Option<u64>, String> {
    serde_json::from_str::<OptionalRequestIdentity>(payload)
        .map(|identity| identity.request_id)
        .map_err(|error| format!("invalid host payload: {error}"))
}

fn parse_stream_id(payload: &str) -> Result<u64, String> {
    serde_json::from_str::<StreamIdentity>(payload)
        .map(|identity| identity.stream_id)
        .map_err(|error| format!("invalid stream payload: {error}"))
}

fn prepare_stream_pull(
    payload: &str,
    streams: &Rc<RefCell<HashMap<u64, StreamState>>>,
) -> Result<u64, String> {
    let stream_id = parse_stream_id(payload)?;
    let mut streams = streams.borrow_mut();
    let stream = streams
        .get_mut(&stream_id)
        .ok_or_else(|| format!("unknown or closed Kunlun stream: {stream_id}"))?;
    if stream.pulling {
        return Err(format!(
            "Kunlun stream {stream_id} already has an in-flight read"
        ));
    }
    stream.pulling = true;
    Ok(stream_id)
}

fn remove_pending(
    id: u64,
    pending: &Rc<RefCell<HashMap<u64, PendingHostCall>>>,
    request_ids: &Rc<RefCell<HashMap<u64, u64>>>,
    streams: &Rc<RefCell<HashMap<u64, StreamState>>>,
    rejection: Option<&str>,
) -> Option<PendingHostCall> {
    let call = pending.borrow_mut().remove(&id)?;
    if let Some(request_id) = call.request_id {
        request_ids.borrow_mut().remove(&request_id);
    }
    if let Some(stream_id) = call.stream_id {
        if let Some(stream) = streams.borrow_mut().get_mut(&stream_id) {
            stream.pulling = false;
        }
    }
    call.task.abort();
    match rejection {
        Some(message) => {
            let _ = call.promise.reject_message(message);
            None
        }
        None => Some(call),
    }
}

fn cancel_request(
    request_id: u64,
    pending: &Rc<RefCell<HashMap<u64, PendingHostCall>>>,
    request_ids: &Rc<RefCell<HashMap<u64, u64>>>,
    streams: &Rc<RefCell<HashMap<u64, StreamState>>>,
    uploads: &Rc<RefCell<HashMap<u64, UploadState>>>,
    message: &str,
) {
    let Some(id) = request_ids.borrow().get(&request_id).copied() else {
        return;
    };
    if let Some(upload_id) = pending.borrow().get(&id).and_then(|call| call.upload_id) {
        if let Some(upload) = uploads.borrow_mut().remove(&upload_id) {
            upload.aborted.store(true, Ordering::Release);
        }
    }
    let _ = remove_pending(id, pending, request_ids, streams, Some(message));
}

fn cancel_stream(
    stream_id: u64,
    pending: &Rc<RefCell<HashMap<u64, PendingHostCall>>>,
    request_ids: &Rc<RefCell<HashMap<u64, u64>>>,
    streams: &Rc<RefCell<HashMap<u64, StreamState>>>,
    message: &str,
) {
    streams.borrow_mut().remove(&stream_id);
    let pulls: Vec<_> = pending
        .borrow()
        .iter()
        .filter_map(|(id, call)| (call.stream_id == Some(stream_id)).then_some(*id))
        .collect();
    for id in pulls {
        let _ = remove_pending(id, pending, request_ids, streams, Some(message));
    }
}

#[allow(clippy::too_many_arguments)]
fn dispatch(
    id: u64,
    call: HostCall,
    stream_id: Option<u64>,
    opened_stream_id: Option<u64>,
    upload_receiver: Option<(Receiver<Vec<u8>>, Arc<AtomicBool>)>,
    upload_sender: Option<Sender<Vec<u8>>>,
    streams: &Rc<RefCell<HashMap<u64, StreamState>>>,
    permissions: HostPermissions,
    http_client: reqwest::Client,
    fetch_client: reqwest::Client,
    completion_tx: Sender<Completion>,
    tasks: TaskTracker,
) -> AbortHandle {
    let stream_receiver = stream_id.and_then(|stream_id| {
        streams
            .borrow()
            .get(&stream_id)
            .map(|stream| Arc::clone(&stream.receiver))
    });
    let task_tracker = tasks.clone();
    tasks.spawn(async move {
        let (value, terminal_stream) = match call.operation.as_str() {
            "fs.readTextFile" => (
                CompletionValue::Value(
                    read_text_file(&call.payload, &permissions, &task_tracker).await,
                ),
                false,
            ),
            "http.request" => (
                CompletionValue::Value(
                    http_request(&call.payload, &permissions, &http_client).await,
                ),
                false,
            ),
            "fs.openReadStream" => {
                let result = open_file_stream(
                    &call.payload,
                    opened_stream_id.expect("stream ID assigned to stream operation"),
                    &permissions,
                    &task_tracker,
                );
                (stream_result(result), false)
            }
            "http.requestStream" => {
                let result = open_http_stream(
                    &call.payload,
                    opened_stream_id.expect("stream ID assigned to stream operation"),
                    &permissions,
                    &http_client,
                    &task_tracker,
                )
                .await;
                (stream_result(result), false)
            }
            "fetch.requestStream" => {
                let result = open_fetch_stream(
                    &call.payload,
                    opened_stream_id.expect("stream ID assigned to stream operation"),
                    upload_receiver,
                    &permissions,
                    &fetch_client,
                    &task_tracker,
                )
                .await;
                (stream_result(result), false)
            }
            "fetch.upload.write" => (
                CompletionValue::Value(write_fetch_upload(&call.payload, upload_sender).await),
                false,
            ),
            "stream.next" => match stream_receiver {
                Some(receiver) => {
                    let message = receiver.lock().await.recv().await;
                    match message {
                        Some(StreamMessage::Chunk(bytes)) => {
                            (CompletionValue::Value(encode_stream_chunk(&bytes)), false)
                        }
                        Some(StreamMessage::Eof) => {
                            (CompletionValue::Value(encode_stream_eof()), true)
                        }
                        Some(StreamMessage::Error(error)) => {
                            (CompletionValue::Value(Err(error)), true)
                        }
                        None => (
                            CompletionValue::Value(Err(
                                "Kunlun stream producer stopped without a terminal message"
                                    .to_owned(),
                            )),
                            true,
                        ),
                    }
                }
                None => (
                    CompletionValue::Value(Err("Kunlun stream is no longer available".to_owned())),
                    true,
                ),
            },
            operation => (
                CompletionValue::Value(Err(format!("unknown Kunlun host operation: {operation}"))),
                false,
            ),
        };
        let _ = completion_tx
            .send(Completion {
                id,
                value,
                terminal_stream,
            })
            .await;
    })
}

fn stream_result(result: Result<OpenedStream, String>) -> CompletionValue {
    match result {
        Ok(stream) => CompletionValue::Stream(stream),
        Err(error) => CompletionValue::Value(Err(error)),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReadTextFilePayload {
    path: String,
    request_id: Option<u64>,
}

async fn read_text_file(
    payload: &str,
    permissions: &HostPermissions,
    tasks: &TaskTracker,
) -> Result<String, String> {
    let request: ReadTextFilePayload =
        serde_json::from_str(payload).map_err(|error| format!("invalid fs payload: {error}"))?;
    let _ = request.request_id;
    let authorized = permissions.authorize_read(Path::new(&request.path))?;
    let display_path = if permissions.active.is_some() {
        "scoped file".to_owned()
    } else {
        authorized.display_path.display().to_string()
    };
    tasks
        .run_blocking(move || {
            let mut options = OpenOptions::new();
            options.read(true).custom_flags(libc::O_NONBLOCK);
            let file = authorized
                .directory
                .open_with(authorized.relative_path, &options)?;
            if !file.metadata()?.is_file() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "filesystem reads require a regular file",
                ));
            }
            let mut bytes = Vec::new();
            file.take((MAX_HTTP_RESPONSE_BYTES + 1) as u64)
                .read_to_end(&mut bytes)?;
            if bytes.len() > MAX_HTTP_RESPONSE_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::FileTooLarge,
                    format!("file exceeds the {MAX_HTTP_RESPONSE_BYTES}-byte bootstrap limit"),
                ));
            }
            String::from_utf8(bytes)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
        })
        .await
        .map_err(|error| {
            permissions.scoped_error(&format!("file read task failed for {display_path}"), error)
        })?
        .map_err(|error| {
            permissions.scoped_error(&format!("cannot read {display_path} as UTF-8 text"), error)
        })
}

fn open_file_stream(
    payload: &str,
    stream_id: u64,
    permissions: &HostPermissions,
    tasks: &TaskTracker,
) -> Result<OpenedStream, String> {
    let request: ReadTextFilePayload =
        serde_json::from_str(payload).map_err(|error| format!("invalid fs payload: {error}"))?;
    let authorized = permissions.authorize_read(Path::new(&request.path))?;
    let display_path = if permissions.active.is_some() {
        "scoped file".to_owned()
    } else {
        authorized.display_path.display().to_string()
    };
    let scoped = permissions.active.is_some();
    let (sender, receiver) = mpsc::channel(STREAM_BUFFER_CHUNKS);
    let producer = tasks.spawn_blocking(move || {
        let result = (|| -> io::Result<()> {
            let mut options = OpenOptions::new();
            options.read(true).custom_flags(libc::O_NONBLOCK);
            let mut file = authorized
                .directory
                .open_with(authorized.relative_path, &options)?;
            if !file.metadata()?.is_file() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "filesystem streams require a regular file",
                ));
            }
            let mut buffer = vec![0; STREAM_CHUNK_BYTES];
            loop {
                let read = file.read(&mut buffer)?;
                if read == 0 {
                    let _ = sender.blocking_send(StreamMessage::Eof);
                    return Ok(());
                }
                if sender
                    .blocking_send(StreamMessage::Chunk(buffer[..read].to_vec()))
                    .is_err()
                {
                    return Ok(());
                }
            }
        })();
        if let Err(error) = result {
            let detail = if scoped {
                "cannot stream scoped file".to_owned()
            } else {
                format!("cannot stream {display_path}: {error}")
            };
            let _ = sender.blocking_send(StreamMessage::Error(detail));
        }
    });
    let metadata = serde_json::to_string(&StreamMetadata { stream_id })
        .map_err(|error| format!("could not encode stream metadata: {error}"))?;
    Ok(OpenedStream {
        stream_id,
        metadata,
        receiver,
        producer,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct HttpRequestPayload {
    url: String,
    method: String,
    #[serde(default)]
    headers: BTreeMap<String, String>,
    body: Option<String>,
    request_id: Option<u64>,
}

#[derive(Serialize)]
struct HttpResponsePayload {
    status: u16,
    headers: BTreeMap<String, String>,
    body: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StreamMetadata {
    stream_id: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HttpStreamMetadata {
    stream_id: u64,
    status: u16,
    headers: BTreeMap<String, String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FetchRequestPayload {
    url: String,
    method: String,
    headers: Vec<(String, String)>,
    body_base64: Option<String>,
    upload_id: Option<u64>,
    request_id: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UploadIdentity {
    upload_id: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UploadChunkPayload {
    upload_id: u64,
    chunk_base64: String,
}

async fn write_fetch_upload(
    payload: &str,
    sender: Option<Sender<Vec<u8>>>,
) -> Result<String, String> {
    let request: UploadChunkPayload =
        serde_json::from_str(payload).map_err(|error| format!("invalid Fetch upload: {error}"))?;
    let _ = request.upload_id;
    if request.chunk_base64.len() > 90_000 {
        return Err("Fetch upload chunk exceeds 64 KiB".into());
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(request.chunk_base64)
        .map_err(|error| format!("invalid Fetch upload chunk: {error}"))?;
    if bytes.len() > STREAM_CHUNK_BYTES {
        return Err("Fetch upload chunk exceeds 64 KiB".into());
    }
    sender
        .ok_or("unknown Fetch upload")?
        .send(bytes)
        .await
        .map_err(|_| "Fetch upload was cancelled".to_owned())?;
    Ok(String::new())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FetchStreamMetadata {
    stream_id: u64,
    url: String,
    status: u16,
    status_text: String,
    headers: Vec<(String, String)>,
}

async fn open_fetch_stream(
    payload: &str,
    stream_id: u64,
    upload_receiver: Option<(Receiver<Vec<u8>>, Arc<AtomicBool>)>,
    permissions: &HostPermissions,
    client: &reqwest::Client,
    tasks: &TaskTracker,
) -> Result<OpenedStream, String> {
    let request: FetchRequestPayload =
        serde_json::from_str(payload).map_err(|error| format!("invalid Fetch payload: {error}"))?;
    let _ = request.request_id;
    let url = reqwest::Url::parse(&request.url)
        .map_err(|error| permissions.scoped_error("invalid Fetch URL", error))?;
    permissions.authorize_fetch_url(&url)?;
    let method = reqwest::Method::from_bytes(request.method.as_bytes())
        .map_err(|error| format!("invalid Fetch method: {error}"))?;
    if request.headers.len() > 256
        || request
            .headers
            .iter()
            .map(|(n, v)| n.len() + v.len())
            .sum::<usize>()
            > 64 * 1024
    {
        return Err("Fetch headers exceed the profile limit".into());
    }
    let mut builder = client.request(method, url);
    for (name, value) in request.headers {
        if matches!(
            name.to_ascii_lowercase().as_str(),
            "host"
                | "content-length"
                | "transfer-encoding"
                | "connection"
                | "upgrade"
                | "proxy-authorization"
                | "te"
                | "trailer"
                | "keep-alive"
        ) {
            return Err(format!("unsupported Fetch request header: {name}"));
        }
        builder = builder.header(&name, &value);
    }
    if request.upload_id.is_some() && request.body_base64.is_some() {
        return Err("Fetch request cannot combine buffered and streaming bodies".into());
    }
    if request.upload_id.is_some() != upload_receiver.is_some() {
        return Err("Fetch upload channel is missing".into());
    }
    if let Some((receiver, aborted)) = upload_receiver {
        let stream = futures_util::stream::unfold(
            (receiver, aborted, false),
            |(mut receiver, aborted, ended)| async move {
                if ended {
                    return None;
                }
                match receiver.recv().await {
                    Some(bytes) => {
                        Some((Ok::<Vec<u8>, io::Error>(bytes), (receiver, aborted, false)))
                    }
                    None if aborted.load(Ordering::Acquire) => Some((
                        Err(io::Error::other("Fetch upload aborted")),
                        (receiver, aborted, true),
                    )),
                    None => None,
                }
            },
        );
        builder = builder.body(reqwest::Body::wrap_stream(stream));
    } else if let Some(body) = request.body_base64 {
        if body.len() > 2 * 1024 * 1024 {
            return Err("Fetch request body exceeds the profile limit".into());
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(body)
            .map_err(|error| format!("invalid Fetch request body: {error}"))?;
        if bytes.len() > 1024 * 1024 {
            return Err("Fetch request body exceeds the profile limit".into());
        }
        builder = builder.body(bytes);
    }
    let response = builder
        .send()
        .await
        .map_err(|error| permissions.scoped_error("Fetch request failed", error))?;
    let metadata = serde_json::to_string(&FetchStreamMetadata {
        stream_id,
        url: response.url().to_string(),
        status: response.status().as_u16(),
        status_text: response
            .status()
            .canonical_reason()
            .unwrap_or("")
            .to_owned(),
        headers: response
            .headers()
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_owned(),
                    value
                        .as_bytes()
                        .iter()
                        .map(|byte| char::from(*byte))
                        .collect(),
                )
            })
            .collect(),
    })
    .map_err(|error| format!("could not encode Fetch metadata: {error}"))?;
    let (sender, receiver) = mpsc::channel(STREAM_BUFFER_CHUNKS);
    let scoped = permissions.active.is_some();
    let producer = tasks.spawn(async move {
        let mut response = response;
        loop {
            match response.chunk().await {
                Ok(Some(chunk)) => {
                    for part in chunk.chunks(STREAM_CHUNK_BYTES) {
                        if sender
                            .send(StreamMessage::Chunk(part.to_vec()))
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                }
                Ok(None) => {
                    let _ = sender.send(StreamMessage::Eof).await;
                    return;
                }
                Err(error) => {
                    let message = if scoped {
                        "Fetch body failed".to_owned()
                    } else {
                        format!("Fetch body failed: {error}")
                    };
                    let _ = sender.send(StreamMessage::Error(message)).await;
                    return;
                }
            }
        }
    });
    Ok(OpenedStream {
        stream_id,
        metadata,
        receiver,
        producer,
    })
}

fn prepare_http_request(
    payload: &str,
    permissions: &HostPermissions,
    client: &reqwest::Client,
) -> Result<reqwest::RequestBuilder, String> {
    let request: HttpRequestPayload =
        serde_json::from_str(payload).map_err(|error| format!("invalid HTTP payload: {error}"))?;
    let _ = request.request_id;
    let url =
        reqwest::Url::parse(&request.url).map_err(|error| format!("invalid HTTP URL: {error}"))?;
    permissions.authorize_url(&url)?;
    let method = reqwest::Method::from_bytes(request.method.as_bytes())
        .map_err(|error| format!("invalid HTTP method: {error}"))?;
    let mut builder = client.request(method, url);
    for (name, value) in request.headers {
        builder = builder.header(&name, &value);
    }
    if let Some(body) = request.body {
        builder = builder.body(body);
    }
    Ok(builder)
}

fn response_headers(response: &reqwest::Response) -> BTreeMap<String, String> {
    response
        .headers()
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_owned(),
                value.to_str().unwrap_or("<non-UTF-8>").to_owned(),
            )
        })
        .collect()
}

async fn http_request(
    payload: &str,
    permissions: &HostPermissions,
    client: &reqwest::Client,
) -> Result<String, String> {
    let mut response = prepare_http_request(payload, permissions, client)?
        .send()
        .await
        .map_err(|error| permissions.scoped_error("HTTP request failed", error))?;
    let status = response.status().as_u16();
    let headers = response_headers(&response);
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| permissions.scoped_error("could not read HTTP response body", error))?
    {
        if body.len().saturating_add(chunk.len()) > MAX_HTTP_RESPONSE_BYTES {
            return Err(format!(
                "HTTP response exceeds the {MAX_HTTP_RESPONSE_BYTES}-byte bootstrap limit"
            ));
        }
        body.extend_from_slice(&chunk);
    }
    let body = String::from_utf8(body)
        .map_err(|error| format!("HTTP response body is not UTF-8: {error}"))?;
    serde_json::to_string(&HttpResponsePayload {
        status,
        headers,
        body,
    })
    .map_err(|error| format!("could not encode HTTP response: {error}"))
}

async fn open_http_stream(
    payload: &str,
    stream_id: u64,
    permissions: &HostPermissions,
    client: &reqwest::Client,
    tasks: &TaskTracker,
) -> Result<OpenedStream, String> {
    let mut response = prepare_http_request(payload, permissions, client)?
        .send()
        .await
        .map_err(|error| permissions.scoped_error("HTTP request failed", error))?;
    let metadata = serde_json::to_string(&HttpStreamMetadata {
        stream_id,
        status: response.status().as_u16(),
        headers: response_headers(&response),
    })
    .map_err(|error| format!("could not encode HTTP stream metadata: {error}"))?;
    let (sender, receiver) = mpsc::channel(STREAM_BUFFER_CHUNKS);
    let scoped = permissions.active.is_some();
    let producer = tasks.spawn(async move {
        loop {
            match response.chunk().await {
                Ok(Some(chunk)) => {
                    for part in chunk.chunks(STREAM_CHUNK_BYTES) {
                        if sender
                            .send(StreamMessage::Chunk(part.to_vec()))
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                }
                Ok(None) => {
                    let _ = sender.send(StreamMessage::Eof).await;
                    return;
                }
                Err(error) => {
                    let detail = if scoped {
                        "could not read HTTP response body".to_owned()
                    } else {
                        format!("could not read HTTP response body: {error}")
                    };
                    let _ = sender.send(StreamMessage::Error(detail)).await;
                    return;
                }
            }
        }
    });
    Ok(OpenedStream {
        stream_id,
        metadata,
        receiver,
        producer,
    })
}

#[derive(Serialize)]
struct StreamChunkPayload<'a> {
    done: bool,
    value: &'a str,
}

#[derive(Serialize)]
struct StreamEofPayload {
    done: bool,
}

fn encode_stream_chunk(bytes: &[u8]) -> Result<String, String> {
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    serde_json::to_string(&StreamChunkPayload {
        done: false,
        value: &encoded,
    })
    .map_err(|error| format!("could not encode stream chunk: {error}"))
}

fn encode_stream_eof() -> Result<String, String> {
    serde_json::to_string(&StreamEofPayload { done: true })
        .map_err(|error| format!("could not encode stream EOF: {error}"))
}

#[cfg(test)]
mod checkpoint_tests {
    use super::*;

    #[test]
    fn completions_and_nested_promise_jobs_follow_channel_fifo() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let vm = JscVm::new("completion-fifo").unwrap();
            let mut host = HostDispatcher::new(HostPermissions::none()).unwrap();
            host.install(&vm).unwrap();
            host.begin_evaluation(1);
            vm.evaluate("globalThis.order = []; for (let i = 1; i <= 2; i++) __kunlunHostCall('test', '{}').then(v => { order.push(v); Promise.resolve().then(() => order.push(v + '-job')); });", "test:///completions.js").unwrap();
            for id in [2, 1] {
                host.completion_tx
                    .try_send(Completion {
                        id,
                        value: CompletionValue::Value(Ok(id.to_string())),
                        terminal_stream: false,
                    })
                    .unwrap();
            }
            host.settle_completions(&vm).unwrap();
            assert_eq!(
                vm.evaluate("order.join(',')", "test:///read.js").unwrap(),
                "2,2-job,1,1-job"
            );
            host.cancel_evaluation(1);
        });
    }

    #[test]
    fn host_rejection_is_observed_at_its_checkpoint() {
        if !JscVm::backend_info().supports_explicit_microtask_checkpoint {
            return;
        }
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let vm = JscVm::new("completion-rejection").unwrap();
            let mut host = HostDispatcher::new(HostPermissions::none()).unwrap();
            host.install(&vm).unwrap();
            vm.evaluate("__kunlunHostCall('test', '{}');", "test:///host.js")
                .unwrap();
            host.completion_tx
                .try_send(Completion {
                    id: 1,
                    value: CompletionValue::Value(Err("denied".to_owned())),
                    terminal_stream: false,
                })
                .unwrap();
            host.settle_completions(&vm).unwrap();
            let records = vm.take_promise_rejections();
            assert_eq!(records.len(), 1);
            assert_eq!(records[0].exception.source_url(), Some("test:///host.js"));
            assert!(
                records[0]
                    .exception
                    .exception_text()
                    .unwrap()
                    .contains("denied")
            );
        });
    }

    #[test]
    fn late_completion_cannot_settle_a_reused_client_request_id() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let vm = JscVm::new("stale-completion").unwrap();
            let mut host = HostDispatcher::new(HostPermissions::none()).unwrap();
            host.install(&vm).unwrap();
            vm.evaluate(
                "globalThis.values = [];\n\
                 __kunlunHostCall('test', '{\"requestId\":7}').then(\n\
                   value => values.push('old:' + value),\n\
                   () => values.push('old:cancelled'));\n\
                 __kunlunHostCall('host.cancel', '{\"requestId\":7}');\n\
                 __kunlunHostCall('test', '{\"requestId\":7}').then(\n\
                   value => values.push('new:' + value));",
                "test:///stale.js",
            )
            .unwrap();
            host.completion_tx
                .try_send(Completion {
                    id: 1,
                    value: CompletionValue::Value(Ok("stale".to_owned())),
                    terminal_stream: false,
                })
                .unwrap();
            host.completion_tx
                .try_send(Completion {
                    id: 2,
                    value: CompletionValue::Value(Ok("fresh".to_owned())),
                    terminal_stream: false,
                })
                .unwrap();
            host.settle_completions(&vm).unwrap();
            crate::checkpoint(&vm).unwrap();
            assert_eq!(
                vm.evaluate("values.join(',')", "test:///stale-result.js")
                    .unwrap(),
                "old:cancelled,new:fresh"
            );
        });
    }

    #[test]
    fn file_producer_stops_when_its_bounded_receiver_is_dropped() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let root = std::env::temp_dir()
                .join(format!("kunlun-stream-backpressure-{}", std::process::id()));
            std::fs::create_dir_all(&root).unwrap();
            let path = root.join("large.bin");
            std::fs::write(
                &path,
                vec![0_u8; STREAM_CHUNK_BYTES * (STREAM_BUFFER_CHUNKS + 2)],
            )
            .unwrap();
            let permissions = HostPermissions::none().allow_read_root(&root).unwrap();
            let tracker = TaskTracker::default();
            let payload = serde_json::json!({ "path": path, "requestId": 1 }).to_string();
            let stream = open_file_stream(&payload, 1, &permissions, &tracker).unwrap();
            tokio::task::yield_now().await;
            assert!(stream.receiver.len() <= STREAM_BUFFER_CHUNKS);
            drop(stream);
            assert!(tracker.wait_empty(Duration::from_secs(1)).await);
            std::fs::remove_dir_all(root).unwrap();
        });
    }

    #[test]
    fn task_tracker_has_a_deterministic_forced_shutdown_deadline() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let tracker = TaskTracker::default();
            let (release_tx, release_rx) = std::sync::mpsc::channel();
            tracker.spawn_blocking(move || release_rx.recv().unwrap());
            assert!(!tracker.wait_empty(Duration::from_millis(1)).await);
            release_tx.send(()).unwrap();
            assert!(tracker.wait_empty(Duration::from_secs(1)).await);
        });
    }

    #[test]
    fn task_tracker_releases_guards_for_tasks_aborted_before_execution() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .max_blocking_threads(1)
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let tracker = TaskTracker::default();
            let asynchronous = tracker.spawn(std::future::pending());
            asynchronous.abort();

            let (started_tx, started_rx) = std::sync::mpsc::channel();
            let (release_tx, release_rx) = std::sync::mpsc::channel();
            tracker.spawn_blocking(move || {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            });
            started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
            let queued = tracker.spawn_blocking(|| {});
            queued.abort();
            release_tx.send(()).unwrap();

            assert!(tracker.wait_empty(Duration::from_secs(1)).await);
        });
    }
}
