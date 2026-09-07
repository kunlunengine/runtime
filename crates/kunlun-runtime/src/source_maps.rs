//! Source-map metadata. Original sources and network URLs are never fetched.
use sourcemap::DecodedMap;
use std::collections::HashMap;
use url::Url;

pub(crate) const MAX_MAP_BYTES: usize = 1024 * 1024;
const MAX_TOTAL_MAP_BYTES: usize = 8 * MAX_MAP_BYTES;

#[derive(Default)]
pub(crate) struct SourceMaps {
    maps: HashMap<String, (Url, DecodedMap)>,
    bytes: usize,
}

impl SourceMaps {
    pub fn contains(&self, url: &str) -> bool {
        self.maps.contains_key(url)
    }

    pub fn insert(&mut self, module: &str, base: Url, json: &[u8]) -> Result<(), String> {
        if self.maps.contains_key(module) {
            return Err(format!("source map for {module} is already registered"));
        }
        if json.len() > MAX_MAP_BYTES || self.bytes + json.len() > MAX_TOTAL_MAP_BYTES {
            return Err("source map exceeds the 1 MiB per-map or 8 MiB per-VM limit".to_owned());
        }
        let map = sourcemap::decode_slice(json)
            .map_err(|error| format!("invalid source map for {module}: {error}"))?;
        self.maps.insert(module.to_owned(), (base, map));
        self.bytes += json.len();
        Ok(())
    }

    pub fn describe(&self, description: &str) -> String {
        let mut result = description.to_owned();
        for frame in description.lines().skip(1).take(64) {
            let Some((prefix, column)) = frame.rsplit_once(':') else {
                continue;
            };
            let Some((location, line)) = prefix.rsplit_once(':') else {
                continue;
            };
            let (Ok(line), Ok(column)) = (line.parse::<u32>(), column.parse::<u32>()) else {
                continue;
            };
            let (Some(line), Some(column)) = (line.checked_sub(1), column.checked_sub(1)) else {
                continue;
            };
            let url = location.rsplit_once('@').map_or(location, |(_, url)| url);
            let Some((base, map)) = self.maps.get(url) else {
                continue;
            };
            let Some(token) = map.lookup_token(line, column) else {
                continue;
            };
            if token.get_dst_line() != line {
                continue;
            }
            let Some(source) = token.get_source() else {
                continue;
            };
            let original = base
                .join(source)
                .map_or_else(|_| source.to_owned(), |url| url.to_string());
            result.push_str(&format!(
                "\n  mapped {url}:{}:{} -> {original}:{}:{}",
                line + 1,
                column + 1,
                u64::from(token.get_src_line()) + 1,
                u64::from(token.get_src_col()) + 1
            ));
        }
        result
    }
}
