use kunlun_jsc::ModuleLoader;
use kunlun_runtime::ModuleSources;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "kunlun-native-modules-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        Self(root.canonicalize().unwrap())
    }
    fn write(&self, name: &str, source: &str) {
        let path = self.0.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, source).unwrap();
    }
    fn sources(&self) -> ModuleSources {
        ModuleSources::new(&self.0).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn sources_preserve_query_identity_and_deny_unregistered_generated_modules() {
    let f = Fixture::new();
    f.write("entry.mjs", "export const answer = 42;");
    let mut sources = f.sources();
    let entry = sources.resolve("entry.mjs", None).unwrap();
    let query = sources.resolve("./entry.mjs?v=2", Some(&entry)).unwrap();
    assert_ne!(entry, query);
    assert_eq!(
        sources.fetch(&entry).unwrap(),
        sources.fetch(&query).unwrap()
    );
    assert!(
        sources
            .fetch("kunlun-generated:///unregistered.mjs")
            .is_err()
    );
    sources
        .register_generated("kunlun-generated:///a.mjs", "export default 1")
        .unwrap();
    assert!(
        sources
            .register_generated("kunlun-generated:///./a.mjs", "export default 2")
            .is_err()
    );
    assert_eq!(
        sources.fetch("kunlun-generated:///a.mjs").unwrap(),
        "export default 1"
    );
}

#[test]
fn source_fetch_rejects_symlink_replacement_and_invalid_utf8() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    let outside = Fixture::new();
    f.write("entry.mjs", "export default 1;");
    outside.write("secret.mjs", "export default 'secret';");
    let sources = f.sources();
    let entry = sources.resolve("entry.mjs", None).unwrap();
    std::fs::remove_file(f.0.join("entry.mjs")).unwrap();
    symlink(outside.0.join("secret.mjs"), f.0.join("entry.mjs")).unwrap();
    assert!(sources.fetch(&entry).is_err());
    std::fs::write(f.0.join("invalid.mjs"), [0xff]).unwrap();
    let url = sources.resolve("invalid.mjs", None).unwrap();
    assert!(sources.fetch(&url).unwrap_err().contains("UTF-8"));
}

#[test]
fn source_budgets_bound_files_generated_sources_and_query_identities() {
    let f = Fixture::new();
    let limit = 8 * 1024 * 1024;
    f.write("large.mjs", &" ".repeat(limit + 1));
    let sources = f.sources();
    let url = sources.resolve("large.mjs", None).unwrap();
    assert!(sources.fetch(&url).unwrap_err().contains("byte limit"));
    f.write("large.mjs", &" ".repeat(limit));
    for index in 0..8 {
        let key = sources
            .resolve(&format!("./large.mjs?v={index}"), Some(&url))
            .unwrap();
        assert_eq!(sources.fetch(&key).unwrap().len(), limit);
    }
    assert!(sources.fetch(&url).unwrap_err().contains("64 MiB"));

    let mut generated = f.sources();
    assert!(
        generated
            .register_generated("kunlun-generated:///large.mjs", " ".repeat(limit + 1))
            .unwrap_err()
            .contains("8 MiB")
    );
    for index in 0..1024 {
        generated
            .register_generated(&format!("kunlun-generated:///{index}.mjs"), "")
            .unwrap();
    }
    assert!(
        generated
            .register_generated("kunlun-generated:///overflow.mjs", "")
            .unwrap_err()
            .contains("budget")
    );
}

#[cfg(feature = "system-jsc")]
#[test]
fn system_backend_rejects_native_modules() {
    use kunlun_jsc::{JscStatus, JscVm};
    let f = Fixture::new();
    let mut vm = JscVm::new("unsupported-modules").unwrap();
    assert_eq!(
        vm.install_module_loader(f.sources()).unwrap_err().status(),
        Some(JscStatus::Unsupported)
    );
    assert_eq!(vm.evaluate("1 + 1", "test:///still-valid.js").unwrap(), "2");
}

