//! Pure policy oracle. Host identity, consent, time, signature verification, persistence,
//! and process-group cleanup are trusted test facts, NOT native implementation evidence.
use super::contract::*;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Clone)]
pub struct Peer {
    pub binding: Binding,
    pub profile: Profile,
}

#[derive(Clone)]
pub struct Grant {
    pub binding: Binding,
    pub operation: Operation,
    pub resource: String,
    pub expires: u64,
}

pub struct Resource {
    pub expires: u64,
    pub lease: u64,
    pub lease_owner: Binding,
    pub lease_expires: u64,
    pub writable: bool,
}

pub struct Broker {
    pub peer: Peer,
    pub attached: bool,
    pub foreground: bool,
    pub input_owner: bool,
    pub grants: BTreeMap<String, Grant>,
    pub resources: BTreeMap<String, Resource>,
}

impl Broker {
    /// Only the trusted host calls this after consent, never the renderer.
    pub fn consent(&mut self, id: String, request: &Request, expires: u64, approved: bool) {
        if approved && self.attached {
            self.grants.insert(
                id,
                Grant {
                    binding: self.peer.binding.clone(),
                    operation: request.operation.clone(),
                    resource: request.resource.clone(),
                    expires,
                },
            );
        }
    }

    pub fn authorize(&self, peer: &Peer, request: &Request, now: u64) -> Result<(), &'static str> {
        if !self.attached
            || peer.binding != self.peer.binding
            || request.binding != peer.binding
            || peer.profile != self.peer.profile
            || request.profile != peer.profile
        {
            return Err("binding");
        }
        let grant = self.grants.get(&request.grant).ok_or("grant")?;
        let resource = self.resources.get(&request.resource).ok_or("resource")?;
        if grant.binding != peer.binding
            || grant.resource != request.resource
            || grant.operation != request.operation
            || now >= grant.expires
            || now >= resource.expires
        {
            return Err("grant");
        }
        if !resource.writable && !request.operation.read_only() {
            return Err("read only");
        }
        if request.operation.requires_input_lease()
            && (resource.lease != request.lease
                || resource.lease_owner != peer.binding
                || now >= resource.lease_expires)
        {
            return Err("lease");
        }
        if matches!(peer.profile, Profile::MobileRemote) && request.operation.local_exec() {
            return Err("unsupported");
        }
        if matches!(
            request.operation,
            Operation::PtyInput { .. } | Operation::PtyResize { .. }
        ) && !(self.foreground && self.input_owner)
        {
            return Err("input owner");
        }
        if !request.operation.valid() {
            return Err("payload");
        }
        Ok(())
    }

    pub fn focus(&mut self, foreground: bool) {
        self.foreground = foreground;
        // Focus regain alone never reacquires the consumer's input lease.
        self.input_owner = false;
    }

    /// Detach, crash and restart have identical authority invalidation.
    pub fn detach(&mut self) {
        self.attached = false;
        self.foreground = false;
        self.input_owner = false;
        self.grants.clear();
        self.resources.clear();
        self.peer.binding.epoch = self
            .peer
            .binding
            .epoch
            .checked_add(1)
            .expect("epoch exhausted");
    }

    pub fn reattach(&mut self) {
        self.attached = true;
        // No grants, resources, input ownership or commands restored.
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Delivery {
    Applied,
    Duplicate,
    Resync,
    WrongSession,
}

pub struct Adapter {
    pub session: String,
    pub history: u64,
    pub sequence: u64,
    pub needs_resync: bool,
    pub terminal: bool,
    pub uncertain: BTreeSet<String>,
}

impl Adapter {
    pub fn new(session: String, history: u64) -> Self {
        Self {
            session,
            history,
            sequence: 0,
            needs_resync: false,
            terminal: false,
            uncertain: BTreeSet::new(),
        }
    }

    pub fn receive(&mut self, frame: &Frame) -> Delivery {
        if frame.session != self.session {
            return Delivery::WrongSession;
        }
        if frame.history != self.history {
            self.needs_resync = true;
            return Delivery::Resync;
        }
        if frame.sequence <= self.sequence {
            return Delivery::Duplicate;
        }
        if self.needs_resync || self.sequence.checked_add(1) != Some(frame.sequence) {
            self.needs_resync = true;
            return Delivery::Resync;
        }
        self.sequence = frame.sequence;
        self.terminal |= matches!(frame.event, SessionEvent::Terminal {});
        if let SessionEvent::CommandDone { command } = &frame.event {
            self.uncertain.remove(command);
        }
        Delivery::Applied
    }

    /// Reconnect produces read-only status queries, never mutation replay.
    pub fn reconnect(&self) -> Vec<String> {
        self.uncertain.iter().cloned().collect()
    }

    /// Trusted authoritative snapshot; a stale snapshot cannot rewind state.
    pub fn resync(
        &mut self,
        history: u64,
        sequence: u64,
        terminal: bool,
    ) -> Result<(), &'static str> {
        if history < self.history || (history == self.history && sequence < self.sequence) {
            return Err("stale snapshot");
        }
        self.history = history;
        self.sequence = sequence;
        self.terminal |= terminal;
        self.needs_resync = false;
        Ok(())
    }
}

