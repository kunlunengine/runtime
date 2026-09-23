use super::{contract::*, model::*};

fn fixture() -> Conversation {
    conversation(include_bytes!("conversation.json")).unwrap()
}

fn broker(request: &Request) -> Broker {
    let mut value = Broker {
        peer: Peer {
            binding: request.binding.clone(),
            profile: request.profile.clone(),
        },
        attached: true,
        foreground: true,
        input_owner: true,
        grants: Default::default(),
        resources: Default::default(),
    };
    value.resources.insert(
        request.resource.clone(),
        Resource {
            expires: 100,
            lease: request.lease,
            lease_owner: request.binding.clone(),
            lease_expires: 40,
            writable: true,
        },
    );
    value.consent(request.grant.clone(), request, 50, true);
    value
}

#[test]
fn serialized_conversation_is_consumed() {
    let conversation = fixture();
    let host = broker(&conversation.request);
    assert_eq!(host.authorize(&host.peer, &conversation.request, 1), Ok(()));
    let mut adapter = Adapter::new(conversation.request.binding.session.clone(), 1);
    adapter.uncertain.insert("command-1".into());
    for frame in &conversation.frames {
        assert_eq!(adapter.receive(frame), Delivery::Applied);
    }
    assert!(adapter.terminal);
    assert!(adapter.reconnect().is_empty());
}

#[test]
fn schema_rejects_unknowns_and_bounds() {
    let base = serde_json::to_value(fixture().request).unwrap();
    for (field, value) in [
        ("protocol", "v2"),
        ("profile", "ambient"),
        ("secret", "credential"),
        ("authenticated_peer", "forged"),
    ] {
        let mut wire = base.clone();
        wire[field] = value.into();
        assert!(
            decode(&serde_json::to_vec(&wire).unwrap()).is_err(),
            "{field}"
        );
    }
    for operation in [
        serde_json::json!({"op":"unknown"}),
        serde_json::json!({"op":"fs_read","path":"/etc/passwd"}),
        serde_json::json!({"op":"pty_resize","columns":0,"rows":24}),
        serde_json::json!({"op":"pty_resize","columns":80,"rows":301}),
        serde_json::json!({"op":"pty_input","data":"x".repeat(MAX_PAYLOAD+1)}),
    ] {
        let mut wire = base.clone();
        wire["operation"] = operation;
        assert!(decode(&serde_json::to_vec(&wire).unwrap()).is_err());
    }
    assert!(decode(&vec![b' '; MAX_MESSAGE + 1]).is_err());
    let mut request = fixture().request;
    request.operation = Operation::PtyResize {
        columns: 80,
        rows: 24,
    };
    assert_eq!(decode(&serde_json::to_vec(&request).unwrap()), Ok(request));
    let mut trace = fixture();
    trace.frames = vec![trace.frames[0].clone(); MAX_FRAMES + 1];
    assert!(conversation(&serde_json::to_vec(&trace).unwrap()).is_err());
}

