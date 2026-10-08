use std::{
    io::{self, Write},
    path::PathBuf,
};

use kunlun_pm::protocol::{
    DetectResult, Diagnostic, Envelope, ErrorCode, Operation, Outcome, ProviderResult, Result,
    VersionResult,
};

struct Command {
    operation: Operation,
    project: PathBuf,
    package: Option<String>,
    ignore_scripts: bool,
}

fn parse(args: &[String]) -> Result<Command> {
    if args.len() > 128 || args.iter().map(String::len).sum::<usize>() > 128 * 1024 {
        return Err(Diagnostic::new(ErrorCode::InvalidArguments));
    }
    let operation = operation(args);
    if operation == Operation::Unknown {
        return Err(Diagnostic::new(ErrorCode::InvalidArguments));
    }
    let mut project = None;
    let mut package = None;
    let mut ignore_scripts = false;
    let mut json = false;
    let mut frozen = false;
    let mut literal = false;
    let mut index = 1;
    while index < args.len() {
        let arg = &args[index];
        match arg.as_str() {
            "--" if !literal => literal = true,
            "--json" if !literal && !json => json = true,
            "--project" if !literal && project.is_none() && operation != Operation::Version => {
                index += 1;
                let value = args
                    .get(index)
                    .filter(|value| !value.is_empty() && !value.starts_with("--"))
                    .ok_or_else(|| Diagnostic::new(ErrorCode::InvalidArguments))?;
                project = Some(PathBuf::from(value));
            }
            "--ignore-scripts" if !literal && !ignore_scripts && operation == Operation::Plan => {
                ignore_scripts = true
            }
            "--frozen" if !literal && !frozen && operation == Operation::Plan => frozen = true,
            value
                if (literal || !value.starts_with('-'))
                    && operation == Operation::Why
                    && package.is_none() =>
            {
                package = Some(value.into())
            }
            _ => return Err(Diagnostic::new(ErrorCode::InvalidArguments)),
        }
        index += 1;
    }
    if operation == Operation::Why && package.is_none() {
        return Err(Diagnostic::new(ErrorCode::InvalidArguments));
    }
    Ok(Command {
        operation,
        project: project.unwrap_or_else(|| PathBuf::from(".")),
        package,
        ignore_scripts,
    })
}

fn operation(args: &[String]) -> Operation {
    match args.first().map(String::as_str) {
        Some("version") => Operation::Version,
        Some("detect") => Operation::Detect,
        Some("plan") => Operation::Plan,
        Some("why") => Operation::Why,
        _ => Operation::Unknown,
    }
}

fn run(command: Command) -> Result<ProviderResult> {
    if command.operation == Operation::Version {
        return Ok(ProviderResult::Version(VersionResult::default()));
    }
    let plan = kunlun_pm::plan(&command.project, command.ignore_scripts)?;
    match command.operation {
        Operation::Plan => Ok(ProviderResult::Plan(plan)),
        Operation::Detect => Ok(ProviderResult::Detect(DetectResult {
            package_manager: plan.package_manager,
            lockfile_version: "9.0",
            authority: "pnpm-lock.yaml",
            read_only: true,
            workspace_importers: plan
                .importers
                .into_iter()
                .map(|importer| importer.path)
                .collect(),
        })),
        Operation::Why => kunlun_pm::why(&plan, command.package.as_deref().unwrap_or_default())
            .map(ProviderResult::Why),
        _ => Err(Diagnostic::new(ErrorCode::InvalidArguments)),
    }
}

fn main() {
    let raw: Vec<_> = std::env::args_os().skip(1).collect();
    let json = raw.iter().any(|arg| arg == "--json");
    let args: std::result::Result<Vec<_>, _> =
        raw.into_iter().map(|arg| arg.into_string()).collect();
    let (operation, result) = match args {
        Ok(args) => (operation(&args), parse(&args).and_then(run)),
        Err(_) => (
            Operation::Unknown,
            Err(Diagnostic::new(ErrorCode::InvalidArguments)),
        ),
    };
    let envelope = Envelope::new(operation, result);
    let status = if json {
        let (status, bytes) = envelope.encode();
        if io::stdout().lock().write_all(&bytes).is_err() {
            std::process::exit(1);
        }
        status
    } else {
        match &envelope.outcome {
            Outcome::Ok { result } => {
                // Local developer presentation only. Core always uses the machine mode.
                let output = serde_json::to_string_pretty(result).unwrap_or_default();
                if writeln!(io::stdout().lock(), "{output}").is_err() {
                    std::process::exit(1);
                }
            }
            Outcome::Error { diagnostics } => {
                for error in diagnostics {
                    let _ = writeln!(
                        io::stderr().lock(),
                        "{}\n{}",
                        error.message,
                        error.remediation
                    );
                }
            }
        }
        envelope.exit_status
    };
    std::process::exit(status);
}
