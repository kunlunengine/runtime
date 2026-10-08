//! Bounded, duplicate-key-rejecting static input. Parser details never leave this module.

use std::{
    cell::Cell,
    collections::BTreeMap,
    fmt,
    io::Read,
    path::{Component, Path, PathBuf},
};

use cap_std::fs::{Dir, OpenOptions};
use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::protocol::{Diagnostic, ErrorCode, MAX_INPUT_BYTES, Result, SCHEMA};

const MAX_TOTAL_BYTES: usize = 32 * 1024 * 1024;
const MAX_VALUES: usize = 100_000;

pub struct Inputs {
    pub root: Dir,
    files: BTreeMap<String, Vec<u8>>,
    total: usize,
}

impl Inputs {
    pub fn new(root: &Path) -> Result<Self> {
        // Only root selection uses ambient authority. All subsequent lookups are descriptor-relative.
        let root = Dir::open_ambient_dir(root, cap_std::ambient_authority())
            .map_err(|_| Diagnostic::new(ErrorCode::ProjectUnreadable))?;
        Ok(Self {
            root,
            files: BTreeMap::new(),
            total: 0,
        })
    }

    pub fn exists(&self, path: &str) -> Result<bool> {
        let mut current = PathBuf::new();
        for component in Path::new(path).components() {
            if !matches!(component, Component::Normal(_)) {
                return Err(Diagnostic::new(ErrorCode::ProjectUnreadable));
            }
            current.push(component);
            match self.root.symlink_metadata(&current) {
                Ok(meta) if meta.is_symlink() => {
                    return Err(Diagnostic::new(ErrorCode::ProjectUnreadable));
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
                Err(_) => return Err(Diagnostic::new(ErrorCode::ProjectUnreadable)),
            }
        }
        Ok(true)
    }

    pub fn read(&mut self, path: &str) -> Result<Vec<u8>> {
        if !self.exists(path)? {
            return Err(Diagnostic::new(ErrorCode::ProjectUnreadable));
        }
        if !self
            .root
            .symlink_metadata(path)
            .map_err(|_| Diagnostic::new(ErrorCode::ProjectUnreadable))?
            .is_file()
        {
            return Err(Diagnostic::new(ErrorCode::ProjectUnreadable));
        }
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use cap_std::fs::OpenOptionsExt;
            // A regular file swapped for a FIFO must not block; a swapped leaf link must not follow.
            options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
        }
        let file = self
            .root
            .open_with(path, &options)
            .map_err(|_| Diagnostic::new(ErrorCode::ProjectUnreadable))?;
        if !file
            .metadata()
            .map_err(|_| Diagnostic::new(ErrorCode::ProjectUnreadable))?
            .is_file()
        {
            return Err(Diagnostic::new(ErrorCode::ProjectUnreadable));
        }
        let mut bytes = Vec::new();
        file.take((MAX_INPUT_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| Diagnostic::new(ErrorCode::ProjectUnreadable))?;
        if bytes.len() > MAX_INPUT_BYTES || self.total + bytes.len() > MAX_TOTAL_BYTES {
            return Err(Diagnostic::new(ErrorCode::LimitExceeded));
        }
        self.total += bytes.len();
        self.files.insert(path.into(), bytes.clone());
        Ok(bytes)
    }

    /// Raw static bytes, relative filenames and protocol semantics; never machine paths or time.
    pub fn fingerprint(&self) -> String {
        let mut digest = Sha256::new();
        digest.update(SCHEMA);
        for (path, bytes) in &self.files {
            digest.update((path.len() as u64).to_le_bytes());
            digest.update(path);
            digest.update((bytes.len() as u64).to_le_bytes());
            digest.update(bytes);
        }
        hex(&digest.finalize())
    }
}

pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(output, "{byte:02x}").expect("writing to String");
    }
    output
}

pub fn json(bytes: &[u8], code: ErrorCode) -> Result<Value> {
    let budget = Cell::new(MAX_VALUES);
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let value = Seed {
        budget: &budget,
        depth: 0,
    }
    .deserialize(&mut deserializer)
    .map_err(|_| {
        Diagnostic::new(if budget.get() == 0 {
            ErrorCode::LimitExceeded
        } else {
            code.clone()
        })
    })?;
    deserializer.end().map_err(|_| Diagnostic::new(code))?;
    Ok(value)
}

