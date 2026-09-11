use kunlun_jsc::{JscVm, PromiseRejection, PromiseRejectionTransition};
use kunlun_runtime::{
    DEFAULT_SHUTDOWN_GRACE, EVENT_LOOP_BACKEND, HostPermissions, ModuleSources, ShutdownOutcome,
    TYPESCRIPT_DECLARATIONS, TokioIsolate,
};
use std::env;
use std::fs;
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;
use tokio::runtime::Builder;

#[cfg(unix)]
use tokio::signal::unix::{Signal, SignalKind};

fn main() -> ExitCode {
    match run(env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Vec<String>) -> Result<(), String> {
    match args.first().map(String::as_str) {
        Some("eval") => eval_command(&args[1..]),
        Some("eval-async") => eval_async_command(&args[1..]),
        Some("run") => run_command(&args[1..]),
        Some("run-async") => run_async_command(&args[1..]),
        Some("run-module") => run_module_command(&args[1..]),
        Some("doctor") => doctor_command(),
        Some("types") => {
            print!("{TYPESCRIPT_DECLARATIONS}");
            Ok(())
        }
        Some("--version" | "-V" | "version") => {
            println!("kunlun-runtime {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("help" | "--help" | "-h") | None => {
            print_help();
            Ok(())
        }
        Some(command) => Err(format!(
            "unknown command `{command}`; run `kunlun-runtime help`"
        )),
    }
}

fn eval_command(args: &[String]) -> Result<(), String> {
    let source = args
        .first()
        .ok_or_else(|| "usage: kunlun-runtime eval <source>".to_owned())?;
    let vm = JscVm::new("kunlun-runtime eval").map_err(|error| error.to_string())?;
    evaluate_script(&vm, source, "kunlun:eval")
}

fn eval_async_command(args: &[String]) -> Result<(), String> {
    let options = parse_async_options(args, "eval-async <async-function-body>")?;
    evaluate_async(
        &options.subject,
        "kunlun:eval-async",
        "kunlun-runtime eval-async",
        options.permissions,
        options.shutdown_grace,
    )
}

fn run_command(args: &[String]) -> Result<(), String> {
    let (source, source_url, display_name) = read_script(args, "run")?;
    let vm = JscVm::new(&display_name).map_err(|error| error.to_string())?;
    evaluate_script(&vm, &source, &source_url)
}

fn run_async_command(args: &[String]) -> Result<(), String> {
    let options = parse_async_options(args, "run-async <file>")?;
    let file_args = [options.subject];
    let (source, source_url, display_name) = read_script(&file_args, "run-async")?;
    evaluate_async(
        &source,
        &source_url,
        &display_name,
        options.permissions,
        options.shutdown_grace,
    )
}

fn run_module_command(args: &[String]) -> Result<(), String> {
    let options = parse_async_options(args, "run-module <file>")?;
    let entry = Path::new(&options.subject)
        .canonicalize()
        .map_err(|error| format!("cannot resolve module entry {}: {error}", options.subject))?;
    let root = entry
        .parent()
        .ok_or("module entry has no parent directory")?;
    let sources = ModuleSources::new(root)?;
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    let mut isolate =
        TokioIsolate::new_with_permissions("kunlun-runtime run-module", options.permissions)
            .map_err(|e| e.to_string())?;
    isolate
        .install_module_sources(sources)
        .map_err(|e| e.to_string())?;
    let entry = entry.to_str().ok_or("module entry is not valid UTF-8")?;
    let result = drive_module(&runtime, &mut isolate, entry, options.shutdown_grace);
    report_rejections(isolate.take_promise_rejections());
    result
}

fn evaluate_script(vm: &JscVm, source: &str, source_url: &str) -> Result<(), String> {
    let result = vm.evaluate(source, source_url);
    if JscVm::backend_info().supports_explicit_microtask_checkpoint {
        while vm
            .microtask_checkpoint()
            .map_err(|error| error.to_string())?
        {}
    }
    report_rejections(vm.take_promise_rejections());
    println!("{}", result.map_err(|error| error.to_string())?);
    Ok(())
}

fn report_rejections(records: Vec<PromiseRejection>) {
    for record in records {
        let transition = match record.transition {
            PromiseRejectionTransition::Unhandled => "unhandled Promise rejection",
            PromiseRejectionTransition::Handled => "Promise rejection handled",
        };
        eprintln!(
            "{transition} [isolate {}, rejection {}]: {}",
            record.isolate_id, record.rejection_id, record.exception
        );
    }
}

fn read_script(args: &[String], command: &str) -> Result<(String, String, String), String> {
    let filename = args
        .first()
        .ok_or_else(|| format!("usage: kunlun-runtime {command} <file>"))?;
    let path = Path::new(filename);
    let source = fs::read_to_string(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    Ok((
        source,
        canonical.display().to_string(),
        format!("kunlun-runtime: {}", path.display()),
    ))
}

fn evaluate_async(
    source: &str,
    source_url: &str,
    name: &str,
    permissions: HostPermissions,
    shutdown_grace: Duration,
) -> Result<(), String> {
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("could not create Tokio event loop: {error}"))?;
    let mut isolate =
        TokioIsolate::new_with_permissions(name, permissions).map_err(|error| error.to_string())?;
    let result = drive_async_body(&runtime, &mut isolate, source, source_url, shutdown_grace);
    report_rejections(isolate.take_promise_rejections());
    println!("{}", result?);
    Ok(())
}

#[derive(Clone, Copy)]
enum ShutdownSignal {
    Interrupt,
    Terminate,
}

impl ShutdownSignal {
    fn name(self) -> &'static str {
        match self {
            Self::Interrupt => "SIGINT",
            Self::Terminate => "SIGTERM",
        }
    }
}

#[cfg(unix)]
struct ShutdownSignals {
    interrupt: Signal,
    terminate: Signal,
}

#[cfg(unix)]
impl ShutdownSignals {
    fn new() -> Result<Self, String> {
        Ok(Self {
            interrupt: tokio::signal::unix::signal(SignalKind::interrupt())
                .map_err(|error| format!("could not install SIGINT handler: {error}"))?,
            terminate: tokio::signal::unix::signal(SignalKind::terminate())
                .map_err(|error| format!("could not install SIGTERM handler: {error}"))?,
        })
    }

    async fn recv(&mut self) -> Result<ShutdownSignal, String> {
        tokio::select! {
            signal = self.interrupt.recv() => signal
                .map(|()| ShutdownSignal::Interrupt)
                .ok_or_else(|| "SIGINT handler closed unexpectedly".to_owned()),
            signal = self.terminate.recv() => signal
                .map(|()| ShutdownSignal::Terminate)
                .ok_or_else(|| "SIGTERM handler closed unexpectedly".to_owned()),
        }
    }
}

#[cfg(not(unix))]
struct ShutdownSignals;

#[cfg(not(unix))]
impl ShutdownSignals {
    fn new() -> Result<Self, String> {
        Ok(Self)
    }

    async fn recv(&mut self) -> Result<ShutdownSignal, String> {
        tokio::signal::ctrl_c()
            .await
            .map(|()| ShutdownSignal::Interrupt)
            .map_err(|error| format!("could not wait for Ctrl-C: {error}"))
    }
}

enum Execution<T> {
    Complete(T),
    Shutdown(ShutdownSignal),
}

fn drive_async_body(
    runtime: &tokio::runtime::Runtime,
    isolate: &mut TokioIsolate,
    source: &str,
    source_url: &str,
    grace: Duration,
) -> Result<String, String> {
    let mut signals = runtime.block_on(async { ShutdownSignals::new() })?;
    let shutdown = isolate.shutdown_handle();
    let execution = runtime.block_on(async {
        Ok::<_, String>(tokio::select! {
            result = isolate.evaluate_async_body(source, source_url) => {
                Execution::Complete(result.map_err(|error| error.to_string()))
            }
            signal = signals.recv() => {
                let signal = signal?;
                shutdown.request();
                Execution::Shutdown(signal)
            },
        })
    })?;
    match execution {
        Execution::Complete(result) => result,
        Execution::Shutdown(signal) => {
            finish_shutdown(runtime, isolate, &mut signals, signal, grace)?;
            Err(format!("execution interrupted by {}", signal.name()))
        }
    }
}

fn drive_module(
    runtime: &tokio::runtime::Runtime,
    isolate: &mut TokioIsolate,
    entry: &str,
    grace: Duration,
) -> Result<(), String> {
    let mut signals = runtime.block_on(async { ShutdownSignals::new() })?;
    let shutdown = isolate.shutdown_handle();
    let execution = runtime.block_on(async {
        Ok::<_, String>(tokio::select! {
            result = isolate.evaluate_module(entry) => {
                Execution::Complete(result.map_err(|error| error.to_string()))
            }
            signal = signals.recv() => {
                let signal = signal?;
                shutdown.request();
                Execution::Shutdown(signal)
            },
        })
    })?;
    match execution {
        Execution::Complete(result) => result,
        Execution::Shutdown(signal) => {
            finish_shutdown(runtime, isolate, &mut signals, signal, grace)?;
            Err(format!("execution interrupted by {}", signal.name()))
        }
    }
}

fn finish_shutdown(
    runtime: &tokio::runtime::Runtime,
    isolate: &mut TokioIsolate,
    signals: &mut ShutdownSignals,
    first: ShutdownSignal,
    grace: Duration,
) -> Result<(), String> {
    eprintln!(
        "received {}; stopping admission and draining for up to {} ms",
        first.name(),
        grace.as_millis()
    );
    enum ShutdownWait {
        Finished(Result<ShutdownOutcome, String>),
        Repeated(ShutdownSignal),
    }
    let wait = runtime.block_on(async {
        Ok::<_, String>(tokio::select! {
            biased;
            signal = signals.recv() => ShutdownWait::Repeated(signal?),
            outcome = isolate.shutdown(grace) => {
                ShutdownWait::Finished(outcome.map_err(|error| error.to_string()))
            }
        })
    })?;
    match wait {
        ShutdownWait::Finished(Ok(ShutdownOutcome::Graceful)) => Ok(()),
        ShutdownWait::Finished(Ok(ShutdownOutcome::Forced)) => Err(format!(
            "shutdown grace period of {} ms expired; forcing termination",
            grace.as_millis()
        )),
        ShutdownWait::Finished(Err(error)) => Err(error),
        ShutdownWait::Repeated(signal) => Err(format!(
            "received {} during graceful shutdown; forcing termination",
            signal.name()
        )),
    }
}

struct AsyncCommandOptions {
    subject: String,
    permissions: HostPermissions,
    shutdown_grace: Duration,
}

fn parse_async_options(args: &[String], usage: &str) -> Result<AsyncCommandOptions, String> {
    let mut subject = None;
    let mut permissions = HostPermissions::none();
    let mut shutdown_grace = DEFAULT_SHUTDOWN_GRACE;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--allow-read" => {
                let root = args
                    .get(index + 1)
                    .ok_or_else(|| "--allow-read requires a directory".to_owned())?;
                permissions = permissions
                    .allow_read_root(root)
                    .map_err(|error| format!("invalid --allow-read root {root}: {error}"))?;
                index += 2;
            }
            "--allow-net" => {
                let host = args
                    .get(index + 1)
                    .ok_or_else(|| "--allow-net requires a host name".to_owned())?;
                permissions = permissions.allow_net_host(host);
                index += 2;
            }
            "--shutdown-grace-ms" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| "--shutdown-grace-ms requires milliseconds".to_owned())?;
                let milliseconds = value.parse::<u64>().map_err(|_| {
                    format!("invalid --shutdown-grace-ms value {value}; expected an integer")
                })?;
                shutdown_grace = Duration::from_millis(milliseconds);
                index += 2;
            }
            value if value.starts_with('-') => {
                return Err(format!("unknown async option: {value}"));
            }
            value if subject.is_none() => {
                subject = Some(value.to_owned());
                index += 1;
            }
            value => return Err(format!("unexpected argument: {value}")),
        }
    }

    Ok(AsyncCommandOptions {
        subject: subject.ok_or_else(|| format!("usage: kunlun-runtime {usage}"))?,
        permissions,
        shutdown_grace,
    })
}

