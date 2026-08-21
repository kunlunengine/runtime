use kunlun_runtime::JscVm;
use std::env;
use std::fs;
use std::path::Path;
use std::process::ExitCode;

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
        Some("run") => run_command(&args[1..]),
        Some("doctor") => doctor_command(),
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
    let mut vm = JscVm::new("kunlun-runtime eval").map_err(|error| error.to_string())?;
    println!(
        "{}",
        vm.evaluate(source, "kunlun:eval")
            .map_err(|error| error.to_string())?
    );
    Ok(())
}

fn run_command(args: &[String]) -> Result<(), String> {
    let filename = args
        .first()
        .ok_or_else(|| "usage: kunlun-runtime run <file>".to_owned())?;
    let path = Path::new(filename);
    let source = fs::read_to_string(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let source_url = path
        .canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .display()
        .to_string();
    let mut vm = JscVm::new(&format!("kunlun-runtime: {}", path.display()))
        .map_err(|error| error.to_string())?;
    println!(
        "{}",
        vm.evaluate(&source, &source_url)
            .map_err(|error| error.to_string())?
    );
    Ok(())
}

fn doctor_command() -> Result<(), String> {
    let backend = JscVm::backend_info();
    println!("runtime: kunlun-runtime {}", env!("CARGO_PKG_VERSION"));
    println!("engine: {}", backend.name);
    println!("distribution: {}", backend.distribution);
    println!("hermetic: {}", backend.hermetic);
    println!("inspection primitive: {}", backend.supports_inspection);

    let mut vm = JscVm::new("kunlun-runtime doctor").map_err(|error| error.to_string())?;
    if backend.supports_inspection {
        vm.set_inspectable(true);
        if !vm.is_inspectable() {
            return Err("JavaScriptCore did not make the context inspectable".to_owned());
        }
        vm.set_inspectable(false);
    }
    let result = vm
        .evaluate("'jsc-ok'", "kunlun:doctor")
        .map_err(|error| error.to_string())?;
    if result != "jsc-ok" {
        return Err(format!(
            "unexpected JavaScriptCore smoke-test result: {result}"
        ));
    }
    println!("smoke test: ok");
    Ok(())
}

fn print_help() {
    println!(
        "Kunlun Runtime {version}\n\n\
         Usage: kunlun-runtime <command>\n\n\
         Commands:\n  \
           eval <source>  Evaluate a classic JavaScript script\n  \
           run <file>     Evaluate a classic JavaScript file\n  \
           doctor         Report the active JSC backend and run a smoke test\n  \
           version        Print the runtime version\n\n\
         This bootstrap does not yet provide ESM, an event loop, Fetch, or the remote inspector.",
        version = env!("CARGO_PKG_VERSION")
    );
}
