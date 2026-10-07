//! MOCK ONLY regression evidence for correlation and delegated cancellation authority.
use kunlun_devtools_protocol::*;

mod support;
use support::{Mock, identity, request, response, source};

#[test]
fn source_map_correlation_rejects_changed_generated_location_and_revision() {
    let location = SourceLocation {
        source: source(1),
        line: 0,
        column: 17,
    };
    let request = request(
        "lookup",
        1,
        Command::LookupSourceMap {
            location: location.clone(),
        },
    );
    let mapping = SourceMapping {
        generated: location,
        map: SourceIdentity {
            id: "map".into(),
            ..source(1)
        },
        original: None,
    };
    let reply = |mapping| {
        response(
            &request,
            Ok(CommandResult::LookupSourceMap {
                mapping: Box::new(mapping),
            }),
        )
    };
    assert_eq!(correlate(&request, &reply(mapping.clone())), Ok(()));
    for mutation in 0..3 {
        let mut changed = mapping.clone();
        match mutation {
            0 => changed.generated.line += 1,
            1 => changed.generated.column += 1,
            _ => changed.generated.source.revision = "revision-2".into(),
        }
        assert_eq!(
            correlate(&request, &reply(changed)),
            Err(Error::SourceNotFound)
        );
    }
}

#[test]
fn cancellation_audits_each_permission_check_truthfully_and_keeps_original_tombstone() {
    let grants = [Permission::Inspect, Permission::Control];
    let mut mock = Mock::new();
    mock.dispatch(&request("attach", 1, Command::Attach), &grants)
        .unwrap();
    mock.queue(request("pending", 1, Command::Pause), &grants)
        .unwrap();
    // A duplicate request must not overwrite the original command's permission metadata.
    assert_eq!(
        mock.dispatch(
            &request("pending", 1, Command::ReadLogs { max_entries: 1 }),
            &grants
        ),
        Err(Error::DuplicateRequest)
    );
    let denied = request(
        "denied",
        1,
        Command::Cancel {
            request_id: "pending".into(),
        },
    );
    assert_eq!(
        mock.dispatch(&denied, &[Permission::Inspect]),
        Err(Error::PermissionDenied {
            permission: Permission::Control
        })
    );
    let checks: Vec<_> = mock
        .audit
        .iter()
        .filter(|audit| audit.request_id == "denied")
        .collect();
    assert_eq!(checks.len(), 2);
    assert_eq!(checks[0].check, AuditCheck::Operation);
    assert_eq!(checks[0].permission, Permission::Inspect);
    assert_eq!(checks[0].outcome, Outcome::Allowed);
    assert_eq!(
        checks[1].check,
        AuditCheck::Cancellation {
            request_id: "pending".into(),
            operation: Operation::Pause
        }
    );
    assert_eq!(checks[1].permission, Permission::Control);
    assert_eq!(checks[1].outcome, Outcome::Denied);
    let mut dishonest = checks[1].clone();
    dishonest.permission = Permission::Inspect;
    let frame = |audit: Audit| Wire::Event {
        event: Event {
            version: Version::V1,
            identity: identity(1),
            sequence: 1,
            data: EventData::Audit { audit },
        },
    };
    assert_eq!(encode(&frame(dishonest)), Err(Error::Malformed));
    for audit in &mock.audit {
        encode(&frame(audit.clone())).unwrap();
    }
    mock.dispatch(
        &request(
            "allowed",
            1,
            Command::Cancel {
                request_id: "pending".into(),
            },
        ),
        &grants,
    )
    .unwrap();
    assert_eq!(
        mock.dispatch(
            &request(
                "late-denied",
                1,
                Command::Cancel {
                    request_id: "pending".into()
                }
            ),
            &[Permission::Inspect]
        ),
        Err(Error::PermissionDenied {
            permission: Permission::Control
        })
    );
    assert!(mock.paused.is_none());
    assert_eq!(
        mock.dispatch(
            &request(
                "recursive",
                1,
                Command::Cancel {
                    request_id: "allowed".into()
                }
            ),
            &grants
        ),
        Err(Error::InvalidState)
    );
}

