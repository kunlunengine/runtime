//! Artifact paths are canonical portable names, not host-native path strings.

use std::path::{Component, Path};

pub fn is_portable_relative_path(value: &str) -> bool {
    value.split('/').all(|part| {
        if part.is_empty()
            || matches!(part, "." | "..")
            || part.ends_with(['.', ' '])
            || part
                .chars()
                .any(|c| c.is_control() || r#"<>:"\|?*"#.contains(c))
        {
            return false;
        }
        // Win32 aliases device names even with an extension or in a subdirectory.
        let stem = part.split('.').next().unwrap().to_ascii_uppercase();
        !matches!(
            stem.as_str(),
            "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
        ) && !["COM", "LPT"].iter().any(|prefix| {
            stem.strip_prefix(prefix).is_some_and(|suffix| {
                matches!(
                    suffix,
                    "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                )
            })
        })
    })
}

/// Encode filesystem-relative paths exactly like Python's `Path.as_posix()`.
/// Reject ambiguous names instead of interpreting POSIX backslashes as separators.
#[allow(dead_code)] // The manifest validator only checks strings; Cargo also inventories files.
pub fn inventory_path(path: &Path) -> Option<String> {
    let parts: Option<Vec<_>> = path
        .components()
        .map(|component| match component {
            Component::Normal(part) => part.to_str(),
            _ => None,
        })
        .collect();
    let value = parts?.join("/");
    is_portable_relative_path(&value).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_canonical_portable_names() {
        for value in [
            "include/kunlun_jsc.h",
            "lib/JavaScriptCore.dll",
            "licenses/01-WebKit.txt",
            "metadata/build.json",
            "目录/模块.mjs",
            "lib/com10.dll",
        ] {
            assert!(is_portable_relative_path(value), "{value:?}");
        }
    }

    #[test]
    fn rejects_host_dependent_and_aliasing_names() {
        for value in [
            "",
            "/lib/jsc.dll",
            "C:/lib/jsc.dll",
            "C:lib/jsc.dll",
            r"lib\jsc.dll",
            r"\\server\share",
            "lib/../escape",
            "lib/./jsc.dll",
            "lib//jsc.dll",
            "lib/",
            "lib/jsc.dll:payload",
            "lib/jsc.dll.",
            "lib/jsc.dll ",
            "lib/NUL.dll",
            "lib/con",
            "lib/COM1.dll",
            "lib/lpt9.txt",
            "lib/COM¹.dll",
            "lib/CONOUT$",
            "lib/a?b",
            "lib/a\u{0}b",
            "lib/a\nb",
        ] {
            assert!(!is_portable_relative_path(value), "{value:?}");
        }
    }

    #[test]
    fn inventory_uses_forward_slashes_on_every_host() {
        let path = Path::new("metadata").join("nested").join("build.json");
        assert_eq!(
            inventory_path(&path).as_deref(),
            Some("metadata/nested/build.json")
        );
    }
}