/// One authenticated stream/epoch, separate from the session event sequence.
/// Lengths/offsets count serialized event bytes, not Unicode characters.
pub struct StreamFrame {
    pub end: u64,
    pub event: SessionEvent,
}

#[derive(Default)]
pub struct Stream {
    pub queue: VecDeque<StreamFrame>,
    pub sent: u64,
    pub acked: u64,
    pub closed: bool,
}

impl Stream {
    /// Fixed negotiated byte window. Only valid ACKs replenish it.
    pub fn push(&mut self, event: SessionEvent) -> Result<(), (&'static str, SessionEvent)> {
        let bytes = serde_json::to_vec(&event).map_err(|_| ("encoding", event.clone()))?;
        if self.closed || bytes.len() > MAX_PAYLOAD {
            return Err(("closed or oversized", event));
        }
        let control = !matches!(event, SessionEvent::Output { .. });
        // Reserve one frame / MAX_PAYLOAD bytes for control even during a data flood.
        let frame_limit = if control { MAX_FRAMES } else { MAX_FRAMES - 1 };
        let byte_limit = if control {
            MAX_STREAM_BYTES
        } else {
            MAX_STREAM_BYTES - MAX_PAYLOAD
        };
        let pending = self.sent - self.acked;
        if self.queue.len() >= frame_limit || pending + bytes.len() as u64 > byte_limit as u64 {
            // Control saturation forces disconnect/resync; the caller retains the event.
            self.closed |= control;
            return Err(("backpressure", event)); // Return ownership; never silently drop.
        }
        self.sent = self
            .sent
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| ("sequence exhausted", event.clone()))?;
        self.queue.push_back(StreamFrame {
            end: self.sent,
            event,
        });
        Ok(())
    }

    pub fn ack(&mut self, end: u64) -> Result<(), &'static str> {
        if self.closed || end <= self.acked || !self.queue.iter().any(|frame| frame.end == end) {
            return Err("ack");
        }
        self.acked = end;
        self.queue.retain(|frame| frame.end > end);
        Ok(())
    }
}

#[derive(Default)]
pub struct ProcessGroup {
    pub target: String,
    pub cancel_requested: bool,
    pub terminal: bool,
    pub cleanup_ack: bool,
}

impl ProcessGroup {
    pub fn cancel(&mut self, target: &str) -> Result<(), &'static str> {
        if target != self.target {
            return Err("target");
        }
        if !self.terminal {
            self.cancel_requested = true;
        }
        Ok(())
    }

    pub fn exited(&mut self) {
        self.terminal = true;
    }

    pub fn cleanup(&mut self, host_confirmed_group_gone: bool) {
        self.cleanup_ack |= host_confirmed_group_gone;
        self.terminal |= self.cleanup_ack;
    }
}