#[test]
fn grants_default_deny_and_bind_every_authority_dimension() {
    for case in [
        "peer",
        "app",
        "window",
        "session",
        "epoch",
        "missing",
        "revoked",
        "expired",
        "resource_expired",
        "resource",
        "operation",
        "lease",
        "lease_owner",
        "lease_expired",
        "readonly",
        "background",
        "input_owner",
        "mobile",
        "no_consent",
    ] {
        let mut request = fixture().request;
        let mut host = broker(&request);
        let mut peer = host.peer.clone();
        let mut now = 1;
        match case {
            "peer" => peer.binding.app = "attacker".into(),
            "app" => request.binding.app = "other".into(),
            "window" => request.binding.window = "other".into(),
            "session" => request.binding.session = "other".into(),
            "epoch" => request.binding.epoch += 1,
            "missing" => request.grant = "missing".into(),
            "revoked" => host.grants.clear(),
            "expired" => now = 50,
            "resource_expired" => host.resources.get_mut(&request.resource).unwrap().expires = 1,
            "resource" => request.resource = "other".into(),
            "operation" => request.operation = Operation::FsRead {},
            "lease" => request.lease += 1,
            "lease_owner" => {
                host.resources
                    .get_mut(&request.resource)
                    .unwrap()
                    .lease_owner
                    .window = "other".into()
            }
            "lease_expired" => now = 40,
            "readonly" => host.resources.get_mut(&request.resource).unwrap().writable = false,
            "background" => host.foreground = false,
            "input_owner" => host.input_owner = false,
            "mobile" => {
                request.profile = Profile::MobileRemote;
                host.peer.profile = Profile::MobileRemote;
                peer.profile = Profile::MobileRemote;
            }
            "no_consent" => {
                host.grants.clear();
                host.consent(request.grant.clone(), &request, 50, false);
            }
            _ => unreachable!(),
        }
        assert!(host.authorize(&peer, &request, now).is_err(), "{case}");
    }
    for operation in [Operation::FsRead {}, Operation::SessionRead {}] {
        let mut request = fixture().request;
        request.operation = operation;
        let mut host = broker(&request);
        host.resources.get_mut(&request.resource).unwrap().writable = false;
        host.resources
            .get_mut(&request.resource)
            .unwrap()
            .lease_expires = 0;
        assert_eq!(host.authorize(&host.peer, &request, 1), Ok(()));
    }
}

#[test]
fn resize_and_input_require_foreground_and_fresh_owner_after_focus_regain() {
    for operation in [
        Operation::PtyInput {
            data: "input".into(),
        },
        Operation::PtyResize {
            columns: 120,
            rows: 40,
        },
    ] {
        let mut request = fixture().request;
        request.operation = operation;
        let mut host = broker(&request);
        assert_eq!(host.authorize(&host.peer, &request, 1), Ok(()));
        host.focus(false);
        assert!(host.authorize(&host.peer, &request, 1).is_err());
        host.focus(true);
        assert!(host.authorize(&host.peer, &request, 1).is_err());
        host.input_owner = true; // Fresh arbitration by the trusted session authority.
        assert_eq!(host.authorize(&host.peer, &request, 1), Ok(()));
        host.resources.get_mut(&request.resource).unwrap().lease += 1;
        assert!(host.authorize(&host.peer, &request, 1).is_err());
    }
}

#[test]
fn independent_permissions_do_not_require_terminal_input_ownership() {
    for operation in [
        Operation::ApprovalRespond {
            approval: "approval-ref".into(),
            allow: true,
        },
        Operation::Cancel {
            target: "remote-command-ref".into(),
        },
        Operation::ProviderDisconnect {},
    ] {
        let mut request = fixture().request;
        request.operation = operation;
        let mut host = broker(&request);
        host.focus(false);
        host.resources
            .get_mut(&request.resource)
            .unwrap()
            .lease_expires = 0;
        // The resource has separate operation permission, not a session input lease.
        assert_eq!(host.authorize(&host.peer, &request, 1), Ok(()));
        host.resources.get_mut(&request.resource).unwrap().writable = false;
        assert!(host.authorize(&host.peer, &request, 1).is_err());
        host.resources.get_mut(&request.resource).unwrap().writable = true;
        host.grants.clear();
        host.input_owner = true; // Input ownership cannot confer unrelated permissions.
        assert!(host.authorize(&host.peer, &request, 1).is_err());
    }
}

