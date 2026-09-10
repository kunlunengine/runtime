use kunlun_jsc::{JscVm, PromiseRejection, PromiseRejectionTransition};
use kunlun_runtime::{
    EVENT_LOOP_BACKEND, HostPermissions, ModuleSources, TYPESCRIPT_DECLARATIONS, TokioIsolate,
};
use std::env;
use std::fs;
use std::path::Path;
use std::process::ExitCode;
use tokio::runtime::Builder;

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
    evaluate_async(&source, &source_url, &display_name, options.permissions)
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
    let result = runtime.block_on(isolate.evaluate_module(entry));
    report_rejections(isolate.take_promise_rejections());
    result.map_err(|e| e.to_string())
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
) -> Result<(), String> {
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("could not create Tokio event loop: {error}"))?;
    let mut isolate =
        TokioIsolate::new_with_permissions(name, permissions).map_err(|error| error.to_string())?;
    let result = runtime.block_on(isolate.evaluate_async_body(source, source_url));
    report_rejections(isolate.take_promise_rejections());
    println!("{}", result.map_err(|error| error.to_string())?);
    Ok(())
}

struct AsyncCommandOptions {
    subject: String,
    permissions: HostPermissions,
}

fn parse_async_options(args: &[String], usage: &str) -> Result<AsyncCommandOptions, String> {
    let mut subject = None;
    let mut permissions = HostPermissions::none();
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
         The async bootstrap supports Promise/async/await and Tokio timers.\n\
         run-module supports native ESM and top-level await with bundled JSC.\n\
         The portable remote inspector is not implemented yet.",
        version = env!("CARGO_PKG_VERSION")
    );
}