pub struct Provider {
    pub state: Auth,
    pub attempt: u64,
    pub connected: bool,
}

impl Provider {
    /// An existing revoked reference is a tombstone: new consent needs a new reference.
    pub fn host_auth(&mut self, attempt: u64, next: Auth) {
        if attempt != self.attempt {
            return;
        }
        if !matches!(
            self.state,
            Auth::Revoked | Auth::Denied | Auth::Expired | Auth::Cancelled
        ) {
            self.state = next;
        }
        if self.state != Auth::Authorized {
            self.connected = false;
        }
    }

    pub fn connect(&mut self) -> Result<(), &'static str> {
        if self.state != Auth::Authorized {
            return Err("auth");
        }
        self.connected = true;
        Ok(())
    }

    pub fn reconnect(&mut self) {
        self.connected = false;
        self.attempt = self.attempt.checked_add(1).expect("auth attempt exhausted");
        if self.state == Auth::Authorized {
            self.state = Auth::ReauthorizationRequired;
        }
    }
}

/// Persist this entire state atomically before using accepted policy.
#[derive(Clone, Default)]
pub struct UpdatePolicy {
    pub minimum_cef: [u32; 4],
    policy: Option<SecurityPolicy>,
    revocations: Option<Revocations>,
    revoked: BTreeSet<String>,
    used_rollbacks: BTreeSet<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateScope {
    pub app: String,
    pub platform: String,
    pub channel: String,
}

/// Facts supplied by the independent metadata verifier, never the renderer/bundle.
#[derive(Clone)]
pub struct Metadata {
    pub sequence: u64,
    pub expires: u64,
    pub scope: UpdateScope,
    pub verified: bool,
}

#[derive(Clone)]
pub struct SecurityPolicy {
    pub metadata: Metadata,
    pub floor: [u32; 4],
    pub approved_cef: BTreeSet<([u32; 4], String, String)>,
}

#[derive(Clone)]
pub struct Revocations {
    pub metadata: Metadata,
    /// Exact identities in this fixture share one namespace.
    pub identities: BTreeSet<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    /// Trusted application release ordering, independent of CEF version.
    pub order: u64,
    pub manifest_digest: String,
}

#[derive(Clone)]
pub struct Bundle {
    pub release: Release,
    pub scope: UpdateScope,
    pub cef: [u32; 4],
    pub revision: String,
    pub artifact: String,
    pub signing_key: String,
    pub verified: bool,
    pub complete: bool,
    pub compatible: bool,
}

#[derive(Clone)]
pub struct Rollback {
    pub id: String,
    pub source: Release,
    pub target: Release,
    pub scope: UpdateScope,
    pub failure: String,
    pub expires: u64,
    pub verified: bool,
    pub rollback_role: bool,
    pub authority_key: String,
}

impl UpdatePolicy {
    fn metadata_valid(metadata: &Metadata, scope: &UpdateScope, now: u64) -> bool {
        metadata.verified && metadata.scope == *scope && now < metadata.expires
    }

    /// Caller supplies the trusted host scope; failed refreshes never replace the cache.
    pub fn policy(
        &mut self,
        policy: SecurityPolicy,
        scope: &UpdateScope,
        now: u64,
    ) -> Result<(), &'static str> {
        if !Self::metadata_valid(&policy.metadata, scope, now)
            || self.policy.as_ref().is_some_and(|old| {
                old.metadata.scope != policy.metadata.scope
                    || policy.metadata.sequence <= old.metadata.sequence
            })
            || self
                .revocations
                .as_ref()
                .is_some_and(|old| old.metadata.scope != *scope)
        {
            return Err("policy metadata");
        }
        self.minimum_cef = self.minimum_cef.max(policy.floor);
        self.policy = Some(policy);
        Ok(())
    }