#[test]
fn all_operations_require_exact_consent() {
    for operation in [
        Operation::PtyOpen {},
        Operation::ProcessStart {},
        Operation::FsRead {},
        Operation::FsWrite {
            data: "data".into(),
        },
        Operation::ProviderConnect {},
        Operation::ProviderDisconnect {},
        Operation::WindowOpen {},
        Operation::DeepLinkOpen {},
        Operation::SessionRead {},
        Operation::SessionCommand {
            command: "command-ref".into(),
        },
        Operation::ApprovalRespond {
            approval: "approval-ref".into(),
            allow: true,
        },
        Operation::Cancel {
            target: "process-group-ref".into(),
        },
    ] {
        let mut request = fixture().request;
        request.operation = operation;
        let mut host = broker(&request);
        assert_eq!(host.authorize(&host.peer, &request, 1), Ok(()));
        host.grants.clear();
        assert!(host.authorize(&host.peer, &request, 1).is_err());
    }
}

#[test]
fn changing_approved_parameters_or_reusing_a_cross_window_grant_is_denied() {
    let mut request = fixture().request;
    request.operation = Operation::ApprovalRespond {
        approval: "patch-digest-a".into(),
        allow: true,
    };
    let host = broker(&request);
    assert_eq!(host.authorize(&host.peer, &request, 1), Ok(()));
    request.operation = Operation::ApprovalRespond {
        approval: "patch-digest-b".into(),
        allow: true,
    };
    assert!(host.authorize(&host.peer, &request, 1).is_err());
    request.operation = Operation::ApprovalRespond {
        approval: "patch-digest-a".into(),
        allow: false,
    };
    assert!(host.authorize(&host.peer, &request, 1).is_err());
    let mut new_peer = host.peer.clone();
    new_peer.binding.window = "window-2".into();
    request.binding = new_peer.binding.clone();
    assert!(host.authorize(&new_peer, &request, 1).is_err());
}

#[test]
fn streams_bound_credit_payload_and_control_without_silent_drop() {
    let mut stream = Stream::default();
    assert!(
        stream
            .push(SessionEvent::Output {
                data: "x".repeat(MAX_PAYLOAD)
            })
            .is_err()
    );
    for _ in 0..MAX_FRAMES - 1 {
        stream
            .push(SessionEvent::Output {
                data: "中文".into(),
            })
            .unwrap();
    }
    let pending = SessionEvent::Output {
        data: "must retry after ACK".into(),
    };
    assert_eq!(stream.push(pending.clone()), Err(("backpressure", pending)));
    assert!(!stream.closed);
    stream.push(SessionEvent::Terminal {}).unwrap(); // Reserved control capacity.
    assert_eq!(stream.queue.len(), MAX_FRAMES);
    assert!(matches!(
        stream.queue.back().unwrap().event,
        SessionEvent::Terminal {}
    ));
    assert!(stream.sent <= MAX_STREAM_BYTES as u64);
    let first = stream.queue.front().unwrap().end;
    let bytes = serde_json::to_vec(&SessionEvent::Output {
        data: "中文".into(),
    })
    .unwrap();
    assert_eq!(first, bytes.len() as u64);
    for bad in [0, first - 1, stream.sent + 1, u64::MAX] {
        assert!(stream.ack(bad).is_err());
    }
    stream.ack(first).unwrap();
    assert!(stream.ack(first).is_err()); // Cannot mint credit by replaying an ACK.
    stream
        .push(SessionEvent::ApprovalRequired {
            approval: "short-ref".into(),
        })
        .unwrap();
    assert_eq!(stream.queue.len(), MAX_FRAMES);
    assert_eq!(
        stream.push(SessionEvent::Terminal {}),
        Err(("backpressure", SessionEvent::Terminal {}))
    );
    assert!(stream.closed); // Control saturation requires disconnect and resync.
    assert!(stream.ack(stream.sent).is_err());
}

