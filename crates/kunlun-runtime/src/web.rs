//! M2 synchronous primitives. Only owned strings/bytes cross the JSC boundary.
use kunlun_jsc::{CallbackReturn, JscError, JscVm};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256, Sha384, Sha512};
use std::{cell::RefCell, rc::Rc};
use url::Url;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsoleRecord {
    pub level: String,
    pub message: String,
    pub source: String,
}

pub(crate) type ConsoleSink = Rc<RefCell<Box<dyn Fn(&ConsoleRecord)>>>;

pub(crate) fn install(vm: &mut JscVm, sink: ConsoleSink) -> Result<(), JscError> {
    vm.install_global_callback("__kunlunWeb", move |args| {
        let operation = args
            .first()
            .ok_or("missing operation")?
            .to_string()
            .map_err(|e| e.to_string())?;
        let payload = args
            .get(1)
            .ok_or("missing payload")?
            .to_string()
            .map_err(|e| e.to_string())?;
        let result = dispatch(&operation, &payload, &sink);
        // Preserve Web API error classes in the JavaScript adapter.
        Ok(CallbackReturn::String(
            match result {
                Ok(value) => json!({"value":value}),
                Err(error) => json!({"error":error}),
            }
            .to_string(),
        ))
    })?;
    vm.evaluate(
        include_str!("web/vendor/streams.js"),
        "kunlun:bootstrap/streams",
    )?;
    vm.evaluate(include_str!("web/globals.js"), "kunlun:bootstrap/web")?;
    Ok(())
}

fn dispatch(operation: &str, payload: &str, sink: &ConsoleSink) -> Result<Value, String> {
    let v: Value = serde_json::from_str(payload).map_err(|e| e.to_string())?;
    match operation {
        "console" => {
            let mut record: ConsoleRecord = serde_json::from_value(v).map_err(|e| e.to_string())?;
            truncate(&mut record.message, 8192);
            truncate(&mut record.source, 2048);
            (sink.borrow())(&record);
            Ok(Value::Null)
        }
        "random" => {
            let length = v
                .as_u64()
                .filter(|n| *n <= 65536)
                .ok_or("invalid random length")?;
            let mut bytes = vec![0; length as usize];
            getrandom::fill(&mut bytes).map_err(|e| e.to_string())?;
            Ok(json!(bytes))
        }
        "digest" => {
            let bytes: Vec<u8> =
                serde_json::from_value(v["bytes"].clone()).map_err(|e| e.to_string())?;
            let digest = match v["algorithm"].as_str() {
                Some("SHA-256") => Sha256::digest(&bytes).to_vec(),
                Some("SHA-384") => Sha384::digest(&bytes).to_vec(),
                Some("SHA-512") => Sha512::digest(&bytes).to_vec(),
                _ => return Err("unsupported digest".into()),
            };
            Ok(json!(digest))
        }
        "decode" => {
            let bytes: Vec<u8> =
                serde_json::from_value(v["bytes"].clone()).map_err(|e| e.to_string())?;
            let mut remaining = bytes.as_slice();
            let mut text = String::new();
            loop {
                match std::str::from_utf8(remaining) {
                    Ok(valid) => {
                        text.push_str(valid);
                        remaining = &[];
                        break;
                    }
                    Err(error) => {
                        let (valid, rest) = remaining.split_at(error.valid_up_to());
                        text.push_str(std::str::from_utf8(valid).map_err(|e| e.to_string())?);
                        if error.error_len().is_none() && v["stream"] == true {
                            remaining = rest;
                            break;
                        }
                        if v["fatal"] == true {
                            return Err("The encoded data was not valid UTF-8".into());
                        }
                        text.push('\u{fffd}');
                        remaining = &rest[error.error_len().unwrap_or(rest.len())..];
                    }
                }
            }
            Ok(json!({"text":text,"pending":remaining}))
        }
        "params.parse" => Ok(json!(
            url::form_urlencoded::parse(v.as_str().ok_or("expected string")?.as_bytes())
                .collect::<Vec<_>>()
        )),
        "params.serialize" => {
            let pairs: Vec<(String, String)> =
                serde_json::from_value(v).map_err(|e| e.to_string())?;
            Ok(json!(
                url::form_urlencoded::Serializer::new(String::new())
                    .extend_pairs(pairs)
                    .finish()
            ))
        }
        "url" => {
            let input = v["input"].as_str().ok_or("expected URL")?;
            let base = v["base"]
                .as_str()
                .map(Url::parse)
                .transpose()
                .map_err(|e| e.to_string())?;
            let mut url = Url::options()
                .base_url(base.as_ref())
                .parse(input)
                .map_err(|e| e.to_string())?;
            if let Some(field) = v["field"].as_str() {
                let value = v["value"].as_str().ok_or("expected setter value")?;
                match field {
                    "protocol" => {
                        let _ = url::quirks::set_protocol(&mut url, value);
                    }
                    "username" => {
                        let _ = url::quirks::set_username(&mut url, value);
                    }
                    "password" => {
                        let _ = url::quirks::set_password(&mut url, value);
                    }
                    "host" => {
                        let _ = url::quirks::set_host(&mut url, value);
                    }
                    "hostname" => {
                        let _ = url::quirks::set_hostname(&mut url, value);
                    }
                    "port" => {
                        let _ = url::quirks::set_port(&mut url, value);
                    }
                    "pathname" => {
                        url::quirks::set_pathname(&mut url, value);
                    }
                    "search" => {
                        url::quirks::set_search(&mut url, value);
                    }
                    "hash" => {
                        url::quirks::set_hash(&mut url, value);
                    }
                    _ => return Err("unsupported URL setter".into()),
                }
            }
            let hostname = url.host().map(|h| h.to_string()).unwrap_or_default();
            let host = match url.port() {
                Some(port) => format!("{hostname}:{port}"),
                None => hostname.clone(),
            };
            Ok(
                json!({"href":url.as_str(), "origin":url.origin().ascii_serialization(), "protocol":format!("{}:",url.scheme()), "username":url.username(), "password":url.password().unwrap_or_default(), "host":host, "hostname":hostname, "port":url.port().map(|p|p.to_string()).unwrap_or_default(), "pathname":url.path(), "search":url.query().filter(|s|!s.is_empty()).map(|q|format!("?{q}")).unwrap_or_default(), "hash":url.fragment().filter(|s|!s.is_empty()).map(|f|format!("#{f}")).unwrap_or_default()}),
            )
        }
        _ => Err("unknown web operation".into()),
    }
}

fn truncate(value: &mut String, max: usize) {
    if value.len() > max {
        let mut end = max;
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        value.truncate(end);
    }
}
