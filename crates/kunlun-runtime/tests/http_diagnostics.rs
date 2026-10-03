//! Native authority diagnostics, not a portable error-code contract.
use kunlun_runtime::{
    AdmissionPolicy, HostPermissions, RequestContext, TokioIsolate, admit_artifact,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture(PathBuf);

impl Fixture {
    fn new(optional: bool) -> Self {
        let root = std::env::temp_dir().join(format!(
            "kunlun-http-diagnostics-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let source =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/runtime-manifest/v1");
        for path in [
            "manifest.json",
            "server.mjs",
            "chunks/问候% space.mjs",
            "maps/server.mjs.map",
            "assets/help/欢迎 组件.svg",
        ] {
            let destination = root.join(path);
            fs::create_dir_all(destination.parent().unwrap()).unwrap();
            fs::copy(source.join(path), destination).unwrap();
        }
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join("manifest.json")).unwrap()).unwrap();
        manifest["capabilities"] = json!({
            "required": [], "optional": []
        });
        manifest["capabilities"][if optional { "optional" } else { "required" }] =
            json!([{"name": "http.host", "resource": "allowed.test"}]);
        fs::write(
            root.join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        Self(root)
    }

    fn policy(&self) -> AdmissionPolicy {
        let digest = Sha256::digest(fs::read(self.0.join("manifest.json")).unwrap());
        AdmissionPolicy::new(
            digest
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>(),
            HostPermissions::none().allow_net_host("allowed.test"),
        )
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn manifest_authority_redacts_ambient_and_handle_denials() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    for optional in [false, true] {
        let fixture = Fixture::new(optional);
        let artifact = admit_artifact(&fixture.0, &fixture.policy()).unwrap();
        let authority = artifact.authority();
        let mut isolate = TokioIsolate::new_with_authority("http-diagnostics", authority).unwrap();
        let mut context = RequestContext::new();
        context.insert("private-context", "private-caller-marker");
        let result = rt.block_on(isolate.evaluate_request_body(
            authority.begin_request(context).unwrap(),
            r#"
            const http = await kunlun.import('kunlun:http');
            const errors = [];
            async function denied(operation, expected) {
              try { await operation(); } catch (error) {
                const message = String(error);
                if (!message.includes(expected)) throw Error('missing denial: ' + message);
                errors.push(message);
                return;
              }
              throw Error('unexpected authorization');
            }
            // Userinfo is rejected by Fetch before native dispatch; host/path
            // without userinfo exercise native Fetch authority denial as well.
            const destination = 'https://private-host.test/private-path?private-query=private-value';
            const credentials = 'https://private-user:private-password@private-host.test/private-path';
            await denied(() => http.request(destination), 'network access denied');
            await denied(() => http.request(credentials), 'network access denied');
            await denied(() => fetch(destination), 'Fetch capability denied');
            await denied(() => env.http['allowed.test'].fetch(destination), 'Fetch capability denied');
            await denied(() => fetch(credentials), 'credentials');
            await denied(() => env.http['allowed.test'].fetch(credentials), 'credentials');
            const text = errors.join('\n');
            for (const marker of ['private-host', 'private-path', 'private-query',
                                  'private-value', 'private-user', 'private-password',
                                  'private-caller-marker', 'https://']) {
              if (text.includes(marker)) throw Error('private diagnostic data leaked: ' + marker);
            }
            return 'redacted';
            "#,
            "test:///http-diagnostics.js",
        )).unwrap();
        assert_eq!(result, "redacted");
    }
}