#[test]
fn sequence_gap_duplicate_reconnect_queries_not_replay() {
    let frames = fixture().frames;
    let mut adapter = Adapter::new("remote-1".into(), 1);
    adapter.uncertain.insert("uncertain-command".into());
    assert_eq!(adapter.receive(&frames[0]), Delivery::Applied);
    assert_eq!(adapter.receive(&frames[0]), Delivery::Duplicate);
    assert_eq!(adapter.receive(&frames[2]), Delivery::Resync);
    assert_eq!(adapter.receive(&frames[1]), Delivery::Resync);
    assert_eq!(adapter.reconnect(), vec!["uncertain-command"]);
    assert!(adapter.resync(1, 0, false).is_err());
    adapter.resync(1, 3, true).unwrap();
    adapter.resync(1, 4, false).unwrap();
    assert!(adapter.terminal);
    assert_eq!(adapter.reconnect(), vec!["uncertain-command"]);
    let mut other = frames[0].clone();
    other.session = "other".into();
    assert_eq!(adapter.receive(&other), Delivery::WrongSession);
    other.session = "remote-1".into();
    other.history = 2;
    assert_eq!(adapter.receive(&other), Delivery::Resync);
    adapter.resync(2, 0, false).unwrap();
    assert_eq!(adapter.receive(&other), Delivery::Applied);
    assert!(adapter.resync(1, 100, false).is_err());
    assert!(adapter.terminal);
}

#[test]
fn crash_restart_reattach_preserves_remote_session_not_authority() {
    let mut request = fixture().request;
    let mut host = broker(&request);
    let mut remote = Adapter::new(request.binding.session.clone(), 1);
    remote.receive(&fixture().frames[0]);
    for _lifecycle in ["detach", "crash", "host restart"] {
        host.detach();
        assert!(host.grants.is_empty() && host.resources.is_empty());
        host.reattach();
        assert!(host.authorize(&host.peer, &request, 1).is_err());
        request.binding = host.peer.binding.clone();
        assert!(host.authorize(&host.peer, &request, 1).is_err());
        host.resources.insert(
            request.resource.clone(),
            Resource {
                expires: 100,
                lease: 7,
                lease_owner: request.binding.clone(),
                lease_expires: 40,
                writable: true,
            },
        );
        host.consent(request.grant.clone(), &request, 50, true);
        assert!(host.authorize(&host.peer, &request, 1).is_err());
        host.foreground = true;
        host.input_owner = true;
        assert_eq!(host.authorize(&host.peer, &request, 1), Ok(()));
        assert_eq!(remote.sequence, 1);
    }
}

#[test]
fn cancellation_is_targeted_idempotent_and_cleanup_is_host_fact() {
    let mut group = ProcessGroup {
        target: "group-ref".into(),
        ..Default::default()
    };
    assert!(group.cancel("other").is_err());
    group.cancel("group-ref").unwrap();
    group.cancel("group-ref").unwrap();
    assert!(group.cancel_requested && !group.terminal && !group.cleanup_ack);
    group.cleanup(false);
    assert!(!group.terminal);
    group.exited(); // A racing successful exit is terminal, but not proof of tree cleanup.
    assert!(group.terminal && !group.cleanup_ack);
    group.cancel("group-ref").unwrap();
    group.cleanup(true);
    group.cleanup(false);
    assert!(group.terminal && group.cleanup_ack);
}

#[test]
fn device_auth_states_and_revocation_tombstone() {
    for state in [
        Auth::Pending,
        Auth::Denied,
        Auth::Expired,
        Auth::Cancelled,
        Auth::ReauthorizationRequired,
        Auth::Revoked,
    ] {
        let mut provider = Provider {
            state,
            attempt: 1,
            connected: false,
        };
        assert!(provider.connect().is_err());
    }
    let mut provider = Provider {
        state: Auth::Pending,
        attempt: 1,
        connected: false,
    };
    provider.host_auth(1, Auth::Authorized);
    provider.connect().unwrap();
    provider.reconnect();
    assert_eq!(provider.state, Auth::ReauthorizationRequired);
    provider.host_auth(1, Auth::Authorized); // Late completion from the old connection epoch.
    assert_eq!(provider.state, Auth::ReauthorizationRequired);
    assert!(provider.connect().is_err());
    provider.host_auth(2, Auth::Authorized);
    provider.connect().unwrap();
    provider.host_auth(2, Auth::Revoked);
    provider.reconnect();
    provider.host_auth(3, Auth::Authorized);
    assert_eq!(provider.state, Auth::Revoked);
    assert!(!provider.connected);
    assert!(provider.connect().is_err());
    for state in [Auth::Denied, Auth::Expired, Auth::Cancelled] {
        let mut provider = Provider {
            state,
            attempt: 1,
            connected: false,
        };
        provider.host_auth(1, Auth::Authorized);
        assert_eq!(provider.state, state); // A completed failed attempt needs a new reference.
    }
}

