//! MOCK ONLY: finite receiver/recording and teardown evidence, no native capture.
use kunlun_devtools_protocol::*;
use serde::Deserialize;

mod support;
use support::{Mock, identity, request};

#[derive(Default)]
struct Recording {
    frames: Vec<Vec<u8>>,
    bytes: usize,
    terminal: bool,
}

impl Recording {
    fn append(&mut self, event: Event, max_bytes: usize) -> Result<(), Error> {
        if self.terminal {
            return Err(Error::Disconnected);
        }
        let bytes = encode(&Wire::Event { event })?;
        if max_bytes > MAX_RECORDING_BYTES
            || self.frames.len() == MAX_EVENTS
            || self.bytes + bytes.len() > max_bytes
        {
            self.terminal = true;
            // Retain the valid bounded prefix for explicit export, not rolling eviction or replay.
            return Err(Error::LimitExceeded {
                resource: Resource::Recording,
            });
        }
        self.bytes += bytes.len();
        self.frames.push(bytes);
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Suite {
    evidence: String,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    name: String,
    max_bytes: usize,
    frames: u64,
    expected: Option<Error>,
}

#[test]
fn every_portable_bounded_recording_case() {
    let suite: Suite = serde_json::from_str(include_str!(
        "../../../fixtures/devtools-v1/bounded-cases.json"
    ))
    .unwrap();
    assert!(suite.evidence.starts_with("MOCK ONLY"));
    for case in suite.cases {
        let mut recording = Recording::default();
        let result = (1..=case.frames)
            .try_for_each(|sequence| recording.append(event(sequence), case.max_bytes));
        assert_eq!(result.err(), case.expected, "{}", case.name);
        assert!(recording.frames.len() <= MAX_EVENTS);
        assert!(recording.bytes <= case.max_bytes);
        assert_eq!(
            recording.bytes,
            recording.frames.iter().map(Vec::len).sum::<usize>()
        );
        for frame in &recording.frames {
            assert!(matches!(decode(frame), Ok(Wire::Event { .. })));
        }
        if recording.terminal {
            assert_eq!(
                recording.append(event(1), case.max_bytes),
                Err(Error::Disconnected)
            );
        }
    }
}

#[test]
fn slow_consumer_terminates_adapter_and_drains_all_live_work() {
    let grants = [Permission::Inspect, Permission::Control];
    let mut mock = Mock::new();
    mock.dispatch(&request("attach", 1, Command::Attach), &grants)
        .unwrap();
    mock.queue(
        request("pending", 1, Command::ReadLogs { max_entries: 1 }),
        &grants,
    )
    .unwrap();
    for n in 0..MAX_EVENTS {
        let command = if n % 2 == 0 {
            Command::Pause
        } else {
            Command::Continue
        };
        mock.dispatch(&request(&format!("control-{n}"), 1, command), &grants)
            .unwrap();
    }
    assert_eq!(mock.events.len(), MAX_EVENTS);
    assert_eq!(
        mock.dispatch(&request("overflow", 1, Command::Pause), &grants),
        Err(Error::EventOverflow)
    );
    assert!(mock.terminal);
    assert!(!mock.connected);
    assert!(mock.paused.is_none());
    assert_eq!(mock.pending_count(), 0);
    assert!(mock.events.is_empty());
    assert_eq!(
        mock.complete("pending", &grants),
        Err(Error::RequestNotFound)
    );
}

#[test]
fn queued_actions_do_not_execute_after_audit_loss_and_epochs_never_wrap() {
    let grants = [Permission::Inspect, Permission::Control];
    let mut mock = Mock::new();
    mock.dispatch(&request("attach", 1, Command::Attach), &grants)
        .unwrap();
    mock.queue(request("pending", 1, Command::Pause), &grants)
        .unwrap();
    mock.audit_available = false;
    let completion = mock.complete("pending", &grants).unwrap();
    assert_eq!(
        completion.result,
        ResponseResult::Failure {
            error: Error::AuditUnavailable
        }
    );
    assert!(mock.terminal);
    assert!(mock.paused.is_none());
    assert!(mock.events.is_empty());
    let mut mock = Mock::new();
    mock.epoch = MAX_SAFE_INTEGER;
    mock.replace();
    assert_eq!(mock.epoch, MAX_SAFE_INTEGER);
    assert!(mock.terminal);
}
