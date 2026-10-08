//! Deterministic MOCK adapter. No JS evaluation, native Inspector, or production session service.
#![allow(dead_code)] // Integration test binaries exercise different parts of the same fixture adapter.
use std::collections::BTreeMap;

use kunlun_devtools_protocol::*;

pub fn identity(epoch: u64) -> Identity {
    Identity {
        target: "mock-runtime".into(),
        session: "session-1".into(),
        epoch,
    }
}

pub fn source(epoch: u64) -> SourceIdentity {
    SourceIdentity {
        target: "mock-runtime".into(),
        epoch,
        id: "module-1".into(),
        url: "kunlun:fixture/module".into(),
        revision: "revision-1".into(),
    }
}

pub fn request(id: &str, epoch: u64, command: Command) -> Request {
    Request {
        version: Version::V1,
        request_id: id.into(),
        identity: identity(epoch),
        command,
    }
}

pub fn response(request: &Request, result: Result<CommandResult, Error>) -> Response {
    Response {
        version: Version::V1,
        request_id: request.request_id.clone(),
        identity: request.identity.clone(),
        result: match result {
            Ok(result) => ResponseResult::Success { result },
            Err(error) => ResponseResult::Failure { error },
        },
    }
}

#[derive(Default)]
pub struct Mock {
    pub epoch: u64,
    pub connected: bool,
    pub paused: Option<u64>,
    pub terminal: bool,
    pub negotiated: bool,
    pub audit_available: bool,
    pub pause_counter: u64,
    pub seen: BTreeMap<String, Operation>,
    pub audit: Vec<Audit>,
    pub events: Vec<Event>,
    pub completions: Vec<Response>,
    pub breakpoints: BTreeMap<String, Breakpoint>,
    pending: BTreeMap<String, Request>,
    sequence: u64,
    breakpoint_counter: u64,
}

impl Mock {
    /// Direct unit tests start after a successful hello; portable workflows exercise hello too.
    pub fn new() -> Self {
        Self {
            epoch: 1,
            negotiated: true,
            audit_available: true,
            ..Self::default()
        }
    }

    pub fn target(&self) -> Target {
        let supported = [
            Operation::Attach,
            Operation::Detach,
            Operation::ListSources,
            Operation::ReadSource,
            Operation::ReadScopes,
            Operation::SetBreakpoint,
            Operation::RemoveBreakpoint,
            Operation::Pause,
            Operation::Continue,
            Operation::Step,
            Operation::ReadLogs,
            Operation::Cancel,
        ];
        let unsupported = [
            Operation::Evaluate,
            Operation::Mutate,
            Operation::CaptureCpu,
            Operation::CaptureHeap,
            Operation::LookupSourceMap,
        ];
        Target {
            id: "mock-runtime".into(),
            epoch: self.epoch,
            name: "Contract fixture (MOCK ONLY)".into(),
            backend: Backend::Mock,
            state: if self.terminal {
                TargetState::Terminated
            } else {
                TargetState::Available
            },
            capabilities: supported
                .into_iter()
                .map(|operation| Capability {
                    operation,
                    support: Support::Supported,
                })
                .chain(unsupported.into_iter().map(|operation| Capability {
                    operation,
                    support: Support::Unsupported {
                        reason: UnsupportedReason::NotImplemented,
                    },
                }))
                .collect(),
        }
    }

    pub fn exchange(&mut self, input: Wire, grants: &[Permission]) -> Result<Wire, Error> {
        encode(&input)?;
        match input {
            Wire::Hello { versions } if !self.negotiated => {
                let version = negotiate(&versions)?;
                self.negotiated = true;
                Ok(Wire::Welcome {
                    version,
                    capabilities: self.target().capabilities,
                })
            }
            Wire::Discover {
                version,
                request_id,
                ..
            } if self.negotiated => {
                authorize(Operation::Discover, grants)?;
                Ok(Wire::Discovered {
                    version,
                    request_id,
                    targets: vec![self.target()],
                    truncated: false,
                })
            }
            Wire::Request { request } => {
                let result = self.dispatch(&request, grants);
                Ok(Wire::Response {
                    response: response(&request, result),
                })
            }
            _ => Err(Error::InvalidState),
        }
    }