#[test]
fn update_floor_sequences_restart_and_scoped_rollback() {
    let (mut state, mut security, revocations, current, target, auth) = update_fixture();
    security.metadata.sequence += 1;
    security.floor = [1, 0, 0, 0];
    state.policy(security, &current.scope, 10).unwrap();
    state = state.clone(); // Trusted durable reload, not real disk persistence.
    assert_eq!(state.minimum_cef, [140, 2, 10, 3]);
    assert_eq!(state.sequences(), (Some(8), Some(9)));
    for cef in [[140, 2, 10, 3], [140, 2, 10, 4], [141, 0, 0, 0]] {
        let mut state = state.clone();
        let mut target = target.clone();
        target.cef = cef;
        // Every application downgrade needs authorization, even with equal/newer CEF.
        assert!(
            state
                .install(&target, &current, Some("health"), None, 10)
                .is_err()
        );
        state
            .install(&target, &current, Some("health"), Some(&auth), 10)
            .unwrap();
        let mut restarted = state.clone();
        restarted.check_bundle(&target, &current.scope, 10).unwrap();
        assert!(
            restarted
                .install(&target, &current, Some("health"), Some(&auth), 10)
                .is_err()
        );
        assert_eq!(restarted.minimum_cef, [140, 2, 10, 3]);
        assert_eq!(restarted.sequences(), (Some(8), Some(9)));
    }
    let mut upgrade = target.clone();
    upgrade.release.order = current.release.order + 1;
    state.install(&upgrade, &current, None, None, 10).unwrap();
    assert!(state.revoke(revocations, &current.scope, 10).is_err());
}

fn update_fixture() -> (
    UpdatePolicy,
    SecurityPolicy,
    Revocations,
    Bundle,
    Bundle,
    Rollback,
) {
    let scope = UpdateScope {
        app: "wuling".into(),
        platform: "macos-arm64".into(),
        channel: "stable".into(),
    };
    let security = SecurityPolicy {
        metadata: Metadata {
            sequence: 7,
            expires: 30,
            scope: scope.clone(),
            verified: true,
        },
        floor: [140, 2, 10, 3],
        approved_cef: [
            [140, 2, 10, 2],
            [140, 2, 10, 3],
            [140, 2, 10, 4],
            [141, 0, 0, 0],
        ]
        .into_iter()
        .map(|cef| (cef, "revision".into(), "artifact".into()))
        .collect(),
    };
    let revocations = Revocations {
        metadata: Metadata {
            sequence: 9,
            expires: 40,
            scope: scope.clone(),
            verified: true,
        },
        identities: ["previously-revoked".into()].into_iter().collect(),
    };
    let target = Bundle {
        release: Release {
            order: 10,
            manifest_digest: "target-digest".into(),
        },
        scope: scope.clone(),
        cef: [140, 2, 10, 3],
        revision: "revision".into(),
        artifact: "artifact".into(),
        signing_key: "bundle-key".into(),
        verified: true,
        complete: true,
        compatible: true,
    };
    let mut current = target.clone();
    current.release = Release {
        order: 11,
        manifest_digest: "source-digest".into(),
    };
    let auth = Rollback {
        id: "once".into(),
        source: current.release.clone(),
        target: target.release.clone(),
        scope: scope.clone(),
        failure: "health".into(),
        expires: 20,
        verified: true,
        rollback_role: true,
        authority_key: "authority-key".into(),
    };
    let mut state = UpdatePolicy::default();
    state.policy(security.clone(), &scope, 10).unwrap();
    state.revoke(revocations.clone(), &scope, 10).unwrap();
    (state, security, revocations, current, target, auth)
}