pub fn yaml(bytes: &[u8], code: ErrorCode) -> Result<Value> {
    let budget = Cell::new(MAX_VALUES);
    let mut documents = serde_yaml_ng::Deserializer::from_slice(bytes);
    let document = documents
        .next()
        .ok_or_else(|| Diagnostic::new(code.clone()))?;
    let value = Seed {
        budget: &budget,
        depth: 0,
    }
    .deserialize(document)
    .map_err(|_| {
        Diagnostic::new(if budget.get() == 0 {
            ErrorCode::LimitExceeded
        } else {
            code.clone()
        })
    })?;
    if documents.next().is_some() {
        return Err(Diagnostic::new(code));
    }
    Ok(value)
}

#[derive(Clone, Copy)]
struct Seed<'a> {
    budget: &'a Cell<usize>,
    depth: usize,
}

impl<'de> DeserializeSeed<'de> for Seed<'_> {
    type Value = Value;

    fn deserialize<D: de::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> std::result::Result<Value, D::Error> {
        if self.depth > 64 || self.budget.get() == 0 {
            self.budget.set(0);
            return Err(de::Error::custom("static input limit"));
        }
        self.budget.set(self.budget.get() - 1);
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Seed<'_> {
    type Value = Value;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("bounded static data with unique string keys")
    }

    fn visit_bool<E: de::Error>(self, value: bool) -> std::result::Result<Value, E> {
        Ok(Value::Bool(value))
    }
    fn visit_i64<E: de::Error>(self, value: i64) -> std::result::Result<Value, E> {
        Ok(value.into())
    }
    fn visit_u64<E: de::Error>(self, value: u64) -> std::result::Result<Value, E> {
        Ok(value.into())
    }
    fn visit_f64<E: de::Error>(self, value: f64) -> std::result::Result<Value, E> {
        serde_json::Number::from_f64(value)
            .map(Value::Number)
            .ok_or_else(|| E::custom("non-finite number"))
    }
    fn visit_str<E: de::Error>(self, value: &str) -> std::result::Result<Value, E> {
        if value.len() > 8192 {
            self.budget.set(0);
            return Err(E::custom("static string limit"));
        }
        Ok(Value::String(value.into()))
    }
    fn visit_string<E: de::Error>(self, value: String) -> std::result::Result<Value, E> {
        self.visit_str(&value)
    }
    fn visit_unit<E: de::Error>(self) -> std::result::Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_none<E: de::Error>(self) -> std::result::Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> std::result::Result<Value, A::Error> {
        let child = Self {
            depth: self.depth + 1,
            ..self
        };
        let mut values = Vec::new();
        while let Some(value) = seq.next_element_seed(child)? {
            values.push(value);
        }
        Ok(Value::Array(values))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> std::result::Result<Value, A::Error> {
        let child = Self {
            depth: self.depth + 1,
            ..self
        };
        let mut values = serde_json::Map::new();
        while let Some(key) = map.next_key_seed(child)? {
            let key = key
                .as_str()
                .ok_or_else(|| de::Error::custom("non-string key"))?;
            if values.contains_key(key) {
                return Err(de::Error::custom("duplicate key"));
            }
            values.insert(key.into(), map.next_value_seed(child)?);
        }
        Ok(Value::Object(values))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsers_reject_duplicate_keys_tags_and_extra_documents() {
        assert!(json(br#"{"x":{"d":1,"d":2}}"#, ErrorCode::InvalidManifest).is_err());
        assert!(yaml(b"x: {d: 1, d: 2}", ErrorCode::InvalidLockfile).is_err());
        assert!(yaml(b"x: !execute command", ErrorCode::InvalidLockfile).is_err());
        assert!(yaml(b"x: 1\n---\nx: 2", ErrorCode::InvalidLockfile).is_err());
        assert!(yaml(b"x: {1: value}", ErrorCode::InvalidLockfile).is_err());
    }
}