fn doctor_command() -> Result<(), String> {
    let backend = JscVm::backend_info();
    println!("runtime: kunlun-runtime {}", env!("CARGO_PKG_VERSION"));
    println!("engine: {}", backend.name);
    println!("backend: {}", backend.backend);
    println!("engine revision: {}", backend.engine_revision);
    println!("target: {}", backend.target);
    println!("distribution mode: {}", backend.distribution_mode);
    println!("distribution: {}", backend.distribution);
    println!("hermetic: {}", backend.hermetic);
    println!("inspection primitive: {}", backend.supports_inspection);
    println!(
        "deferred Promise primitive: {}",
        backend.supports_deferred_promises
    );
    println!("native ESM loader: {}", backend.supports_native_modules);
    println!(
        "explicit microtask checkpoint: {}",
        backend.supports_explicit_microtask_checkpoint
    );
    println!("event loop: {EVENT_LOOP_BACKEND}");
    println!("AbortSignal and bounded host streams: true");
    println!(
        "default shutdown grace: {} ms",
        DEFAULT_SHUTDOWN_GRACE.as_millis()
    );
    println!("built-in modules: kunlun:fs, kunlun:http (capability-gated)");

    let vm = JscVm::new("kunlun-runtime doctor").map_err(|error| error.to_string())?;
    if backend.supports_inspection {
        vm.set_inspectable(true)
            .map_err(|error| error.to_string())?;
        if !vm.is_inspectable().map_err(|error| error.to_string())? {
            return Err("JavaScriptCore did not make the context inspectable".to_owned());
        }
        vm.set_inspectable(false)
            .map_err(|error| error.to_string())?;
    }
    let result = vm
        .evaluate("'jsc-ok'", "kunlun:doctor")
        .map_err(|error| error.to_string())?;
    if result != "jsc-ok" {
        return Err(format!(
            "unexpected JavaScriptCore smoke-test result: {result}"
        ));
    }
    println!("synchronous smoke test: ok");
    let temporal = vm
        .evaluate(
            "typeof Temporal === 'object' && typeof Temporal.PlainDate === 'function'",
            "kunlun:temporal-doctor",
        )
        .map_err(|e| e.to_string())?
        == "true";
    println!("native Temporal API: {temporal}");
    if backend.hermetic && !temporal {
        return Err("pinned JSC must expose Temporal; check JSC_useTemporal overrides".to_owned());
    }
    if temporal {
        let value = vm
            .evaluate(
                "Temporal.PlainDate.from('2024-02-28').add({ days: 1 }).toString()",
                "kunlun:temporal-doctor",
            )
            .map_err(|e| e.to_string())?;
        if value != "2024-02-29" {
            return Err(format!("unexpected Temporal smoke-test result: {value}"));
        }
        println!("Temporal smoke test: ok");
    }

    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("could not create Tokio event loop: {error}"))?;
    let mut isolate =
        TokioIsolate::new("kunlun-runtime async doctor").map_err(|error| error.to_string())?;
    let result = runtime
        .block_on(
            isolate
                .evaluate_async_body("await sleep(1); return 'async-ok';", "kunlun:async-doctor"),
        )
        .map_err(|error| error.to_string())?;
    if result != "async-ok" {
        return Err(format!("unexpected async smoke-test result: {result}"));
    }
    println!("Promise/async/Tokio smoke test: ok");
    Ok(())
}