#[test]
fn update_bundle_and_rollback_denials() {
    for case in [
        "floor",
        "incomplete",
        "incompatible",
        "signature",
        "revision",
        "artifact",
        "bundle_app",
        "bundle_platform",
        "bundle_channel",
        "source",
        "source_order",
        "target",
        "target_order",
        "app",
        "platform",
        "channel",
        "failure",
        "no_failure",
        "empty_failure",
        "expired",
        "auth_signature",
        "role",
        "empty_digest",
        "empty_key",
        "empty_auth",
        "empty_authority",
        "same_order_different_digest",
    ] {
        let (mut state, _, _, current, mut bundle, mut auth) = update_fixture();
        match case {
            "floor" => bundle.cef = [140, 2, 10, 2],
            "incomplete" => bundle.complete = false,
            "incompatible" => bundle.compatible = false,
            "signature" => bundle.verified = false,
            "revision" => bundle.revision = "unapproved".into(),
            "artifact" => bundle.artifact = "unapproved".into(),
            "bundle_app" => bundle.scope.app = "other".into(),
            "bundle_platform" => bundle.scope.platform = "other".into(),
            "bundle_channel" => bundle.scope.channel = "other".into(),
            "source" => auth.source.manifest_digest = "other".into(),
            "source_order" => auth.source.order += 1,
            "target" => auth.target.manifest_digest = "other".into(),
            "target_order" => auth.target.order += 1,
            "app" => auth.scope.app = "other".into(),
            "platform" => auth.scope.platform = "other".into(),
            "channel" => auth.scope.channel = "other".into(),
            "failure" => auth.failure = "other".into(),
            "empty_failure" => auth.failure.clear(),
            "expired" => auth.expires = 10,
            "auth_signature" => auth.verified = false,
            "role" => auth.rollback_role = false,
            "empty_digest" => {
                bundle.release.manifest_digest.clear();
                auth.target = bundle.release.clone();
            }
            "empty_key" => bundle.signing_key.clear(),
            "empty_auth" => auth.id.clear(),
            "empty_authority" => auth.authority_key.clear(),
            "same_order_different_digest" => bundle.release.order = current.release.order,
            _ => {}
        }
        let failure = if case == "no_failure" {
            None
        } else {
            Some("health")
        };
        assert!(
            state
                .install(&bundle, &current, failure, Some(&auth), 10)
                .is_err(),
            "{case}"
        );
    }
}

#[test]
fn update_metadata_fails_closed_and_preserves_cache() {
    for kind in ["policy", "revocations"] {
        for case in [
            "missing",
            "signature",
            "expired",
            "app",
            "platform",
            "channel",
            "older",
            "equal",
        ] {
            let (mut state, mut security, mut revocations, current, target, auth) =
                update_fixture();
            let metadata = if kind == "policy" {
                &mut security.metadata
            } else {
                &mut revocations.metadata
            };
            match case {
                "signature" => metadata.verified = false,
                "expired" => metadata.expires = 10,
                "app" => metadata.scope.app = "other".into(),
                "platform" => metadata.scope.platform = "other".into(),
                "channel" => metadata.scope.channel = "other".into(),
                "older" => metadata.sequence -= 1,
                _ => {}
            }
            if !matches!(case, "older" | "equal") {
                metadata.sequence += 1;
            }
            if case == "missing" {
                state = UpdatePolicy::default();
                if kind == "policy" {
                    state.revoke(revocations, &current.scope, 10).unwrap();
                } else {
                    state.policy(security, &current.scope, 10).unwrap();
                }
                assert!(
                    state
                        .install(&target, &current, Some("health"), Some(&auth), 10)
                        .is_err()
                );
            } else {
                let result = if kind == "policy" {
                    state.policy(security, &current.scope, 10)
                } else {
                    state.revoke(revocations, &current.scope, 10)
                };
                assert!(result.is_err(), "{kind}/{case}");
                assert_eq!(state.sequences(), (Some(7), Some(9)));
                state.check_bundle(&target, &current.scope, 10).unwrap();
            }
        }
        let (mut state, mut security, _, current, target, _) = update_fixture();
        // Independently expire each cached metadata record.
        let now = if kind == "policy" {
            30
        } else {
            security.metadata.sequence += 1;
            security.metadata.expires = 50;
            state.policy(security, &current.scope, 10).unwrap();
            40
        };
        assert!(
            state
                .clone()
                .check_bundle(&target, &current.scope, now)
                .is_err(),
            "{kind}"
        );
    }
}

