//! MOCK business-contract evidence only. This is not a native adapter or a session service.
use std::collections::BTreeSet;

use kunlun_devtools_protocol::*;

fn identity(epoch: u64) -> Identity {
    Identity {
        target: "mock-runtime".into(),
        session: "session-1".into(),
        epoch,
    }
}

fn source(epoch: u64) -> SourceIdentity {
    SourceIdentity {
        target: "mock-runtime".into(),
        epoch,
        id: "module-1".into(),
        url: "kunlun:fixture/module".into(),
        revision: "revision-1".into(),
    }
}

fn request(id: &str, epoch: u64, command: Command) -> Request {
    Request {
        version: Version::V1,
        request_id: id.into(),
        identity: identity(epoch),
        command,
    }
}

#[derive(Default)]
struct Mock {
    epoch: u64,
    connected: bool,
    paused: Option<u64>,
    pause_counter: u64,
    seen: BTreeSet<String>,
    audit: Vec<Audit>,
}

impl Mock {
    fn new() -> Self {
        Self {
            epoch: 1,
            ..Self::default()
        }
    }

    fn dispatch(&mut self, request: &Request, grants: &[Permission]) -> Result<(), Error> {
        validate_identity(&request.identity)?;
        if request.identity != identity(self.epoch) {
            return Err(Error::StaleEpoch);
        }
        if !self.seen.insert(request.request_id.clone()) {
            return Err(Error::DuplicateRequest);
        }
        let operation = request.command.operation();
        let permission = operation.permission();
        let authorization = authorize(operation, grants);
        // Audit is retained only within this bounded fixture run.
        self.audit.push(Audit {
            identity: request.identity.clone(),
            request_id: request.request_id.clone(),
            operation,
            permission,
            outcome: if authorization.is_ok() {
                Outcome::Allowed
            } else {
                Outcome::Denied
            },
        });
        authorization?;
        if !self.connected && !matches!(request.command, Command::Attach) {
            self.audit.last_mut().unwrap().outcome = Outcome::Failed;
            return Err(Error::Disconnected);
        }
        let result = match &request.command {
            Command::Attach if !self.connected => {
                self.connected = true;
                Ok(())
            }
            Command::Detach => {
                self.connected = false;
                self.paused = None;
                Ok(())
            }
            Command::Pause if self.paused.is_none() => {
                self.pause_counter += 1;
                self.paused = Some(self.pause_counter);
                Ok(())
            }
            Command::Continue if self.paused.is_some() => {
                self.paused = None;
                Ok(())
            }
            Command::ReadScopes { handle } => {
                if handle.identity != request.identity
                    || Some(handle.pause) != self.paused
                    || handle.id != "scope-1"
                {
                    Err(Error::StaleHandle)
                } else {
                    Ok(())
                }
            }
            Command::ReadSource {
                source: requested,
                offset,
                max_bytes,
            } => {
                if requested != &source(self.epoch) {
                    Err(Error::SourceNotFound)
                } else {
                    source_chunk(
                        requested.clone(),
                        include_str!("../../../fixtures/devtools-v1/module.js"),
                        *offset,
                        *max_bytes,
                    )
                    .map(|_| ())
                }
            }
            Command::Attach | Command::Pause | Command::Continue => Err(Error::InvalidState),
            // Never fake evaluation, stepping, capture, logs, or mutation success.
            _ => Err(Error::Unsupported {
                operation,
                reason: UnsupportedReason::NotImplemented,
            }),
        };
        if result.is_err() {
            self.audit.last_mut().unwrap().outcome = Outcome::Failed;
        }
        result
    }

    fn replace(&mut self) {
        self.epoch += 1;
        self.connected = false;
        self.paused = None;
        // Do not retain an executable queue or replay old commands.
        self.seen.clear();
    }

    fn disconnect(&mut self) {
        self.connected = false;
        self.paused = None;
        // Request IDs remain tombstoned throughout this epoch.
    }
}

/// Test-only receive model. A gap/overflow terminates rather than silently dropping events.
#[derive(Default)]
struct Receiver {
    last: u64,
    queue: Vec<Event>,
    terminal: bool,
}

