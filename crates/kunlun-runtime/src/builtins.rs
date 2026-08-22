use kunlun_jsc::{JscError, JscVm};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuiltinModuleDescriptor {
    pub specifier: &'static str,
    pub exports: &'static [&'static str],
}

pub const BUILTIN_MODULES: &[BuiltinModuleDescriptor] = &[
    BuiltinModuleDescriptor {
        specifier: "kunlun:fs",
        exports: &["readTextFile"],
    },
    BuiltinModuleDescriptor {
        specifier: "kunlun:http",
        exports: &["request"],
    },
];

pub const TYPESCRIPT_DECLARATIONS: &str = include_str!("../../../types/index.d.ts");

const BOOTSTRAP_SOURCE: &str = r#"
(() => {
  'use strict';
  const hostCall = globalThis.__kunlunHostCall;
  if (typeof hostCall !== 'function') {
    throw new Error('Kunlun host-call bridge is not installed');
  }

  const asModule = (exports) => {
    Object.defineProperty(exports, Symbol.toStringTag, { value: 'Module' });
    return Object.freeze(exports);
  };

  const fs = asModule({
    readTextFile(path) {
      return hostCall('fs.readTextFile', JSON.stringify({ path: String(path) }));
    },
  });

  const http = asModule({
    async request(url, init = {}) {
      const headers = {};
      if (init.headers != null) {
        for (const [name, value] of Object.entries(init.headers)) {
          headers[String(name)] = String(value);
        }
      }
      const encoded = await hostCall('http.request', JSON.stringify({
        url: String(url),
        method: init.method == null ? 'GET' : String(init.method),
        headers,
        body: init.body == null ? null : String(init.body),
      }));
      return JSON.parse(encoded);
    },
  });

  const modules = Object.freeze({
    'kunlun:fs': fs,
    'kunlun:http': http,
  });
  const runtime = Object.freeze({
    import(specifier) {
      const module = modules[String(specifier)];
      return module === undefined
        ? Promise.reject(new TypeError(`Unknown Kunlun built-in module: ${specifier}`))
        : Promise.resolve(module);
    },
  });

  Object.defineProperty(globalThis, 'kunlun', {
    value: runtime,
    configurable: false,
    enumerable: false,
    writable: false,
  });
  delete globalThis.__kunlunHostCall;
})();
"#;

pub(crate) fn install_builtin_modules(vm: &mut JscVm) -> Result<(), JscError> {
    vm.evaluate(BOOTSTRAP_SOURCE, "kunlun:bootstrap/builtins")?;
    Ok(())
}

pub fn is_builtin_specifier(specifier: &str) -> bool {
    BUILTIN_MODULES
        .iter()
        .any(|module| module.specifier == specifier)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declarations_cover_every_builtin_export() {
        for module in BUILTIN_MODULES {
            assert!(
                TYPESCRIPT_DECLARATIONS
                    .contains(&format!("declare module \"{}\"", module.specifier)),
                "missing declaration for {}",
                module.specifier
            );
            for export in module.exports {
                assert!(
                    TYPESCRIPT_DECLARATIONS.contains(export),
                    "missing declaration for {}::{export}",
                    module.specifier
                );
            }
        }
    }

    #[test]
    fn resolves_only_registered_builtin_specifiers() {
        assert!(is_builtin_specifier("kunlun:fs"));
        assert!(is_builtin_specifier("kunlun:http"));
        assert!(!is_builtin_specifier("node:fs"));
        assert!(!is_builtin_specifier("kunlun:unknown"));
    }
}