    pub fn revoke(
        &mut self,
        revocations: Revocations,
        scope: &UpdateScope,
        now: u64,
    ) -> Result<(), &'static str> {
        if !Self::metadata_valid(&revocations.metadata, scope, now)
            || self.revocations.as_ref().is_some_and(|old| {
                old.metadata.scope != revocations.metadata.scope
                    || revocations.metadata.sequence <= old.metadata.sequence
            })
            || self
                .policy
                .as_ref()
                .is_some_and(|old| old.metadata.scope != *scope)
        {
            return Err("revocation metadata");
        }
        self.revoked.extend(revocations.identities.iter().cloned());
        self.revocations = Some(revocations);
        Ok(())
    }

    pub fn sequences(&self) -> (Option<u64>, Option<u64>) {
        (
            self.policy.as_ref().map(|p| p.metadata.sequence),
            self.revocations.as_ref().map(|r| r.metadata.sequence),
        )
    }

    /// Shared by staging, activation, and startup; no cached staging approval.
    pub fn check_bundle(
        &self,
        bundle: &Bundle,
        scope: &UpdateScope,
        now: u64,
    ) -> Result<(), &'static str> {
        let policy = self.policy.as_ref().ok_or("policy metadata")?;
        let revocations = self.revocations.as_ref().ok_or("revocation metadata")?;
        if !Self::metadata_valid(&policy.metadata, scope, now)
            || !Self::metadata_valid(&revocations.metadata, scope, now)
        {
            return Err("expired or scoped metadata");
        }
        if !bundle.verified
            || !bundle.complete
            || !bundle.compatible
            || bundle.scope != *scope
            || bundle.cef < self.minimum_cef
            || !policy.approved_cef.contains(&(
                bundle.cef,
                bundle.revision.clone(),
                bundle.artifact.clone(),
            ))
            || [
                &bundle.release.manifest_digest,
                &bundle.revision,
                &bundle.artifact,
                &bundle.signing_key,
            ]
            .iter()
            .any(|id| id.is_empty() || self.revoked.contains(*id))
        {
            return Err("bundle");
        }
        Ok(())
    }

    /// `current` and `failure` are trusted installed-state/health facts, not candidate claims.
    pub fn preflight(
        &self,
        bundle: &Bundle,
        current: &Bundle,
        failure: Option<&str>,
        rollback: Option<&Rollback>,
        now: u64,
    ) -> Result<(), &'static str> {
        self.check_bundle(bundle, &current.scope, now)?;
        if bundle.release.order == current.release.order && bundle.release != current.release {
            return Err("release identity");
        }
        if bundle.release.order < current.release.order {
            let auth = rollback.ok_or("rollback")?;
            if !auth.verified
                || !auth.rollback_role
                || now >= auth.expires
                || auth.source != current.release
                || auth.target != bundle.release
                || auth.scope != current.scope
                || auth.failure.is_empty()
                || failure != Some(auth.failure.as_str())
                || auth.id.is_empty()
                || auth.authority_key.is_empty()
                || self.revoked.contains(&auth.id)
                || self.revoked.contains(&auth.authority_key)
                || self.used_rollbacks.contains(&auth.id)
            {
                return Err("rollback");
            }
        }
        Ok(())
    }

    /// Pure decision/consumption only: real atomic persistence and bundle replacement are not modeled.
    pub fn install(
        &mut self,
        bundle: &Bundle,
        current: &Bundle,
        failure: Option<&str>,
        rollback: Option<&Rollback>,
        now: u64,
    ) -> Result<(), &'static str> {
        self.preflight(bundle, current, failure, rollback, now)?;
        if bundle.release.order < current.release.order {
            self.used_rollbacks
                .insert(rollback.ok_or("rollback")?.id.clone());
        }
        Ok(())
    }
}