fn print_help() {
    println!(
        "Kunlun Runtime {version}\n\n\
         Usage: kunlun-runtime <command>\n\n\
         Commands:\n  \
           eval <source>       Evaluate a classic JavaScript script\n  \
           eval-async <body>   Evaluate an async body with sleep and built-ins\n  \
           run <file>          Evaluate a classic JavaScript file\n  \
           run-async <file>    Evaluate a file as an async function body\n  \
           run-module <file>   Evaluate native ESM (bundled JSC only)\n  \
           doctor              Verify JSC, Inspector, Promise, and Tokio integration\n  \
           types               Print TypeScript declarations for built-in modules\n  \
           version             Print the runtime version\n\n\
         Async permissions:\n  \
           --allow-read <dir>  Grant kunlun:fs read access to a directory\n  \
           --allow-net <host>  Grant kunlun:http access to an exact host\n\n\
           --shutdown-grace-ms <ms>  Set the SIGINT/SIGTERM drain deadline\n\n\
         The async bootstrap supports Promise/async/await and Tokio timers.\n\
         run-module supports native ESM and top-level await with bundled JSC.\n\
         The portable remote inspector is not implemented yet.",
        version = env!("CARGO_PKG_VERSION")
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_configurable_shutdown_grace() {
        let options = parse_async_options(
            &[
                "return 'ok'".to_owned(),
                "--shutdown-grace-ms".to_owned(),
                "275".to_owned(),
            ],
            "eval-async <body>",
        )
        .unwrap();
        assert_eq!(options.shutdown_grace, Duration::from_millis(275));
    }

    #[test]
    fn rejects_invalid_shutdown_grace() {
        let error = parse_async_options(
            &[
                "return 'ok'".to_owned(),
                "--shutdown-grace-ms".to_owned(),
                "forever".to_owned(),
            ],
            "eval-async <body>",
        )
        .err()
        .unwrap();
        assert!(error.contains("expected an integer"));
    }
}
