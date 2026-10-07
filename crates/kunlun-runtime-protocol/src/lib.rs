//! The process boundary consumed by Core's `kunlun` CLI, not a second workflow CLI.
use serde::{Deserialize, Serialize};

pub const PROVIDER_SCHEMA: &str = "kunlun.runtime-provider/v0.2";
pub const MAX_RESPONSE_BYTES: usize = 16 * 1024;
const MAX_ARGUMENT_BYTES: usize = 128 * 1024;
const MAX_ARGUMENTS: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Command {
    Version,
    Doctor,
    CheckArtifact,
}

impl Command {
    pub fn recognize(value: &str) -> Option<Self> {
        match value {
            "version" | "--version" | "-V" => Some(Self::Version),
            "doctor" => Some(Self::Doctor),
            "check-artifact" => Some(Self::CheckArtifact),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub command: Command,
    pub json: bool,
    pub artifact: Option<ArtifactOptions>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactOptions {
    pub directory: String,
    pub manifest_sha256: String,
    pub read_bindings: Vec<(String, String)>,
    pub net_hosts: Vec<String>,
}

/// Parses provider operations without opening files, granting authority, or loading JSC.
/// Unknown workflow/developer commands remain the responsibility of their own dispatcher.
pub fn parse(command: Command, args: &[String]) -> Result<Request, &'static str> {
    if args.len() > MAX_ARGUMENTS
        || args.iter().map(String::len).sum::<usize>() > MAX_ARGUMENT_BYTES
    {
        return Err("provider arguments exceed the configured limit");
    }
    let mut json = false;
    let mut directory = None;
    let mut digest = None;
    let mut read_bindings = Vec::new();
    let mut net_hosts = Vec::new();
    let mut positional = false;
    let mut index = 0;
    while index < args.len() {
        let argument = args[index].as_str();
        match argument {
            "--json" if !positional && !json => json = true,
            "--" if !positional && command == Command::CheckArtifact => positional = true,
            "--manifest-sha256" | "--bind-read" | "--allow-net"
                if !positional && command == Command::CheckArtifact =>
            {
                let value = args
                    .get(index + 1)
                    .ok_or("provider option requires a value")?;
                if value.is_empty() || value.starts_with('-') {
                    return Err("provider option requires a non-empty value");
                }
                match argument {
                    "--manifest-sha256" => {
                        if digest.replace(value.clone()).is_some() {
                            return Err("--manifest-sha256 must occur exactly once");
                        }
                    }
                    "--bind-read" => {
                        let root = args
                            .get(index + 2)
                            .ok_or("--bind-read requires a binding name and directory")?;
                        if root.is_empty() || root.starts_with('-') {
                            return Err("--bind-read requires a non-empty directory");
                        }
                        if !value
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
                        {
                            return Err("--bind-read requires a valid binding name");
                        }
                        if read_bindings.iter().any(|(name, _)| name == value) {
                            return Err("--bind-read binding names must be unique");
                        }
                        read_bindings.push((value.clone(), root.clone()));
                        index += 1;
                    }
                    "--allow-net" => net_hosts.push(value.clone()),
                    _ => unreachable!(),
                }
                index += 1;
            }
            value
                if command == Command::CheckArtifact && (positional || !value.starts_with('-')) =>
            {
                if value.is_empty() || directory.replace(value.to_owned()).is_some() {
                    return Err("check-artifact requires exactly one artifact directory");
                }
            }
            _ => return Err("unknown, duplicate, or unexpected provider argument"),
        }
        index += 1;
    }
    let artifact = if command == Command::CheckArtifact {
        let manifest_sha256 = digest.ok_or("check-artifact requires --manifest-sha256")?;
        if manifest_sha256.len() != 64
            || !manifest_sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err("--manifest-sha256 requires 64 lowercase hexadecimal characters");
        }
        Some(ArtifactOptions {
            directory: directory.ok_or("check-artifact requires an artifact directory")?,
            manifest_sha256,
            read_bindings,
            net_hosts,
        })
    } else {
        None
    };
    Ok(Request {
        command,
        json,
        artifact,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidArguments,
    AdmissionRejected,
    DiagnosticFailed,
    Internal,
}

impl ErrorCode {
    /// Process exits distinguish caller usage (2) from execution/admission failure (1).
    pub fn exit_code(self) -> u8 {
        match self {
            Self::InvalidArguments => 2,
            _ => 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderError {
    pub code: ErrorCode,
    pub message: String,
    pub remediation: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub admission_kind: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Outcome<T> {
    Ok { result: T },
    Error { diagnostics: Vec<ProviderError> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Response<T> {
    pub schema: String,
    pub operation: Command,
    #[serde(flatten)]
    pub outcome: Outcome<T>,
}

impl<T> Response<T> {
    pub fn success(command: Command, result: T) -> Self {
        Self {
            schema: PROVIDER_SCHEMA.to_owned(),
            operation: command,
            outcome: Outcome::Ok { result },
        }
    }

    pub fn failure(command: Command, error: ProviderError) -> Self {
        Self {
            schema: PROVIDER_SCHEMA.to_owned(),
            operation: command,
            outcome: Outcome::Error {
                diagnostics: vec![error],
            },
        }
    }
}

/// One JSON object plus newline on stdout. No partial oversized response is written.
pub fn encode<T: Serialize>(response: &Response<T>) -> Result<String, &'static str> {
    let mut json =
        serde_json::to_string(response).map_err(|_| "cannot encode provider response")?;
    if json.len() + 1 > MAX_RESPONSE_BYTES {
        return Err("provider response exceeds the configured limit");
    }
    json.push('\n');
    Ok(json)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn metadata_commands_are_strict() {
        for command in [Command::Doctor, Command::Version] {
            assert!(!parse(command, &[]).unwrap().json);
            assert!(parse(command, &args(&["--json"])).unwrap().json);
            for invalid in [
                &["extra"][..],
                &["--json", "--json"],
                &["--allow-net", "host"],
            ] {
                assert!(parse(command, &args(invalid)).is_err());
            }
        }
    }

    #[test]
    fn admission_requires_a_trusted_pin_and_explicit_grants() {
        let digest = "a".repeat(64);
        let request = parse(
            Command::CheckArtifact,
            &args(&[
                "artifact",
                "--json",
                "--manifest-sha256",
                &digest,
                "--bind-read",
                "public-data",
                "data",
                "--allow-net",
                "example.com",
            ]),
        )
        .unwrap();
        let artifact = request.artifact.unwrap();
        assert!(request.json);
        assert_eq!(artifact.directory, "artifact");
        assert_eq!(
            artifact.read_bindings,
            [("public-data".into(), "data".into())]
        );
        assert_eq!(artifact.net_hosts, ["example.com"]);
        for invalid in [
            args(&["artifact"]),
            args(&["artifact", "--manifest-sha256", "invalid"]),
            args(&["artifact", "second", "--manifest-sha256", &digest]),
            args(&[
                "artifact",
                "--manifest-sha256",
                &digest,
                "--manifest-sha256",
                &digest,
            ]),
            args(&[
                "artifact",
                "--manifest-sha256",
                &digest,
                "--allow-net",
                "--json",
            ]),
        ] {
            assert!(parse(Command::CheckArtifact, &invalid).is_err());
        }
    }

    #[test]
    fn artifact_bindings_are_named_and_cannot_be_implicitly_granted() {
        let digest = "a".repeat(64);
        for binding_args in [
            vec!["--allow-read", "data"],
            vec!["--bind-read", "public-data"],
            vec!["--bind-read", "invalid/name", "data"],
            vec!["--bind-read", "public-data", "--json"],
            vec![
                "--bind-read",
                "public-data",
                "data",
                "--bind-read",
                "public-data",
                "other",
            ],
        ] {
            let mut arguments = args(&["artifact", "--manifest-sha256", &digest]);
            arguments.extend(args(&binding_args));
            assert!(parse(Command::CheckArtifact, &arguments).is_err());
        }
    }

    #[test]
    fn literal_directories_and_limits_are_explicit() {
        let digest = "0".repeat(64);
        let request = parse(
            Command::CheckArtifact,
            &args(&["--manifest-sha256", &digest, "--", "-directory"]),
        )
        .unwrap();
        assert_eq!(request.artifact.unwrap().directory, "-directory");
        assert!(parse(Command::Version, &vec!["--json".into(); MAX_ARGUMENTS + 1]).is_err());
        assert!(parse(Command::Version, &["x".repeat(MAX_ARGUMENT_BYTES + 1)]).is_err());
    }

    #[test]
    fn replies_round_trip_and_are_bounded() {
        let response = Response::success(Command::Version, "0.1.0");
        let wire = encode(&response).unwrap();
        assert_eq!(
            serde_json::from_str::<Response<String>>(&wire).unwrap(),
            Response::success(Command::Version, "0.1.0".to_owned())
        );
        assert!(wire.ends_with('\n'));
        assert_eq!(wire.lines().count(), 1);
        assert!(
            encode(&Response::success(
                Command::Doctor,
                "x".repeat(MAX_RESPONSE_BYTES)
            ))
            .is_err()
        );
        let response = Response::<()>::failure(
            Command::CheckArtifact,
            ProviderError {
                code: ErrorCode::AdmissionRejected,
                message: "artifact admission rejected".into(),
                remediation: "verify trusted metadata and the artifact contents".into(),
                admission_kind: Some("integrity".into()),
            },
        );
        let wire = encode(&response).unwrap();
        assert_eq!(
            serde_json::from_str::<Response<()>>(&wire).unwrap(),
            response
        );
        assert_eq!(ErrorCode::InvalidArguments.exit_code(), 2);
        assert_eq!(ErrorCode::AdmissionRejected.exit_code(), 1);
    }
}
