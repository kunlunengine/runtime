use cap_std::ambient_authority;
use cap_std::fs::Dir;
use kunlun_jsc::{DeferredPromise, HostCall, JscError, JscVm};
use reqwest::redirect::Policy;
use serde::{Deserialize, Serialize};
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

pub(crate) const MAX_HTTP_RESPONSE_BYTES: usize = 1024 * 1024;
pub(crate) const MAX_IN_FLIGHT_HOST_CALLS: usize = 256;

#[derive(Debug, Clone, Default)]
pub struct HostPermissions {
    read_roots: Vec<ReadRoot>,
    net_hosts: HashSet<String>,
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

    pub fn allow_net_host(mut self, host: impl Into<String>) -> Self {
        self.net_hosts.insert(host.into().to_ascii_lowercase());
        self
    }

    fn authorize_read(&self, path: &Path) -> Result<AuthorizedRead, String> {
        let absolute = std::path::absolute(path)
            .map_err(|error| format!("cannot resolve {}: {error}", path.display()))?;
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
        Err(format!(
            "read access denied for {}; grant a containing root with --allow-read",
            absolute.display()
        ))
    }

    fn authorize_url(&self, url: &reqwest::Url) -> Result<(), String> {
        if !matches!(url.scheme(), "http" | "https") {
            return Err(format!("unsupported URL scheme: {}", url.scheme()));
        }
        let host = url
            .host_str()
            .ok_or_else(|| "HTTP URL does not contain a host".to_owned())?
            .to_ascii_lowercase();
        if self.net_hosts.contains(&host) {
            Ok(())
        } else {
            Err(format!(
                "network access denied for {host}; grant it with --allow-net {host}"
            ))
        }
    }
}

struct Completion {
    id: u64,
    result: Result<String, String>,
}

pub(crate) struct HostDispatcher {
    next_id: Rc<Cell<u64>>,
    pending: Rc<RefCell<HashMap<u64, DeferredPromise>>>,
    completion_tx: UnboundedSender<Completion>,
    completion_rx: UnboundedReceiver<Completion>,
    buffered_completion: Option<Completion>,
    permissions: HostPermissions,
    http_client: reqwest::Client,
}

impl HostDispatcher {
    pub(crate) fn new(permissions: HostPermissions) -> Result<Self, reqwest::Error> {
        let (completion_tx, completion_rx) = mpsc::unbounded_channel();
        let http_client = reqwest::Client::builder()
            .redirect(Policy::none())
            .build()?;
        Ok(Self {
            next_id: Rc::new(Cell::new(1)),
            pending: Rc::new(RefCell::new(HashMap::new())),
            completion_tx,
            completion_rx,
            buffered_completion: None,
            permissions,
            http_client,
        })
    }

    pub(crate) fn install(&self, vm: &JscVm) -> Result<(), JscError> {
        let next_id = Rc::clone(&self.next_id);
        let pending = Rc::clone(&self.pending);
        let completion_tx = self.completion_tx.clone();
        let permissions = self.permissions.clone();
        let http_client = self.http_client.clone();

        vm.install_host_call_scheduler(move |call, promise| {
            // Pending entries are retained through completion settlement, so this
            // admission check also bounds the completion queue.
            if pending.borrow().len() >= MAX_IN_FLIGHT_HOST_CALLS {
                let _ = promise.reject_message(&format!(
                    "too many in-flight Kunlun host calls; limit is {MAX_IN_FLIGHT_HOST_CALLS}"
                ));
                return;
            }

            let id = next_id.get();
            next_id.set(id.wrapping_add(1).max(1));
            pending.borrow_mut().insert(id, promise);
            dispatch(
                id,
                call,
                permissions.clone(),
                http_client.clone(),
                completion_tx.clone(),
            );
        })
    }

    pub(crate) fn settle_completions(&mut self) -> Result<(), JscError> {
        while let Some(completion) = self
            .buffered_completion
            .take()
            .or_else(|| self.completion_rx.try_recv().ok())
        {
            let promise = self.pending.borrow_mut().remove(&completion.id);
            let Some(promise) = promise else {
                continue;
            };
            match completion.result {
                Ok(value) => promise.resolve_string(&value)?,
                Err(message) => promise.reject_message(&message)?,
            }
        }
        Ok(())
    }

    pub(crate) async fn wait_for_completion(&mut self) {
        debug_assert!(self.buffered_completion.is_none());
        self.buffered_completion = self.completion_rx.recv().await;
    }
}

fn dispatch(
    id: u64,
    call: HostCall,
    permissions: HostPermissions,
    http_client: reqwest::Client,
    completion_tx: UnboundedSender<Completion>,
) {
    tokio::spawn(async move {
        let result = match call.operation.as_str() {
            "fs.readTextFile" => read_text_file(&call.payload, &permissions).await,
            "http.request" => http_request(&call.payload, &permissions, &http_client).await,
            operation => Err(format!("unknown Kunlun host operation: {operation}")),
        };
        let _ = completion_tx.send(Completion { id, result });
    });
}

#[derive(Deserialize)]
struct ReadTextFilePayload {
    path: String,
}

async fn read_text_file(payload: &str, permissions: &HostPermissions) -> Result<String, String> {
    let request: ReadTextFilePayload =
        serde_json::from_str(payload).map_err(|error| format!("invalid fs payload: {error}"))?;
    let authorized = permissions.authorize_read(Path::new(&request.path))?;
    let display_path = authorized.display_path;
    tokio::task::spawn_blocking(move || {
        let file = authorized.directory.open(authorized.relative_path)?;
        let mut bytes = Vec::new();
        file.take((MAX_HTTP_RESPONSE_BYTES + 1) as u64)
            .read_to_end(&mut bytes)?;
        if bytes.len() > MAX_HTTP_RESPONSE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::FileTooLarge,
                format!("file exceeds the {MAX_HTTP_RESPONSE_BYTES}-byte bootstrap limit"),
            ));
        }
        String::from_utf8(bytes).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    })
    .await
    .map_err(|error| {
        format!(
            "file read task failed for {}: {error}",
            display_path.display()
        )
    })?
    .map_err(|error| {
        format!(
            "cannot read {} as UTF-8 text: {error}",
            display_path.display()
        )
    })
}

#[derive(Deserialize)]
struct HttpRequestPayload {
    url: String,
    method: String,
    #[serde(default)]
    headers: BTreeMap<String, String>,
    body: Option<String>,
}

#[derive(Serialize)]
struct HttpResponsePayload {
    status: u16,
    headers: BTreeMap<String, String>,
    body: String,
}

async fn http_request(
    payload: &str,
    permissions: &HostPermissions,
    client: &reqwest::Client,
) -> Result<String, String> {
    let request: HttpRequestPayload =
        serde_json::from_str(payload).map_err(|error| format!("invalid HTTP payload: {error}"))?;
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

    let mut response = builder
        .send()
        .await
        .map_err(|error| format!("HTTP request failed: {error}"))?;
    let status = response.status().as_u16();
    let headers = response
        .headers()
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_owned(),
                value.to_str().unwrap_or("<non-UTF-8>").to_owned(),
            )
        })
        .collect();
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| format!("could not read HTTP response body: {error}"))?
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
