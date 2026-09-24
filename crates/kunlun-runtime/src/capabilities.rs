//! Deployment authority projected into application and request lifetimes.
use crate::artifact::{Capability, CapabilityRequirements};
use crate::host::HostPermissions;
use std::collections::{BTreeMap, BTreeSet};
use std::marker::PhantomData;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// The admitted application's exact declaration/deployment intersection.
/// Its host permissions are used for M3 built-ins and are revoked on drop.
pub struct ApplicationAuthority {
    permissions: HostPermissions,
    effective: BTreeSet<Capability>,
    active: Arc<AtomicBool>,
    claimed: AtomicBool,
}

impl ApplicationAuthority {
    pub(crate) fn new(deployment: &HostPermissions, declarations: &CapabilityRequirements) -> Self {
        let active = Arc::new(AtomicBool::new(true));
        let (permissions, effective) = deployment.scoped_to(declarations, Arc::clone(&active));
        Self {
            permissions,
            effective,
            active,
            claimed: AtomicBool::new(false),
        }
    }

    /// Optional declarations without deployment grants are absent.
    pub fn contains(&self, capability: &Capability) -> bool {
        self.active.load(Ordering::Acquire) && self.effective.contains(capability)
    }

    /// Begin one request with context owned exclusively by that request.
    pub fn begin_request(
        &self,
        context: RequestContext,
    ) -> Result<RequestEnvironment, &'static str> {
        if !self.active.load(Ordering::Acquire) {
            return Err("application authority has ended");
        }
        if !self.claimed.load(Ordering::Acquire) {
            return Err("application authority has no isolate");
        }
        let active = Arc::new(AtomicBool::new(true));
        Ok(RequestEnvironment {
            permissions: self.permissions.for_request(Arc::clone(&active)),
            effective: self.effective.clone(),
            active,
            application_active: Arc::clone(&self.active),
            context,
            thread_affine: PhantomData,
        })
    }

    pub(crate) fn claim_isolate(&self) -> Result<Arc<AtomicBool>, &'static str> {
        if !self.active.load(Ordering::Acquire) {
            return Err("application authority has ended");
        }
        self.claimed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| "application authority already belongs to an isolate")?;
        Ok(Arc::clone(&self.active))
    }

    /// Stop all new privileged operations, including those on retained request handles.
    pub fn revoke(&self) {
        self.active.store(false, Ordering::Release);
    }

    pub(crate) fn host_permissions(&self) -> HostPermissions {
        self.permissions.clone()
    }
}

impl Drop for ApplicationAuthority {
    fn drop(&mut self) {
        self.revoke();
    }
}

/// Trusted caller data. Values remain owned by one request and are not part of
/// the artifact, authority handle, diagnostics, or automatic Debug output.
#[derive(Default)]
pub struct RequestContext {
    values: BTreeMap<String, String>,
}

impl RequestContext {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.values.insert(name.into(), value.into());
    }
}

/// Request-owned projection. Dropping it revokes every handle obtained from it.
pub struct RequestEnvironment {
    permissions: HostPermissions,
    effective: BTreeSet<Capability>,
    active: Arc<AtomicBool>,
    application_active: Arc<AtomicBool>,
    context: RequestContext,
    thread_affine: PhantomData<Rc<()>>,
}

impl RequestEnvironment {
    /// Return a handle only for an effective declared grant.
    pub fn handle(&self, capability: &Capability) -> Option<ScopedHandle> {
        if !self.is_active() || !self.effective.contains(capability) {
            return None;
        }
        Some(ScopedHandle {
            capability: capability.clone(),
            request_active: Arc::clone(&self.active),
            application_active: Arc::clone(&self.application_active),
            thread_affine: PhantomData,
        })
    }

    /// Read a caller value only from its owning request environment.
    pub fn context_value(&self, name: &str) -> Option<&str> {
        self.is_active()
            .then(|| self.context.values.get(name).map(String::as_str))
            .flatten()
    }

    pub fn revoke(&self) {
        self.active.store(false, Ordering::Release);
    }

    fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire) && self.application_active.load(Ordering::Acquire)
    }
}

impl Drop for RequestEnvironment {
    fn drop(&mut self) {
        self.revoke();
    }
}

/// Opaque request-bound authority. Its fields cannot be constructed by an
/// artifact or serialized into a browser bundle.
pub struct ScopedHandle {
    capability: Capability,
    request_active: Arc<AtomicBool>,
    application_active: Arc<AtomicBool>,
    thread_affine: PhantomData<Rc<()>>,
}

impl ScopedHandle {
    fn check_owner(&self, environment: &RequestEnvironment) -> Result<(), &'static str> {
        if !Arc::ptr_eq(&self.request_active, &environment.active)
            || !Arc::ptr_eq(&self.application_active, &environment.application_active)
            || !environment.is_active()
        {
            return Err("capability handle is expired or belongs to another request");
        }
        Ok(())
    }

    /// Check one HTTP destination; callers must repeat this for every redirect.
    pub fn authorize_http(
        &self,
        environment: &RequestEnvironment,
        url: &str,
    ) -> Result<(), String> {
        self.check_owner(environment).map_err(str::to_owned)?;
        if self.capability.name != "http.host" {
            return Err("handle does not grant HTTP access".to_owned());
        }
        environment
            .permissions
            .authorize_scoped_url(&self.capability.resource, url)
    }

    /// Check a binding-relative regular file without exposing its host path.
    pub fn authorize_read(
        &self,
        environment: &RequestEnvironment,
        path: &Path,
    ) -> Result<(), String> {
        self.check_owner(environment).map_err(str::to_owned)?;
        if self.capability.name != "fs.binding" {
            return Err("handle does not grant filesystem access".to_owned());
        }
        environment
            .permissions
            .authorize_binding_read(&self.capability.resource, path)
    }
}
