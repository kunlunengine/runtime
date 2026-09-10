//! Separate process: race first-use TLS initialization before any VM exists.
use kunlun_jsc::{JscVm, ModuleLoader, ModuleState};
use std::sync::{Arc, Barrier};

struct Source;
impl ModuleLoader for Source {
    fn resolve(&self, specifier: &str, _: Option<&str>) -> Result<String, String> {
        Ok(specifier.to_owned())
    }
    fn fetch(&self, _: &str) -> Result<String, String> {
        Ok("await Promise.resolve(); export const value = 42;".to_owned())
    }
}

#[test]
fn independent_threads_can_initialize_checkpoint_and_module_registries_together() {
    if !JscVm::backend_info().supports_explicit_microtask_checkpoint {
        return;
    }
    let barrier = Arc::new(Barrier::new(16));
    let threads: Vec<_> = (0..16)
        .map(|_| {
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                for _ in 0..8 {
                    let mut vm = JscVm::new("parallel-first-use").unwrap();
                    vm.install_module_loader(Source).unwrap();
                    let mut module = vm.load_module("test:///entry.mjs").unwrap();
                    vm.microtask_checkpoint().unwrap();
                    assert_eq!(module.poll().unwrap(), ModuleState::Fulfilled);
                    module.evaluate().unwrap();
                    vm.microtask_checkpoint().unwrap();
                    assert_eq!(module.poll().unwrap(), ModuleState::Fulfilled);
                    vm.evaluate("Promise.reject('thread-local');", "test:///parallel.js")
                        .unwrap();
                    vm.microtask_checkpoint().unwrap();
                    assert_eq!(vm.take_promise_rejections().len(), 1);
                }
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
}