impl Receiver {
    fn receive(&mut self, event: Event) -> Result<(), Error> {
        if self.terminal {
            return Err(Error::Disconnected);
        }
        if event.identity != identity(1) || event.sequence != self.last + 1 {
            self.terminal = true;
            self.queue.clear();
            return Err(Error::EventOrder);
        }
        if self.queue.len() == MAX_EVENTS {
            self.terminal = true;
            self.queue.clear();
            return Err(Error::EventOverflow);
        }
        self.last = event.sequence;
        self.queue.push(event);
        Ok(())
    }
}

fn event(sequence: u64) -> Event {
    Event {
        version: Version::V1,
        identity: identity(1),
        sequence,
        data: EventData::Resumed,
    }
}

#[test]
fn json_schema_and_rust_cross_check_every_fixture() {
    let schema = serde_json::to_value(schema()).unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    let fixtures: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("../../../fixtures/devtools-v1/wire.json")).unwrap();
    for fixture in fixtures {
        assert!(validator.is_valid(&fixture), "{fixture}");
        let wire = decode(&serde_json::to_vec(&fixture).unwrap()).unwrap();
        let encoded = encode(&wire).unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&encoded).unwrap(),
            fixture
        );
        if let Wire::Source { chunk } = wire {
            assert_eq!(
                chunk,
                source_chunk(
                    source(1),
                    include_str!("../../../fixtures/devtools-v1/module.js"),
                    0,
                    MAX_SOURCE_BYTES,
                )
                .unwrap()
            );
        }
    }
}

#[test]
fn negotiation_and_negative_wire_inputs() {
    assert_eq!(negotiate(&["2".into(), "1".into()]), Ok(Version::V1));
    assert_eq!(negotiate(&[]), Err(Error::UnknownVersion));
    assert_eq!(negotiate(&["1.0".into()]), Err(Error::UnknownVersion));
    assert_eq!(decode(b"{"), Err(Error::Malformed));
    assert_eq!(
        decode(&vec![b' '; MAX_MESSAGE_BYTES + 1]),
        Err(Error::MessageTooLarge)
    );
    let value = serde_json::to_value(Wire::Request {
        request: request("r1", 1, Command::Attach),
    })
    .unwrap();
    let schema = serde_json::to_value(schema()).unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    for (field, bad, error) in [
        ("version", serde_json::json!("99"), Error::UnknownVersion),
        (
            "command",
            serde_json::json!({"op":"invented"}),
            Error::UnknownOperation,
        ),
        (
            "command",
            serde_json::json!({"op":"attach","extra":true}),
            Error::Malformed,
        ),
    ] {
        let mut malformed = value.clone();
        malformed["request"][field] = bad;
        assert!(!validator.is_valid(&malformed));
        assert_eq!(decode(&serde_json::to_vec(&malformed).unwrap()), Err(error));
    }
    assert_eq!(validate_identity(&identity(0)), Err(Error::InvalidIdentity));
    assert_eq!(
        validate_identity(&identity(MAX_SAFE_INTEGER + 1)),
        Err(Error::InvalidIdentity)
    );
}

