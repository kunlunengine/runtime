//! Host cleanup scheduling evidence. This does not qualify admitted Fetch entry loading.
use kunlun_runtime::{HostPermissions, RuntimeResourceCounts, TokioIsolate};
use std::net::TcpListener;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};
use tokio::sync::oneshot;

struct HeldConnection {
    url: String,
    stop: mpsc::Sender<()>,
    worker: Option<thread::JoinHandle<()>>,
}

impl HeldConnection {
    fn start() -> (Self, oneshot::Receiver<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!(
            "http://127.0.0.1:{}/pending",
            listener.local_addr().unwrap().port()
        );
        let (started_tx, started_rx) = oneshot::channel();
        let (stop, stopped) = mpsc::channel();
        let worker = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                if stopped.try_recv().is_ok() || Instant::now() >= deadline {
                    return;
                }
                match listener.accept() {
                    Ok((stream, _)) => {
                        let _ = started_tx.send(());
                        // Keep the socket open without headers, so the real HTTP worker suspends.
                        let _ = stopped.recv_timeout(Duration::from_secs(5));
                        drop(stream);
                        return;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(1));
                    }
                    Err(error) => panic!("loopback fixture accept failed: {error}"),
                }
            }
        });
        (
            Self {
                url,
                stop,
                worker: Some(worker),
            },
            started_rx,
        )
    }
}

impl Drop for HeldConnection {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn dropping_host_invocation_clears_tables_and_aborted_worker_drains_after_yield() {
    let (connection, mut started) = HeldConnection::start();
    let mut isolate = TokioIsolate::new_with_permissions(
        "worker cleanup scheduling",
        HostPermissions::none().allow_net_host("127.0.0.1"),
    )
    .unwrap();
    let source = format!(
        "const http = await kunlun.import('kunlun:http'); \
         await http.request({}); return 'unexpected completion';",
        serde_json::to_string(&connection.url).unwrap()
    );
    {
        let mut invocation =
            Box::pin(isolate.evaluate_async_body(&source, "kunlun:worker-cleanup"));
        tokio::time::timeout(Duration::from_secs(5), async {
            tokio::select! {
                result = &mut invocation => panic!("worker unexpectedly completed: {result:?}"),
                ready = &mut started => ready.unwrap(),
            }
        })
        .await
        .unwrap();
        // The worker is confirmed in flight; dropping is cancellation, not normal completion.
        drop(invocation);
    }
    let immediate = isolate.resource_counts();
    assert!(immediate.active_tasks <= 1);
    assert_eq!(
        RuntimeResourceCounts {
            active_tasks: 0,
            ..immediate
        },
        RuntimeResourceCounts::default()
    );
    // AbortHandle schedules cancellation; keep the owning executor running for task destruction.
    tokio::time::timeout(Duration::from_secs(1), async {
        while isolate.resource_counts().active_tasks != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(isolate.resource_counts(), RuntimeResourceCounts::default());
}
