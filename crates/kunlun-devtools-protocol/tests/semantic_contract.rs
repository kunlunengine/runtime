//! MOCK business-contract tests, never evidence of native Inspector or standalone client delivery.
use kunlun_devtools_protocol::*;
use serde::Deserialize;

mod support;
use support::{Mock, identity, request, response, source};

const GRANTS: &[Permission] = &[
    Permission::Inspect,
    Permission::Control,
    Permission::Evaluate,
    Permission::Mutate,
    Permission::CaptureSensitive,
];

fn attached() -> Mock {
    let mut mock = Mock::new();
    let attach = request("setup-attach", 1, Command::Attach);
    let reply = response(&attach, mock.dispatch(&attach, GRANTS));
    correlate(&attach, &reply).unwrap();
    mock
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Suite {
    evidence: String,
    identity: Identity,
    setup: String,
    target: Target,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    name: String,
    grants: Vec<Permission>,
    steps: Vec<Step>,
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Step {
    Command {
        request_id: String,
        epoch: u64,
        command: Command,
        expect: Box<ResponseResult>,
    },
    Queue {
        request_id: String,
        epoch: u64,
        command: Command,
    },
    Completions {
        expect: Vec<Response>,
    },
    Events {
        expect: Vec<Event>,
    },
    Attach {
        request_id: String,
        epoch: u64,
        next_sequence: u64,
        state: ExecutionState,
    },
    Disconnect,
    Replace,
}

#[test]
fn every_portable_session_workflow() {
    let suite: Suite =
        serde_json::from_str(include_str!("../../../fixtures/devtools-v1/workflows.json")).unwrap();
    assert!(suite.evidence.starts_with("MOCK ONLY"));
    assert_eq!(suite.identity, identity(1));
    assert!(suite.setup.contains("setup-attach"));
    let validator = jsonschema::validator_for(&serde_json::to_value(schema()).unwrap()).unwrap();
    for case in suite.cases {
        let mut mock = attached();
        assert_eq!(mock.target(), suite.target);
        for step in case.steps {
            match step {
                Step::Command {
                    request_id,
                    epoch,
                    command,
                    expect,
                } => {
                    let request = request(&request_id, epoch, command);
                    let response = response(&request, mock.dispatch(&request, &case.grants));
                    assert_eq!(response.result, *expect, "{}: {request_id}", case.name);
                    correlate(&request, &response).unwrap();
                    let wire = Wire::Response { response };
                    let value = serde_json::to_value(&wire).unwrap();
                    assert!(validator.is_valid(&value), "{}: {value}", case.name);
                    assert_eq!(decode(&encode(&wire).unwrap()).unwrap(), wire);
                }
                Step::Queue {
                    request_id,
                    epoch,
                    command,
                } => {
                    mock.queue(request(&request_id, epoch, command), &case.grants)
                        .unwrap();
                }
                Step::Completions { expect } => {
                    assert_eq!(
                        std::mem::take(&mut mock.completions),
                        expect,
                        "{}",
                        case.name
                    );
                    for response in expect {
                        let wire = Wire::Response { response };
                        assert!(validator.is_valid(&serde_json::to_value(&wire).unwrap()));
                        assert_eq!(decode(&encode(&wire).unwrap()).unwrap(), wire);
                    }
                }
                Step::Events { expect } => {
                    assert_eq!(std::mem::take(&mut mock.events), expect, "{}", case.name);
                    for event in expect {
                        let wire = Wire::Event { event };
                        assert!(validator.is_valid(&serde_json::to_value(&wire).unwrap()));
                        assert_eq!(decode(&encode(&wire).unwrap()).unwrap(), wire);
                    }
                }
                Step::Attach {
                    request_id,
                    epoch,
                    next_sequence,
                    state,
                } => {
                    let request = request(&request_id, epoch, Command::Attach);
                    let mut target = suite.target.clone();
                    target.epoch = epoch;
                    let result = mock.dispatch(&request, &case.grants).unwrap();
                    assert_eq!(
                        result,
                        CommandResult::Attach {
                            target,
                            state,
                            next_sequence
                        },
                        "{}",
                        case.name
                    );
                    let response = response(&request, Ok(result));
                    correlate(&request, &response).unwrap();
                    assert!(
                        validator
                            .is_valid(&serde_json::to_value(Wire::Response { response }).unwrap())
                    );
                }
                Step::Disconnect => mock.disconnect(),
                Step::Replace => mock.replace(),
            }
        }
        for event in &mock.events {
            encode(&Wire::Event {
                event: event.clone(),
            })
            .unwrap();
        }
        let audit = serde_json::to_string(&mock.audit).unwrap();
        assert!(!audit.contains("fixture-secret-never-audit"));
        assert!(!audit.contains("expression"));
    }
}

#[test]
fn discovery_requires_negotiation_and_inspect_and_reports_backend_truthfully() {
    let mut mock = Mock::new();
    mock.negotiated = false;
    let discover = Wire::Discover {
        version: Version::V1,
        request_id: "d1".into(),
        max_targets: 1,
    };
    assert_eq!(
        mock.exchange(discover.clone(), GRANTS),
        Err(Error::InvalidState)
    );
    assert_eq!(
        mock.exchange(
            Wire::Hello {
                versions: vec!["1.0".into()]
            },
            GRANTS
        ),
        Err(Error::UnknownVersion),
    );
    let welcome = mock
        .exchange(
            Wire::Hello {
                versions: vec!["2".into(), "1".into()],
            },
            GRANTS,
        )
        .unwrap();
    assert!(matches!(
        welcome,
        Wire::Welcome {
            version: Version::V1,
            ..
        }
    ));
    assert_eq!(
        mock.exchange(discover.clone(), &[]),
        Err(Error::PermissionDenied {
            permission: Permission::Inspect
        })
    );
    let discovered = mock.exchange(discover, &[Permission::Inspect]).unwrap();
    let Wire::Discovered {
        request_id,
        targets,
        truncated,
        ..
    } = discovered
    else {
        panic!()
    };
    assert_eq!(request_id, "d1");
    assert!(!truncated);
    assert_eq!(targets, vec![mock.target()]);
    assert_eq!(targets[0].backend, Backend::Mock);
    let unsupported = targets[0]
        .capabilities
        .iter()
        .find(|c| c.operation == Operation::Evaluate)
        .unwrap();
    assert_eq!(
        unsupported.support,
        Support::Unsupported {
            reason: UnsupportedReason::NotImplemented
        }
    );
    let mut duplicate_capabilities = targets[0].capabilities.clone();
    duplicate_capabilities.push(duplicate_capabilities[0].clone());
    assert_eq!(
        encode(&Wire::Welcome {
            version: Version::V1,
            capabilities: duplicate_capabilities
        }),
        Err(Error::Malformed)
    );
}

#[test]
fn responses_must_match_request_operation_identity_revision_and_requested_bounds() {
    let request = request(
        "source",
        1,
        Command::ReadSource {
            source: source(1),
            offset: 0,
            max_bytes: 4,
        },
    );
    let chunk = source_chunk(source(1), "text", 0, 4).unwrap();
    let reply = response(
        &request,
        Ok(CommandResult::ReadSource {
            chunk: chunk.clone(),
        }),
    );
    assert_eq!(correlate(&request, &reply), Ok(()));
    let mut bad = reply.clone();
    bad.identity.session = "foreign".into();
    assert!(correlate(&request, &bad).is_err());
    let mut bad = reply.clone();
    bad.request_id = "another".into();
    assert_eq!(correlate(&request, &bad), Err(Error::InvalidIdentity));
    let mut changed = chunk.clone();
    changed.source.revision = "different".into();
    assert_eq!(
        correlate(
            &request,
            &response(&request, Ok(CommandResult::ReadSource { chunk: changed }))
        ),
        Err(Error::SourceRange)
    );
    assert_eq!(
        correlate(&request, &response(&request, Ok(CommandResult::Continue))),
        Err(Error::Malformed)
    );
    let larger = source_chunk(source(1), "text-too-long", 0, 5).unwrap();
    assert_eq!(
        correlate(
            &request,
            &response(&request, Ok(CommandResult::ReadSource { chunk: larger }))
        ),
        Err(Error::SourceRange)
    );
    let failed = response(&request, Err(Error::SourceNotFound));
    assert_eq!(correlate(&request, &failed), Ok(()));
}

#[test]
fn nullable_fields_are_required_but_null_is_valid_in_schema_and_decoder() {
    let mapping = SourceMapping {
        generated: SourceLocation {
            source: source(1),
            line: 1,
            column: 0,
        },
        map: SourceIdentity {
            id: "map".into(),
            ..source(1)
        },
        original: None,
    };
    let frames = [
        Wire::Failure {
            request_id: None,
            error: Error::UnknownVersion,
        },
        Wire::Response {
            response: response(
                &request(
                    "map",
                    1,
                    Command::LookupSourceMap {
                        location: mapping.generated.clone(),
                    },
                ),
                Ok(CommandResult::LookupSourceMap {
                    mapping: Box::new(mapping),
                }),
            ),
        },
        Wire::Response {
            response: response(
                &request("sources", 1, Command::ListSources { max_entries: 1 }),
                Ok(CommandResult::ListSources {
                    sources: vec![SourceDescriptor {
                        source: source(1),
                        source_map: None,
                    }],
                    truncated: false,
                }),
            ),
        },
        Wire::Event {
            event: Event {
                version: Version::V1,
                identity: identity(1),
                sequence: 1,
                data: EventData::Paused {
                    pause: 1,
                    reason: PauseReason::Backend,
                    frames: vec![StackFrame {
                        name: "anonymous".into(),
                        handle: Handle {
                            identity: identity(1),
                            pause: 1,
                            id: "frame".into(),
                        },
                        location: None,
                    }],
                    truncated: false,
                },
            },
        },
    ];
    let paths: &[&[&str]] = &[
        &["request_id"],
        &["response", "result", "result", "mapping", "original"],
        &["response", "result", "result", "sources", "0", "source_map"],
        &["event", "data", "frames", "0", "location"],
    ];
    let validator = jsonschema::validator_for(&serde_json::to_value(schema()).unwrap()).unwrap();
    for (wire, path) in frames.into_iter().zip(paths) {
        let bytes = encode(&wire).unwrap();
        let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(validator.is_valid(&value), "{value}");
        assert_eq!(decode(&bytes).unwrap(), wire);
        let mut parent = &mut value;
        for key in &path[..path.len() - 1] {
            parent = if let Ok(index) = key.parse::<usize>() {
                &mut parent[index]
            } else {
                &mut parent[*key]
            };
        }
        parent.as_object_mut().unwrap().remove(path[path.len() - 1]);
        assert!(!validator.is_valid(&value), "{value}");
        assert_eq!(
            decode(&serde_json::to_vec(&value).unwrap()),
            Err(Error::Malformed)
        );
    }
}

#[test]
fn breakpoint_and_step_results_have_real_mock_state_and_fresh_scopes() {
    let mut mock = attached();
    let set = request(
        "set",
        1,
        Command::SetBreakpoint {
            source: source(1),
            line: 0,
            column: 13,
        },
    );
    let result = mock.dispatch(&set, GRANTS).unwrap();
    let CommandResult::SetBreakpoint { breakpoint } = &result else {
        panic!()
    };
    assert_eq!(mock.breakpoints.get(&breakpoint.id), Some(breakpoint));
    correlate(&set, &response(&set, Ok(result.clone()))).unwrap();
    mock.dispatch(&request("pause", 1, Command::Pause), GRANTS)
        .unwrap();
    let old = Handle {
        identity: identity(1),
        pause: 1,
        id: "scope-1".into(),
    };
    mock.dispatch(&request("step", 1, Command::Step), GRANTS)
        .unwrap();
    assert_eq!(mock.paused, Some(2));
    assert_eq!(mock.events.len(), 3);
    assert_eq!(
        mock.dispatch(
            &request("stale", 1, Command::ReadScopes { handle: old }),
            GRANTS
        ),
        Err(Error::StaleHandle)
    );
    let handle = Handle {
        identity: identity(1),
        pause: 2,
        id: "scope-1".into(),
    };
    let scopes = mock
        .dispatch(&request("fresh", 1, Command::ReadScopes { handle }), GRANTS)
        .unwrap();
    let CommandResult::ReadScopes { scopes, .. } = scopes else {
        panic!()
    };
    assert_eq!(
        scopes[0].variables[0].value,
        RemoteValue::Number { value: "1".into() }
    );
    assert_eq!(scopes[0].variables[1].value, RemoteValue::Redacted);
    mock.dispatch(
        &request(
            "remove",
            1,
            Command::RemoveBreakpoint {
                breakpoint_id: breakpoint.id.clone(),
            },
        ),
        GRANTS,
    )
    .unwrap();
    assert!(mock.breakpoints.is_empty());
    mock.disconnect();
    mock.dispatch(&request("reattach", 1, Command::Attach), GRANTS)
        .unwrap();
    mock.dispatch(&request("repause", 1, Command::Pause), GRANTS)
        .unwrap();
    assert_eq!(mock.paused, Some(3)); // Same-epoch reconnect never reuses a pause generation.
}

#[test]
fn cancellation_is_correlated_non_replaying_and_cannot_escalate_authority() {
    let mut mock = attached();
    let pending = request("pending", 1, Command::Pause);
    mock.queue(pending.clone(), GRANTS).unwrap();
    let denied = mock.dispatch(
        &request(
            "denied-cancel",
            1,
            Command::Cancel {
                request_id: "pending".into(),
            },
        ),
        &[Permission::Inspect],
    );
    assert_eq!(
        denied,
        Err(Error::PermissionDenied {
            permission: Permission::Control
        })
    );
    assert_eq!(mock.pending_count(), 1);
    let cancel = request(
        "cancel",
        1,
        Command::Cancel {
            request_id: "pending".into(),
        },
    );
    let result = mock.dispatch(&cancel, GRANTS).unwrap();
    assert_eq!(
        result,
        CommandResult::Cancel {
            request_id: "pending".into(),
            status: CancellationStatus::Cancelled
        }
    );
    correlate(&cancel, &response(&cancel, Ok(result))).unwrap();
    let completion = mock.completions.pop().unwrap();
    correlate(&pending, &completion).unwrap();
    assert_eq!(
        completion.result,
        ResponseResult::Failure {
            error: Error::Cancelled
        }
    );
    assert_eq!(
        mock.complete("pending", GRANTS),
        Err(Error::RequestNotFound)
    );
    assert_eq!(
        mock.dispatch(&pending, GRANTS),
        Err(Error::DuplicateRequest)
    );
    assert!(mock.paused.is_none());
    assert_eq!(
        mock.dispatch(
            &request(
                "late-cancel",
                1,
                Command::Cancel {
                    request_id: "pending".into()
                }
            ),
            GRANTS
        ),
        Ok(CommandResult::Cancel {
            request_id: "pending".into(),
            status: CancellationStatus::TooLate
        })
    );
    let queued_eval = request(
        "queued-eval",
        1,
        Command::Evaluate {
            expression: "must-not-replay".into(),
        },
    );
    mock.queue(queued_eval.clone(), GRANTS).unwrap();
    mock.disconnect();
    assert_eq!(mock.pending_count(), 0);
    mock.dispatch(&request("reattach", 1, Command::Attach), GRANTS)
        .unwrap();
    assert_eq!(
        mock.dispatch(&queued_eval, GRANTS),
        Err(Error::DuplicateRequest)
    );
    assert_eq!(
        mock.complete("queued-eval", GRANTS),
        Err(Error::RequestNotFound)
    );
}

#[test]
fn audit_delivery_failure_fails_closed_and_queued_work_revalidates_grants() {
    let mut mock = attached();
    let pause = request("pause", 1, Command::Pause);
    mock.queue(pause.clone(), GRANTS).unwrap();
    let completion = mock.complete("pause", &[Permission::Inspect]).unwrap();
    assert_eq!(
        completion.result,
        ResponseResult::Failure {
            error: Error::PermissionDenied {
                permission: Permission::Control
            }
        }
    );
    assert!(mock.paused.is_none());
    assert_eq!(mock.audit.last().unwrap().outcome, Outcome::Denied);
    mock.audit_available = false;
    assert_eq!(
        mock.dispatch(&request("no-audit", 1, Command::Pause), GRANTS),
        Err(Error::AuditUnavailable)
    );
    assert!(mock.terminal);
    assert!(mock.paused.is_none());
    assert!(mock.events.is_empty());
    let mut target = serde_json::to_value(mock.target()).unwrap();
    for field in ["secrets", "host_grants", "raw_headers", "environment"] {
        target[field] = serde_json::json!({"fixture": "never-export"});
        assert!(serde_json::from_value::<Target>(target.clone()).is_err());
        target.as_object_mut().unwrap().remove(field);
    }
}

#[test]
fn pending_and_tombstone_limits_do_not_evict_or_reenable_commands() {
    let mut mock = attached();
    for n in 0..MAX_PENDING_REQUESTS {
        mock.queue(
            request(
                &format!("pending-{n}"),
                1,
                Command::ReadLogs { max_entries: 1 },
            ),
            GRANTS,
        )
        .unwrap();
    }
    let excess = request("pending-overflow", 1, Command::ReadLogs { max_entries: 1 });
    assert_eq!(
        mock.queue(excess.clone(), GRANTS),
        Err(Error::LimitExceeded {
            resource: Resource::PendingRequests
        })
    );
    assert_eq!(mock.pending_count(), MAX_PENDING_REQUESTS);
    assert_eq!(mock.dispatch(&excess, GRANTS), Err(Error::DuplicateRequest));
    mock.disconnect();
    assert_eq!(mock.pending_count(), 0);
    assert!(mock.events.is_empty());
    let mut mock = attached();
    for n in 1..MAX_REQUEST_HISTORY {
        mock.dispatch(
            &request(&format!("r-{n}"), 1, Command::ReadLogs { max_entries: 1 }),
            GRANTS,
        )
        .unwrap();
    }
    assert_eq!(mock.seen.len(), MAX_REQUEST_HISTORY);
    assert_eq!(
        mock.dispatch(
            &request("history-overflow", 1, Command::ReadLogs { max_entries: 1 }),
            GRANTS
        ),
        Err(Error::LimitExceeded {
            resource: Resource::RequestHistory
        })
    );
    assert!(mock.terminal);
    assert_eq!(mock.seen.len(), MAX_REQUEST_HISTORY);
    assert_eq!(
        mock.dispatch(&request("setup-attach", 1, Command::Attach), GRANTS),
        Err(Error::Disconnected)
    );
}

#[test]
fn snapshot_collection_value_and_escaped_frame_bounds_apply_in_both_directions() {
    for command in [
        Command::ListSources { max_entries: 0 },
        Command::ReadLogs {
            max_entries: MAX_ENTRIES + 1,
        },
        Command::CaptureCpu {
            max_bytes: MAX_CAPTURE_BYTES + 1,
        },
        Command::CaptureHeap { max_bytes: 0 },
    ] {
        assert!(
            encode(&Wire::Request {
                request: request("bad", 1, command)
            })
            .is_err()
        );
    }
    let capture = request("cpu", 1, Command::CaptureCpu { max_bytes: 4 });
    let snapshot = DiagnosticSnapshot {
        format: DiagnosticFormat::Text,
        text: "12345".into(),
        truncated: true,
    };
    assert_eq!(
        correlate(
            &capture,
            &response(&capture, Ok(CommandResult::CaptureCpu { snapshot }))
        ),
        Err(Error::LimitExceeded {
            resource: Resource::Snapshot
        })
    );
    let snapshot = DiagnosticSnapshot {
        format: DiagnosticFormat::Text,
        text: "x".repeat(MAX_CAPTURE_BYTES as usize + 1),
        truncated: false,
    };
    assert!(
        encode(&Wire::Response {
            response: response(&capture, Ok(CommandResult::CaptureCpu { snapshot }))
        })
        .is_err()
    );
    let snapshot = DiagnosticSnapshot {
        format: DiagnosticFormat::Text,
        text: "\0".repeat(MAX_CAPTURE_BYTES as usize),
        truncated: true,
    };
    assert_eq!(
        encode(&Wire::Response {
            response: response(&capture, Ok(CommandResult::CaptureCpu { snapshot }))
        }),
        Err(Error::MessageTooLarge)
    );
    let snapshot = DiagnosticSnapshot {
        format: DiagnosticFormat::Json,
        text: "{\"samples\":0}".into(),
        truncated: false,
    };
    let frame = Wire::Response {
        response: response(
            &request("schema-only", 1, Command::CaptureCpu { max_bytes: 1024 }),
            Ok(CommandResult::CaptureCpu { snapshot }),
        ),
    };
    let validator = jsonschema::validator_for(&serde_json::to_value(schema()).unwrap()).unwrap();
    assert!(validator.is_valid(&serde_json::to_value(&frame).unwrap()));
    assert_eq!(decode(&encode(&frame).unwrap()).unwrap(), frame); // Vocabulary only, not a capture.
    let large_value = CommandResult::Evaluate {
        value: RemoteValue::String {
            text: "x".repeat(MAX_VALUE_BYTES + 1),
            truncated: false,
        },
    };
    assert!(
        encode(&Wire::Response {
            response: response(
                &request(
                    "eval",
                    1,
                    Command::Evaluate {
                        expression: "x".into()
                    }
                ),
                Ok(large_value)
            )
        })
        .is_err()
    );
    let logs = CommandResult::ReadLogs {
        entries: vec![
            LogEntry {
                sequence: 1,
                level: LogLevel::Info,
                message: RemoteValue::Redacted
            };
            MAX_ENTRIES as usize + 1
        ],
        truncated: true,
    };
    assert!(
        encode(&Wire::Response {
            response: response(
                &request("logs", 1, Command::ReadLogs { max_entries: 1 }),
                Ok(logs)
            )
        })
        .is_err()
    );
}