#[test]
fn update_activation_rechecks_all_revocations_and_retains_them() {
    for identity in [
        "revision",
        "artifact",
        "target-digest",
        "bundle-key",
        "once",
        "authority-key",
    ] {
        let (mut state, _, mut revocations, current, target, auth) = update_fixture();
        state
            .preflight(&target, &current, Some("health"), Some(&auth), 10)
            .unwrap();
        revocations.metadata.sequence += 1;
        revocations.identities = [identity.into()].into_iter().collect();
        state
            .revoke(revocations.clone(), &current.scope, 10)
            .unwrap();
        // A later empty list cannot undo retained revocations.
        revocations.metadata.sequence += 1;
        revocations.identities.clear();
        state.revoke(revocations, &current.scope, 10).unwrap();
        let mut restarted = state.clone();
        assert!(
            restarted
                .install(&target, &current, Some("health"), Some(&auth), 10)
                .is_err(),
            "{identity}"
        );
        assert_eq!(restarted.sequences(), (Some(7), Some(11)));
        if !matches!(identity, "once" | "authority-key") {
            assert!(restarted.check_bundle(&target, &current.scope, 10).is_err());
        }
    }
}

#[test]
fn update_numeric_floor_and_activation_time_are_rechecked() {
    for cef in [
        [139, 99, 99, 99],
        [140, 1, 99, 99],
        [140, 2, 9, 99],
        [140, 2, 10, 2],
    ] {
        let (mut state, mut security, _, current, mut target, auth) = update_fixture();
        target.cef = cef;
        // Keep approval valid so each rejection is specifically the numeric floor.
        security
            .approved_cef
            .insert((cef, target.revision.clone(), target.artifact.clone()));
        security.metadata.sequence += 1;
        state.policy(security, &current.scope, 10).unwrap();
        assert!(
            state
                .install(&target, &current, Some("health"), Some(&auth), 10)
                .is_err()
        );
        target.release.order = current.release.order + 1;
        assert!(state.install(&target, &current, None, None, 10).is_err());
    }
    for case in ["floor", "policy_expiry", "revocation_expiry", "auth_expiry"] {
        let (mut state, mut security, _, current, target, mut auth) = update_fixture();
        if case != "auth_expiry" {
            auth.expires = 100;
        }
        state
            .preflight(&target, &current, Some("health"), Some(&auth), 10)
            .unwrap();
        let now = match case {
            "floor" => {
                security.metadata.sequence += 1;
                security.floor = [140, 2, 10, 4];
                state.policy(security, &current.scope, 10).unwrap();
                10
            }
            "policy_expiry" => 30,
            "revocation_expiry" => {
                security.metadata.sequence += 1;
                security.metadata.expires = 50;
                state.policy(security, &current.scope, 10).unwrap();
                40
            }
            _ => 20,
        };
        assert!(
            state
                .install(&target, &current, Some("health"), Some(&auth), now)
                .is_err(),
            "{case}"
        );
    }
}