    fn admit(&mut self, request: &Request, grants: &[Permission]) -> Result<(), Error> {
        encode(&Wire::Request {
            request: request.clone(),
        })?;
        if request.identity != identity(self.epoch) {
            return Err(Error::StaleEpoch);
        }
        if self.terminal {
            return Err(Error::Disconnected);
        }
        if !self.negotiated {
            return Err(Error::InvalidState);
        }
        if self.seen.len() == MAX_REQUEST_HISTORY && !self.seen.contains_key(&request.request_id) {
            self.terminate();
            return Err(Error::LimitExceeded {
                resource: Resource::RequestHistory,
            });
        }
        let operation = request.command.operation();
        let duplicate = match self.seen.entry(request.request_id.clone()) {
            std::collections::btree_map::Entry::Occupied(_) => true,
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(operation);
                false
            }
        };
        let authorization = authorize(operation, grants);
        // A durable audit reservation is required before any adapter action. No payload is copied.
        if !self.audit_available || self.audit.len() == MAX_REQUEST_HISTORY {
            self.terminate();
            return Err(Error::AuditUnavailable);
        }
        self.audit.push(Audit {
            identity: request.identity.clone(),
            request_id: request.request_id.clone(),
            operation,
            permission: operation.permission(),
            check: AuditCheck::Operation,
            outcome: if authorization.is_err() {
                Outcome::Denied
            } else {
                Outcome::Allowed
            },
        });
        if duplicate {
            self.audit.last_mut().unwrap().outcome = Outcome::Failed;
            return Err(Error::DuplicateRequest);
        }
        authorization?;
        if !self.connected && !matches!(request.command, Command::Attach) {
            self.audit.last_mut().unwrap().outcome = Outcome::Failed;
            return Err(Error::Disconnected);
        }
        Ok(())
    }

    pub fn dispatch(
        &mut self,
        request: &Request,
        grants: &[Permission],
    ) -> Result<CommandResult, Error> {
        self.admit(request, grants)?;
        let result = self.execute(request, grants);
        self.finish_audit(&request.request_id, &result);
        result
    }

    fn finish_audit(&mut self, id: &str, result: &Result<CommandResult, Error>) {
        if result.is_err() {
            if let Some(audit) = self
                .audit
                .iter_mut()
                .rev()
                .find(|audit| audit.request_id == id)
            {
                audit.outcome = if matches!(result, Err(Error::PermissionDenied { .. })) {
                    Outcome::Denied
                } else {
                    Outcome::Failed
                };
            }
        }
    }

    fn emit(&mut self, data: EventData) -> Result<(), Error> {
        if self.events.len() == MAX_EVENTS || self.sequence == MAX_SAFE_INTEGER {
            self.terminate();
            return Err(Error::EventOverflow);
        }
        self.sequence += 1;
        self.events.push(Event {
            version: Version::V1,
            identity: identity(self.epoch),
            sequence: self.sequence,
            data,
        });
        Ok(())
    }

    fn pause_event(&self, pause: u64, reason: PauseReason) -> EventData {
        EventData::Paused {
            pause,
            reason,
            truncated: false,
            frames: vec![StackFrame {
                name: "fixture".into(),
                handle: Handle {
                    identity: identity(self.epoch),
                    pause,
                    id: "scope-1".into(),
                },
                location: Some(SourceLocation {
                    source: source(self.epoch),
                    line: 0,
                    column: 0,
                }),
            }],
        }
    }

    fn next_pause(&mut self) -> Result<u64, Error> {
        if let Some(pause) = self
            .pause_counter
            .checked_add(1)
            .filter(|pause| *pause <= MAX_SAFE_INTEGER)
        {
            Ok(pause)
        } else {
            self.terminate();
            Err(Error::StaleHandle)
        }
    }

    fn execute(
        &mut self,
        request: &Request,
        grants: &[Permission],
    ) -> Result<CommandResult, Error> {
        let operation = request.command.operation();
        match &request.command {
            Command::Attach if !self.connected => {
                if self.sequence == MAX_SAFE_INTEGER {
                    self.terminate();
                    return Err(Error::EventOverflow);
                }
                self.connected = true;
                Ok(CommandResult::Attach {
                    target: self.target(),
                    state: ExecutionState::Running,
                    next_sequence: self.sequence + 1,
                })
            }
            Command::Detach => {
                self.disconnect();
                Ok(CommandResult::Detach)
            }
            Command::Pause if self.paused.is_none() => {
                let pause = self.next_pause()?;
                self.emit(self.pause_event(pause, PauseReason::Requested))?;
                self.pause_counter = pause;
                self.paused = Some(pause);
                Ok(CommandResult::Pause)
            }
            Command::Continue if self.paused.is_some() => {
                self.emit(EventData::Resumed)?;
                self.paused = None;
                Ok(CommandResult::Continue)
            }
            Command::Step if self.paused.is_some() => {
                if self.events.len() + 2 > MAX_EVENTS || self.sequence > MAX_SAFE_INTEGER - 2 {
                    self.terminate();
                    return Err(Error::EventOverflow);
                }
                let pause = self.next_pause()?;
                self.emit(EventData::Resumed)?;
                self.emit(self.pause_event(pause, PauseReason::Step))?;
                self.pause_counter = pause;
                self.paused = Some(pause);
                Ok(CommandResult::Step)
            }
            Command::ReadScopes { handle } => {
                if Some(handle.pause) != self.paused || handle.id != "scope-1" {
                    return Err(Error::StaleHandle);
                }
                Ok(CommandResult::ReadScopes {
                    scopes: vec![Scope {
                        name: "module".into(),
                        handle: handle.clone(),
                        truncated: false,
                        variables: vec![
                            Variable {
                                name: "n".into(),
                                value: RemoteValue::Number { value: "1".into() },
                            },
                            Variable {
                                name: "sensitive".into(),
                                value: RemoteValue::Redacted,
                            },
                        ],
                    }],
                    truncated: false,
                })
            }
            Command::ListSources { .. } => Ok(CommandResult::ListSources {
                sources: vec![SourceDescriptor {
                    source: source(self.epoch),
                    source_map: None,
                }],
                truncated: false,
            }),
            Command::ReadSource {
                source: requested,
                offset,
                max_bytes,
            } => {
                if requested != &source(self.epoch) {
                    return Err(Error::SourceNotFound);
                }
                let chunk = source_chunk(
                    requested.clone(),
                    include_str!("../../../../fixtures/devtools-v1/module.js"),
                    *offset,
                    *max_bytes,
                )?;
                Ok(CommandResult::ReadSource { chunk })
            }
            Command::SetBreakpoint {
                source: requested,
                line,
                column,
            } => {
                if requested != &source(self.epoch) {
                    return Err(Error::SourceNotFound);
                }
                if *line != 0 || *column > 18 {
                    return Err(Error::SourceRange);
                }
                if self.breakpoints.len() == MAX_ENTRIES as usize {
                    return Err(Error::LimitExceeded {
                        resource: Resource::Collection,
                    });
                }
                let location = SourceLocation {
                    source: requested.clone(),
                    line: *line,
                    column: *column,
                };
                let breakpoint = Breakpoint {
                    id: format!("bp-{}", self.breakpoint_counter + 1),
                    requested: location.clone(),
                    resolved: vec![location],
                };
                self.breakpoint_counter += 1;
                self.breakpoints
                    .insert(breakpoint.id.clone(), breakpoint.clone());
                Ok(CommandResult::SetBreakpoint { breakpoint })
            }
            Command::RemoveBreakpoint { breakpoint_id } => {
                self.breakpoints
                    .remove(breakpoint_id)
                    .ok_or(Error::RequestNotFound)?;
                Ok(CommandResult::RemoveBreakpoint {
                    breakpoint_id: breakpoint_id.clone(),
                })
            }
            Command::ReadLogs { .. } => Ok(CommandResult::ReadLogs {
                entries: vec![LogEntry {
                    sequence: 1,
                    level: LogLevel::Info,
                    message: RemoteValue::Redacted,
                }],
                truncated: false,
            }),
            Command::Cancel { request_id } => {
                let operation = *self.seen.get(request_id).ok_or(Error::RequestNotFound)?;
                if operation == Operation::Cancel {
                    return Err(Error::InvalidState); // Do not recursively delegate cancellation.
                }
                // Cancellation cannot become an inspect-only backdoor into high-trust work.
                let authorization = authorize(operation, grants);
                if !self.audit_available || self.audit.len() == MAX_REQUEST_HISTORY {
                    self.terminate();
                    return Err(Error::AuditUnavailable);
                }
                self.audit.push(Audit {
                    identity: request.identity.clone(),
                    request_id: request.request_id.clone(),
                    operation: Operation::Cancel,
                    permission: operation.permission(),
                    check: AuditCheck::Cancellation {
                        request_id: request_id.clone(),
                        operation,
                    },
                    outcome: if authorization.is_ok() {
                        Outcome::Allowed
                    } else {
                        Outcome::Denied
                    },
                });
                authorization?;
                if !self.pending.contains_key(request_id) {
                    return Ok(CommandResult::Cancel {
                        request_id: request_id.clone(),
                        status: CancellationStatus::TooLate,
                    });
                }
                if self.completions.len() == MAX_EVENTS {
                    self.terminate();
                    return Err(Error::EventOverflow);
                }
                let pending = self.pending.remove(request_id).unwrap();
                self.completions
                    .push(response(&pending, Err(Error::Cancelled)));
                self.finish_audit(request_id, &Err(Error::Cancelled));
                Ok(CommandResult::Cancel {
                    request_id: request_id.clone(),
                    status: CancellationStatus::Cancelled,
                })
            }
            Command::Attach | Command::Pause | Command::Continue | Command::Step => {
                Err(Error::InvalidState)
            }
            _ => Err(Error::Unsupported {
                operation,
                reason: UnsupportedReason::NotImplemented,
            }),
        }
    }

    /// Test driver holds a request before dispatch, allowing deterministic cancellation races.
    pub fn queue(&mut self, request: Request, grants: &[Permission]) -> Result<(), Error> {
        self.admit(&request, grants)?;
        if self.pending.len() == MAX_PENDING_REQUESTS {
            self.finish_audit(
                &request.request_id,
                &Err(Error::LimitExceeded {
                    resource: Resource::PendingRequests,
                }),
            );
            return Err(Error::LimitExceeded {
                resource: Resource::PendingRequests,
            });
        }
        self.pending.insert(request.request_id.clone(), request);
        Ok(())
    }

    pub fn complete(&mut self, id: &str, grants: &[Permission]) -> Result<Response, Error> {
        let request = self.pending.remove(id).ok_or(Error::RequestNotFound)?;
        // Revalidate grants at execution, not only admission.
        let result = if !self.audit_available {
            self.terminate();
            Err(Error::AuditUnavailable)
        } else {
            authorize(request.command.operation(), grants)
                .and_then(|_| self.execute(&request, grants))
        };
        self.finish_audit(id, &result);
        let response = response(&request, result);
        encode(&Wire::Response {
            response: response.clone(),
        })?;
        Ok(response)
    }

    pub fn disconnect(&mut self) {
        self.connected = false;
        self.paused = None;
        let ids: Vec<String> = self.pending.keys().cloned().collect();
        for id in ids {
            self.finish_audit(&id, &Err(Error::Cancelled));
        }
        self.pending.clear();
        self.events.clear();
        self.completions.clear();
        self.breakpoints.clear();
        // Pause/sequence counters and request tombstones persist throughout the epoch.
    }

    pub fn replace(&mut self) {
        self.disconnect();
        if self.epoch == MAX_SAFE_INTEGER {
            self.terminate();
            return; // Continuity is exhausted; a new target ID is required.
        }
        self.epoch += 1;
        self.pause_counter = 0;
        self.sequence = 0;
        self.breakpoint_counter = 0;
        self.terminal = false;
        self.seen.clear();
        self.audit.clear();
        // Test-driver replacement notification is out of band. New epoch requires explicit attach.
    }

    pub fn terminate(&mut self) {
        self.disconnect();
        self.terminal = true;
    }

    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }
}