#[test]
fn unit_variants_reject_unknown_fields_and_sources_reject_inconsistent_bounds() {
    let validator = jsonschema::validator_for(&serde_json::to_value(schema()).unwrap()).unwrap();
    for value in [
        serde_json::json!({"kind":"failure","request_id":null,"error":{"code":"cancelled","secret":"no"}}),
        serde_json::json!({"kind":"welcome","version":"1","capabilities":[{"operation":"pause","support":{"status":"supported","extra":true}}]}),
        serde_json::json!({"kind":"failure","error":{"code":"cancelled"}}),
    ] {
        assert!(!validator.is_valid(&value));
        assert_eq!(
            decode(&serde_json::to_vec(&value).unwrap()),
            Err(Error::Malformed)
        );
    }
    assert_eq!(
        decode(br#"{"kind":"welcome","version":"2","capabilities":[]}"#),
        Err(Error::UnknownVersion)
    );
    assert_eq!(
        decode(br#"{"kind":"failure","request_id":null,"request_id":"duplicate","error":{"code":"cancelled"}}"#),
        Err(Error::Malformed)
    );
    let mut chunk = source_chunk(source(1), "text", 0, 4).unwrap();
    chunk.eof = false;
    assert_eq!(encode(&Wire::Source { chunk }), Err(Error::SourceRange));
    let mut chunk = source_chunk(source(1), "text", 0, 4).unwrap();
    chunk.total_bytes = 3;
    assert_eq!(encode(&Wire::Source { chunk }), Err(Error::SourceRange));
}

#[test]
fn wire_handles_cannot_cross_target_session_or_epoch() {
    let current = identity(2);
    let mut foreign_target = current.clone();
    foreign_target.target = "other-runtime".into();
    let mut foreign_session = current.clone();
    foreign_session.session = "session-2".into();
    for foreign in [foreign_target, foreign_session, identity(1)] {
        let handle = Handle {
            identity: foreign,
            pause: 1,
            id: "scope-1".into(),
        };
        for command in [
            Command::ReadScopes {
                handle: handle.clone(),
            },
            Command::Mutate {
                handle: handle.clone(),
                value: "not-dispatched".into(),
            },
        ] {
            let wire = Wire::Request {
                request: request("request-1", 2, command),
            };
            assert_eq!(
                decode(&serde_json::to_vec(&wire).unwrap()),
                Err(Error::StaleHandle)
            );
            assert_eq!(encode(&wire), Err(Error::StaleHandle));
        }
    }
}

#[test]
fn failure_correlation_uses_the_same_identifier_rules_as_requests() {
    for request_id in ["".to_owned(), "bad\nid".to_owned(), "a".repeat(129)] {
        let wire = Wire::Failure {
            request_id: Some(request_id),
            error: Error::Cancelled,
        };
        assert_eq!(
            decode(&serde_json::to_vec(&wire).unwrap()),
            Err(Error::InvalidIdentity)
        );
        assert_eq!(encode(&wire), Err(Error::InvalidIdentity));
    }
    for request_id in [None, Some("request-1".to_owned()), Some("a".repeat(128))] {
        let wire = Wire::Failure {
            request_id,
            error: Error::Cancelled,
        };
        assert_eq!(decode(&encode(&wire).unwrap()).unwrap(), wire);
    }
}

#[test]
fn mock_pause_disconnect_replacement_and_no_replay() {
    let mut mock = Mock::new();
    let grants = [Permission::Inspect, Permission::Control];
    assert_eq!(
        mock.dispatch(&request("a", 1, Command::Attach), &grants),
        Ok(())
    );
    assert_eq!(
        mock.dispatch(&request("p", 1, Command::Pause), &grants),
        Ok(())
    );
    let handle = Handle {
        identity: identity(1),
        pause: 1,
        id: "scope-1".into(),
    };
    assert_eq!(
        mock.dispatch(
            &request(
                "s",
                1,
                Command::ReadScopes {
                    handle: handle.clone()
                }
            ),
            &grants
        ),
        Ok(())
    );
    assert_eq!(
        mock.dispatch(&request("c", 1, Command::Continue), &grants),
        Ok(())
    );
    assert_eq!(
        mock.dispatch(
            &request(
                "s2",
                1,
                Command::ReadScopes {
                    handle: handle.clone()
                }
            ),
            &grants
        ),
        Err(Error::StaleHandle)
    );
    mock.disconnect();
    assert_eq!(
        mock.dispatch(&request("p2", 1, Command::Pause), &grants),
        Err(Error::Disconnected)
    );
    assert_eq!(mock.audit.last().unwrap().request_id, "p2");
    assert_eq!(mock.audit.last().unwrap().outcome, Outcome::Failed);
    assert_eq!(
        mock.dispatch(&request("a2", 1, Command::Attach), &grants),
        Ok(())
    );
    assert_eq!(
        mock.dispatch(&request("p", 1, Command::Pause), &grants),
        Err(Error::DuplicateRequest)
    );
    mock.replace();
    assert_eq!(
        mock.dispatch(
            &request(
                "old",
                1,
                Command::Evaluate {
                    expression: "must-not-replay".into()
                }
            ),
            &[Permission::Evaluate]
        ),
        Err(Error::StaleEpoch)
    );
    assert!(!mock.connected);
    assert_eq!(
        mock.dispatch(&request("a3", 2, Command::Attach), &grants),
        Ok(())
    );
    assert_eq!(
        mock.dispatch(&request("s3", 2, Command::ReadScopes { handle }), &grants),
        Err(Error::StaleHandle)
    );
}

#[test]
fn permission_denial_redaction_and_explicit_unsupported() {
    let mut mock = Mock::new();
    mock.dispatch(&request("a", 1, Command::Attach), &[Permission::Inspect])
        .unwrap();
    let evaluate = request(
        "secret-attempt",
        1,
        Command::Evaluate {
            expression: "secret-token".into(),
        },
    );
    assert_eq!(
        mock.dispatch(&evaluate, &[Permission::Inspect]),
        Err(Error::PermissionDenied {
            permission: Permission::Evaluate
        })
    );
    let audit = serde_json::to_string(&mock.audit).unwrap();
    assert!(!audit.contains("secret-token"));
    assert!(!audit.contains("expression"));
    assert_eq!(mock.audit.last().unwrap().outcome, Outcome::Denied);
    assert_eq!(
        mock.dispatch(
            &request("eval2", 1, evaluate.command),
            &[Permission::Evaluate]
        ),
        Err(Error::Unsupported {
            operation: Operation::Evaluate,
            reason: UnsupportedReason::NotImplemented
        })
    );
    for operation in [
        Operation::Continue,
        Operation::Mutate,
        Operation::CaptureCpu,
        Operation::CaptureHeap,
    ] {
        assert!(authorize(operation, &[Permission::Inspect]).is_err());
    }
    let mut audit_value = serde_json::to_value(&mock.audit[0]).unwrap();
    audit_value["environment"] = serde_json::json!({"TOKEN":"secret"});
    assert!(serde_json::from_value::<Audit>(audit_value).is_err());
}

#[test]
fn immutable_bounded_sources_and_utf8_ranges() {
    let module = include_str!("../../../fixtures/devtools-v1/module.js");
    let full = source_chunk(source(1), module, 0, MAX_SOURCE_BYTES).unwrap();
    assert!(full.eof);
    assert_eq!(full.text, module);
    assert_eq!(full.total_bytes as usize, module.len());
    assert_eq!(
        source_chunk(source(1), module, 0, MAX_SOURCE_BYTES + 1),
        Err(Error::SourceRange)
    );
    assert_eq!(source_chunk(source(1), "é", 1, 2), Err(Error::SourceRange));
    assert_eq!(source_chunk(source(1), "é", 0, 1), Err(Error::SourceRange));
    assert_eq!(source_chunk(source(1), "éx", 0, 2).unwrap().text, "é");
    assert_eq!(
        source_chunk(source(1), module, u32::MAX, 1),
        Err(Error::SourceRange)
    );
    let mut mock = Mock::new();
    let grants = [Permission::Inspect];
    mock.dispatch(&request("a", 1, Command::Attach), &grants)
        .unwrap();
    let mut unknown = source(1);
    unknown.url = "file:///etc/passwd".into();
    assert_eq!(
        mock.dispatch(
            &request(
                "source",
                1,
                Command::ReadSource {
                    source: unknown,
                    offset: 0,
                    max_bytes: 10
                }
            ),
            &grants
        ),
        Err(Error::SourceNotFound)
    );
}

#[test]
fn slow_consumer_overflow_duplicates_and_reorder_are_terminal() {
    let mut receiver = Receiver::default();
    for sequence in 1..=MAX_EVENTS as u64 {
        receiver.receive(event(sequence)).unwrap();
    }
    assert_eq!(
        receiver.receive(event(MAX_EVENTS as u64 + 1)),
        Err(Error::EventOverflow)
    );
    assert!(receiver.queue.is_empty());
    assert_eq!(receiver.receive(event(1)), Err(Error::Disconnected));
    for bad_sequence in [1, 3] {
        let mut receiver = Receiver::default();
        receiver.receive(event(1)).unwrap();
        assert_eq!(
            receiver.receive(event(bad_sequence)),
            Err(Error::EventOrder)
        );
        assert!(receiver.terminal);
    }
}