#[test]
fn discovery_correlation_rejects_wrong_ids_and_excess_targets() {
    let request = Wire::Discover {
        version: Version::V1,
        request_id: "discover".into(),
        max_targets: 1,
    };
    let mock = Mock::new();
    let good = Wire::Discovered {
        version: Version::V1,
        request_id: "discover".into(),
        targets: vec![mock.target()],
        truncated: false,
    };
    assert_eq!(correlate_discovery(&request, &good), Ok(()));
    let wrong_id = Wire::Discovered {
        version: Version::V1,
        request_id: "other".into(),
        targets: vec![],
        truncated: false,
    };
    assert_eq!(
        correlate_discovery(&request, &wrong_id),
        Err(Error::InvalidIdentity)
    );
    let mut foreign = mock.target();
    foreign.id = "another-runtime".into();
    let excess = Wire::Discovered {
        version: Version::V1,
        request_id: "discover".into(),
        targets: vec![mock.target(), foreign],
        truncated: false,
    };
    assert!(encode(&excess).is_ok()); // Individually valid, but not for this request's budget.
    assert_eq!(
        correlate_discovery(&request, &excess),
        Err(Error::LimitExceeded {
            resource: Resource::Collection
        })
    );
    let failure = Wire::Failure {
        request_id: Some("discover".into()),
        error: Error::PermissionDenied {
            permission: Permission::Inspect,
        },
    };
    assert_eq!(correlate_discovery(&request, &failure), Ok(()));
}

#[test]
fn failure_operation_and_permission_are_part_of_response_correlation() {
    let logs = request("logs", 1, Command::ReadLogs { max_entries: 1 });
    for error in [
        Error::Unsupported {
            operation: Operation::Evaluate,
            reason: UnsupportedReason::ProtocolSpecific,
        },
        Error::PermissionDenied {
            permission: Permission::CaptureSensitive,
        },
    ] {
        assert_eq!(
            correlate(&logs, &response(&logs, Err(error))),
            Err(Error::Malformed)
        );
    }
    assert_eq!(
        correlate(
            &logs,
            &response(
                &logs,
                Err(Error::Unsupported {
                    operation: Operation::ReadLogs,
                    reason: UnsupportedReason::BackendUnavailable
                })
            )
        ),
        Ok(())
    );
    let mut invalid = serde_json::to_value(Wire::Request { request: logs }).unwrap();
    invalid["request"]["command"] = serde_json::json!({"op":"discover"});
    assert_eq!(
        decode(&serde_json::to_vec(&invalid).unwrap()),
        Err(Error::UnknownOperation)
    );
}

#[test]
fn exact_serialized_frame_limit_is_enforced_and_escaping_counts() {
    let empty = Wire::Request {
        request: request(
            "eval",
            1,
            Command::Evaluate {
                expression: String::new(),
            },
        ),
    };
    let overhead = encode(&empty).unwrap().len();
    let frame = |length| Wire::Request {
        request: request(
            "eval",
            1,
            Command::Evaluate {
                expression: "x".repeat(length),
            },
        ),
    };
    assert_eq!(
        encode(&frame(MAX_MESSAGE_BYTES - overhead)).unwrap().len(),
        MAX_MESSAGE_BYTES
    );
    assert_eq!(
        encode(&frame(MAX_MESSAGE_BYTES - overhead + 1)),
        Err(Error::MessageTooLarge)
    );
    let escaped = Wire::Request {
        request: request(
            "eval",
            1,
            Command::Evaluate {
                expression: "\n".repeat(MAX_MESSAGE_BYTES / 2),
            },
        ),
    };
    assert_eq!(encode(&escaped), Err(Error::MessageTooLarge));
}

#[test]
fn opaque_breakpoint_ids_stay_bounded_and_pause_generations_never_wrap() {
    let grants = [Permission::Inspect, Permission::Control];
    let mut mock = Mock::new();
    mock.dispatch(&request("attach", 1, Command::Attach), &grants)
        .unwrap();
    let set = request(
        &"x".repeat(128),
        1,
        Command::SetBreakpoint {
            source: source(1),
            line: 0,
            column: 0,
        },
    );
    let result = mock.dispatch(&set, &grants);
    assert_eq!(correlate(&set, &response(&set, result)), Ok(()));
    mock.pause_counter = MAX_SAFE_INTEGER;
    assert_eq!(
        mock.dispatch(&request("pause", 1, Command::Pause), &grants),
        Err(Error::StaleHandle)
    );
    assert!(mock.terminal);
    assert_eq!(mock.pause_counter, MAX_SAFE_INTEGER);
}