#[test]
fn source_maps_preserve_generated_locations_and_never_fetch_originals() {
    let f = Fixture::new();
    f.write(
        "entry.mjs",
        "throw Error('mapped');\n//# sourceMappingURL=entry.mjs.map",
    );
    f.write("entry.mjs.map", r#"{"version":3,"sources":["https://example.invalid/original.ts"],"names":[],"mappings":"AAAA"}"#);
    let sources = f.sources();
    let url = sources.resolve("entry.mjs", None).unwrap();
    sources.fetch(&url).unwrap();
    let generated = format!("Error: mapped\nmodule code@{url}:1:12");
    let mapped = sources.map_error(&generated);
    assert!(mapped.starts_with(&generated));
    assert!(mapped.contains("https://example.invalid/original.ts:1:1"));
    assert_eq!(
        sources.map_error(&format!("Error: unknown\nmodule code@{url}:2:1")),
        format!("Error: unknown\nmodule code@{url}:2:1")
    );
}

#[test]
fn malformed_and_outside_source_maps_preserve_the_original_error() {
    let f = Fixture::new();
    let outside = Fixture::new();
    outside.write(
        "map.json",
        r#"{"version":3,"sources":["secret.ts"],"names":[],"mappings":"AAAA"}"#,
    );
    for reference in [
        "https://example.invalid/map.json".to_owned(),
        url::Url::from_file_path(outside.0.join("map.json"))
            .unwrap()
            .to_string(),
        "invalid.map".to_owned(),
    ] {
        f.write(
            "entry.mjs",
            &format!("throw Error('original');\n//# sourceMappingURL={reference}"),
        );
        f.write("invalid.map", "not a source map");
        let sources = f.sources();
        let url = sources.resolve("entry.mjs", None).unwrap();
        sources.fetch(&url).unwrap();
        let generated = format!("Error: original\nmodule code@{url}:1:1");
        assert_eq!(sources.map_error(&generated), generated);
    }
}

#[cfg(feature = "bundled-jsc")]
mod native {
    use super::*;
    use kunlun_jsc::{JscVm, ModuleState};
    use kunlun_runtime::{HostPermissions, TokioIsolate};
    use std::future::{Future, poll_fn};
    use std::task::Poll;
    use std::time::Duration;
    use tokio::runtime::{Builder, Runtime};

    fn runtime() -> Runtime {
        Builder::new_current_thread().enable_all().build().unwrap()
    }
    fn isolate(f: &Fixture) -> TokioIsolate {
        let mut isolate = TokioIsolate::new("native-modules-test").unwrap();
        isolate.install_module_sources(f.sources()).unwrap();
        isolate
    }
    fn run(
        rt: &Runtime,
        isolate: &mut TokioIsolate,
        entry: &str,
    ) -> Result<(), kunlun_runtime::RuntimeError> {
        rt.block_on(async {
            tokio::time::timeout(Duration::from_secs(10), isolate.evaluate_module(entry))
                .await
                .expect("module evaluation stalled")
        })
    }

    #[test]
    fn native_graphs_preserve_cycles_live_bindings_and_dynamic_cache_identity() {
        let f = Fixture::new();
        f.write("a.mjs", "import { readB } from './b.mjs'; export let value = 1; export function bump() { value++; } export const read = () => readB(); globalThis.aRuns = (globalThis.aRuns || 0) + 1;");
        f.write(
            "b.mjs",
            "import { value } from './a.mjs'; export const readB = () => value;",
        );
        f.write("entry.mjs", "import * as a from './a.mjs'; a.bump(); const [first, second] = await Promise.all([import('./a.mjs'), import('./a.mjs')]); globalThis.result = [a.read(), a.value, first === second, first === a, aRuns].join(','); globalThis.entryUrl = import.meta.url;");
        let rt = runtime();
        let mut isolate = isolate(&f);
        run(&rt, &mut isolate, "entry.mjs").unwrap();
        assert_eq!(
            isolate.evaluate("result", "test:///check.js").unwrap(),
            "2,2,true,true,1"
        );
        assert_eq!(
            isolate.evaluate("entryUrl", "test:///check.js").unwrap(),
            f.sources().resolve("entry.mjs", None).unwrap()
        );
        run(&rt, &mut isolate, "entry.mjs").unwrap();
        assert_eq!(isolate.evaluate("aRuns", "test:///check.js").unwrap(), "1");
    }

    #[test]
    fn tla_drives_host_completions_and_keeps_builtin_permissions() {
        let f = Fixture::new();
        f.write("message.txt", "host completion");
        let path = serde_json::to_string(f.0.join("message.txt").to_str().unwrap()).unwrap();
        f.write("entry.mjs", &format!("import {{ readTextFile }} from 'kunlun:fs'; await sleep(0); globalThis.result = await readTextFile({path});"));
        let rt = runtime();
        let permissions = HostPermissions::none().allow_read_root(&f.0).unwrap();
        let mut permitted =
            TokioIsolate::new_with_permissions("permitted-modules", permissions).unwrap();
        permitted.install_module_sources(f.sources()).unwrap();
        run(&rt, &mut permitted, "entry.mjs").unwrap();
        assert_eq!(
            permitted.evaluate("result", "test:///check.js").unwrap(),
            "host completion"
        );
        let mut denied = isolate(&f);
        assert!(
            run(&rt, &mut denied, "entry.mjs")
                .unwrap_err()
                .to_string()
                .contains("read access denied")
        );
    }

    #[test]
    fn generated_modules_resolve_relative_imports_and_reject_dynamic_failures() {
        let f = Fixture::new();
        let mut sources = f.sources();
        sources
            .register_generated("kunlun-generated:///nested/value.mjs", "export default 42;")
            .unwrap();
        sources.register_generated("kunlun-generated:///nested/entry.mjs", "import answer from './value.mjs'; const errors = []; for (const key of ['./missing.mjs', 'https://example.test/a.mjs', 'node:fs']) { try { await import(key); } catch (error) { errors.push(String(error)); } } globalThis.result = answer + ':' + errors.length; globalThis.url = import.meta.url;").unwrap();
        let rt = runtime();
        let mut isolate = TokioIsolate::new("generated-modules").unwrap();
        isolate.install_module_sources(sources).unwrap();
        run(&rt, &mut isolate, "kunlun-generated:///nested/entry.mjs").unwrap();
        assert_eq!(
            isolate.evaluate("result", "test:///check.js").unwrap(),
            "42:3"
        );
        assert_eq!(
            isolate.evaluate("url", "test:///check.js").unwrap(),
            "kunlun-generated:///nested/entry.mjs"
        );
    }

    #[test]
    fn static_parse_link_evaluation_and_tla_failures_propagate() {
        let rt = runtime();
        for (source, message) in [
            ("export const = ;", "SyntaxError"),
            ("import { missing } from './value.mjs';", "missing"),
            ("throw new Error('sync module error');", "sync module error"),
            (
                "await sleep(0); throw new Error('tla module error');",
                "tla module error",
            ),
            ("import './absent.mjs';", "absent.mjs"),
        ] {
            let f = Fixture::new();
            f.write("value.mjs", "export const value = 1;");
            f.write("entry.mjs", source);
            let mut isolate = isolate(&f);
            let error = run(&rt, &mut isolate, "entry.mjs").unwrap_err();
            assert!(error.to_string().contains(message), "{error}");
        }
    }

    #[test]
    fn dynamic_import_rejects_fetch_parse_link_evaluation_and_tla_failures() {
        let f = Fixture::new();
        f.write("parse.mjs", "export const = ;");
        f.write("value.mjs", "export const value = 1;");
        f.write("link.mjs", "import { missing } from './value.mjs';");
        f.write("evaluate.mjs", "throw Error('dynamic evaluation');");
        f.write("tla.mjs", "await sleep(0); throw Error('dynamic tla');");
        f.write("entry.mjs", "globalThis.errors = []; for (const name of ['absent', 'parse', 'link', 'evaluate', 'tla']) { for (let attempt = 0; attempt < 2; attempt++) { try { await import('./' + name + '.mjs'); errors.push('unexpected success'); } catch (e) { errors.push(String(e)); } } }");
        let rt = runtime();
        let mut isolate = isolate(&f);
        run(&rt, &mut isolate, "entry.mjs").unwrap();
        let errors: Vec<String> = serde_json::from_str(
            &isolate
                .evaluate("JSON.stringify(errors)", "test:///check.js")
                .unwrap(),
        )
        .unwrap();
        assert_eq!(errors.len(), 10);
        for (pair, expected) in errors.chunks_exact(2).zip([
            "absent.mjs",
            "SyntaxError",
            "missing",
            "dynamic evaluation",
            "dynamic tla",
        ]) {
            assert!(
                pair.iter().all(|error| error.contains(expected)),
                "{pair:?}"
            );
        }
    }

    #[test]
    fn cancelling_tla_retires_the_isolate_and_releases_host_work() {
        let f = Fixture::new();
        f.write(
            "entry.mjs",
            "await sleep(1000000); globalThis.mustNotRun = true;",
        );
        let rt = runtime();
        let mut isolate = isolate(&f);
        rt.block_on(async {
            let mut evaluation = Box::pin(isolate.evaluate_module("entry.mjs"));
            poll_fn(|cx| {
                assert!(evaluation.as_mut().poll(cx).is_pending());
                Poll::Ready(())
            })
            .await;
            drop(evaluation);
        });
        assert!(
            isolate
                .evaluate("1", "test:///retired.js")
                .unwrap_err()
                .to_string()
                .contains("discard this isolate")
        );
        assert!(
            run(&rt, &mut isolate, "entry.mjs")
                .unwrap_err()
                .to_string()
                .contains("discard this isolate")
        );
    }

    #[test]
    fn module_handles_survive_gc_and_cannot_evaluate_twice() {
        for _ in 0..32 {
            let f = Fixture::new();
            f.write("entry.mjs", "globalThis.result = 42;");
            let mut vm = JscVm::new("module-roots").unwrap();
            vm.install_module_loader(f.sources()).unwrap();
            let mut record = vm.load_module("entry.mjs").unwrap();
            vm.microtask_checkpoint().unwrap();
            assert_eq!(record.poll().unwrap(), ModuleState::Fulfilled);
            vm.collect_garbage().unwrap();
            record.evaluate().unwrap();
            vm.microtask_checkpoint().unwrap();
            assert_eq!(record.poll().unwrap(), ModuleState::Fulfilled);
            assert!(record.evaluate().is_err());
            drop(record);
            assert_eq!(vm.evaluate("result", "test:///check.js").unwrap(), "42");
        }
    }

    #[test]
    fn module_exceptions_retain_generated_and_mapped_source_identity() {
        use base64::Engine;
        let rt = runtime();
        let f = Fixture::new();
        let map = r#"{"version":3,"sources":["src/original.ts"],"names":[],"mappings":"AAAA"}"#;
        let encoded = base64::engine::general_purpose::STANDARD.encode(map);
        f.write("entry.mjs", &format!("throw Error('mapped boom');\n//# sourceMappingURL=data:application/json;base64,{encoded}"));
        let mut isolate = isolate(&f);
        let error = run(&rt, &mut isolate, "entry.mjs").unwrap_err().to_string();
        assert!(error.contains("mapped boom"), "{error}");
        assert!(error.contains("entry.mjs:1:"), "{error}");
        assert!(error.contains("src/original.ts:1:1"), "{error}");
        let mut sources = f.sources();
        sources
            .register_generated(
                "kunlun-generated:///entry.mjs",
                "throw Error('generated boom');",
            )
            .unwrap();
        sources
            .register_source_map("kunlun-generated:///entry.mjs", map)
            .unwrap();
        let mut generated = TokioIsolate::new("mapped-generated").unwrap();
        generated.install_module_sources(sources).unwrap();
        let error = run(&rt, &mut generated, "kunlun-generated:///entry.mjs")
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("kunlun-generated:///entry.mjs:1:"),
            "{error}"
        );
        assert!(
            error.contains("kunlun-generated:///src/original.ts:1:1"),
            "{error}"
        );
    }

    #[test]
    fn throwing_stack_getters_and_primitive_rejections_keep_the_original_error() {
        let rt = runtime();
        for source in [
            "throw 'primitive rejection';",
            "const e = Error('original rejection'); Object.defineProperty(e, 'stack', { get() { throw Error('secondary'); } }); throw e;",
        ] {
            let f = Fixture::new();
            f.write("entry.mjs", source);
            let mut isolate = isolate(&f);
            let error = run(&rt, &mut isolate, "entry.mjs").unwrap_err().to_string();
            assert!(error.contains("rejection"), "{error}");
            assert!(!error.contains("secondary"), "{error}");
            assert_eq!(
                isolate
                    .evaluate("1 + 1", "test:///exception-cleared.js")
                    .unwrap(),
                "2"
            );
        }
    }

    #[test]
    fn fetch_callback_panics_are_contained_and_reentry_keeps_state_live() {
        use std::cell::{Cell, RefCell};
        use std::rc::{Rc, Weak};
        struct Loader {
            owner: Rc<RefCell<Weak<JscVm>>>,
            drops: Rc<Cell<u32>>,
            panic: bool,
        }
        impl Drop for Loader {
            fn drop(&mut self) {
                self.drops.set(self.drops.get() + 1);
            }
        }
        impl ModuleLoader for Loader {
            fn resolve(&self, _: &str, _: Option<&str>) -> Result<String, String> {
                Ok("test:///entry.mjs".to_owned())
            }
            fn fetch(&self, _: &str) -> Result<String, String> {
                if self.panic {
                    panic!("fetch panic");
                }
                let owner = self.owner.borrow().upgrade().unwrap();
                assert_eq!(owner.evaluate("6 * 7", "test:///reentry.js").unwrap(), "42");
                owner.collect_garbage().unwrap();
                Ok("export const answer = 42;".to_owned())
            }
        }
        for panic in [false, true] {
            let owner = Rc::new(RefCell::new(Weak::new()));
            let drops = Rc::new(Cell::new(0));
            let mut vm = JscVm::new("module-callback-lifecycle").unwrap();
            vm.install_module_loader(Loader {
                owner: Rc::clone(&owner),
                drops: Rc::clone(&drops),
                panic,
            })
            .unwrap();
            let vm = Rc::new(vm);
            *owner.borrow_mut() = Rc::downgrade(&vm);
            let record = vm.load_module("entry").unwrap();
            vm.microtask_checkpoint().unwrap();
            if panic {
                assert!(
                    record
                        .poll()
                        .unwrap_err()
                        .to_string()
                        .contains("module callback panicked")
                );
            } else {
                assert_eq!(record.poll().unwrap(), ModuleState::Fulfilled);
            }
            drop(record);
            drop(vm);
            assert_eq!(drops.get(), 1);
        }
    }
}
