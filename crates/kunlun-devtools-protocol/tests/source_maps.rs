//! MOCK ONLY: registered fixture bytes, not native Inspector/HMR qualification.
use kunlun_devtools_protocol::{
    Command, CommandResult, Error, Identity, Operation, Request, Response, ResponseResult,
    SourceDescriptor, SourceIdentity, SourceLocation, SourceMapping, UnsupportedReason, Version,
    Wire, correlate, decode, encode, schema, source_chunk,
};
use serde_json::{Value, json};

const MAX_MAP_BYTES: usize = 1024 * 1024;
const GENERATED: &str = include_str!("../../../fixtures/devtools-v1/module.js");
const ORIGINAL: &str = include_str!("../../../fixtures/devtools-v1/original.ts");
const MAP: &[u8] = include_bytes!("../../../fixtures/devtools-v1/module.js.map");

fn identity(id: &str, url: &str) -> SourceIdentity {
    SourceIdentity {
        target: "fixture".into(),
        epoch: 1,
        id: id.into(),
        url: url.into(),
        revision: "r1".into(),
    }
}

fn generated() -> SourceIdentity {
    identity("module", "kunlun:fixture/module")
}

fn map_identity() -> SourceIdentity {
    identity("map", "kunlun:fixture/module.js.map")
}

fn original() -> SourceIdentity {
    identity("original", "kunlun:fixture/original.ts")
}

/// Exact immutable identities are the only resolver. Neither map source names nor
/// caller URLs are opened or fetched; sourcesContent is not an authority either.
fn lookup(case: &Value) -> Result<SourceMapping, Error> {
    let mutation = case["mutation"].as_str().unwrap_or("");
    let mut source = generated();
    match mutation {
        "epoch" => source.epoch += 1,
        "target" => source.target = "foreign".into(),
        "revision" => source.revision = "r2".into(),
        "source" => source.url = "file:///etc/passwd".into(),
        _ => {}
    }
    if source.target != generated().target || source.epoch != generated().epoch {
        return Err(Error::StaleEpoch);
    }
    let registered_sources = if mutation == "original" {
        vec![(generated(), GENERATED)]
    } else {
        vec![(generated(), GENERATED), (original(), ORIGINAL)]
    };
    if !registered_sources.iter().any(|(id, _)| id == &source) {
        return Err(Error::SourceNotFound);
    }
    if mutation == "unsupported" {
        return Err(Error::Unsupported {
            operation: Operation::LookupSourceMap,
            reason: UnsupportedReason::NotImplemented,
        });
    }
    let bytes = match mutation {
        "invalid_map" => b"not JSON".to_vec(),
        "oversized_map" => vec![b' '; MAX_MAP_BYTES + 1],
        "max_size_map" => {
            let mut bytes = MAP.to_vec();
            bytes.resize(MAX_MAP_BYTES, b' ');
            bytes
        }
        _ => MAP.to_vec(),
    };
    let registered_maps = if mutation == "map" {
        Vec::new()
    } else {
        vec![(map_identity(), bytes)]
    };
    let mut associated_map = map_identity();
    match mutation {
        "map_revision" => associated_map.revision = "r2".into(),
        "map_epoch" => associated_map.epoch = 2,
        "map_target" => associated_map.target = "foreign".into(),
        _ => {}
    }
    let descriptors = [SourceDescriptor {
        source: generated(),
        source_map: if mutation == "no_association" {
            None
        } else {
            Some(associated_map)
        },
    }];
    let descriptor = descriptors
        .iter()
        .find(|descriptor| descriptor.source == source)
        .ok_or(Error::SourceNotFound)?;
    let requested_map = descriptor.source_map.clone().ok_or(Error::SourceNotFound)?;
    if requested_map.target != source.target || requested_map.epoch != source.epoch {
        return Err(Error::StaleEpoch);
    }
    let bytes = &registered_maps
        .iter()
        .find(|(id, _)| id == &requested_map)
        .ok_or(Error::SourceNotFound)?
        .1;
    // Bound before parsing, just as runtime SourceMaps::insert does.
    if bytes.len() > MAX_MAP_BYTES {
        return Err(Error::MessageTooLarge);
    }
    let decoded = sourcemap::decode_slice(bytes).map_err(|_| Error::Malformed)?;
    let location = SourceLocation {
        source,
        line: case["line"].as_u64().unwrap_or(0) as u32,
        column: case["column"].as_u64().unwrap_or(0) as u32,
    };
    let token = decoded
        .lookup_token(location.line, location.column)
        // GLB must never bleed a preceding generated line into this lookup.
        .filter(|token| token.get_dst_line() == location.line);
    let original = match token {
        Some(token) if token.get_source().is_some() => {
            let name = token.get_source().unwrap();
            let registered = registered_sources
                .iter()
                .find(|(id, _)| id == &original() && id.url == name)
                .ok_or(Error::SourceNotFound)?
                .0
                .clone();
            Some(SourceLocation {
                source: registered,
                line: token.get_src_line(),
                column: token.get_src_col(),
            })
        }
        _ => None,
    };
    Ok(SourceMapping {
        generated: location,
        map: requested_map,
        original,
    })
}

#[test]
fn every_portable_mock_case() {
    let suite: Value = serde_json::from_str(include_str!(
        "../../../fixtures/devtools-v1/source-map-cases.json"
    ))
    .unwrap();
    assert!(suite["evidence"].as_str().unwrap().starts_with("MOCK ONLY"));
    assert_eq!(suite["max_map_bytes"], MAX_MAP_BYTES);
    let validator = jsonschema::validator_for(&serde_json::to_value(schema()).unwrap()).unwrap();
    for case in suite["cases"].as_array().unwrap() {
        let actual = match case["action"].as_str().unwrap() {
            "lookup" => lookup(case).map(|mapping| {
                // Exercise generated-source retrieval as well as real map decoding.
                assert_eq!(
                    source_chunk(mapping.generated.source.clone(), GENERATED, 0, 16384)
                        .unwrap()
                        .text,
                    GENERATED
                );
                let value = serde_json::to_value(&mapping).unwrap();
                assert_eq!(
                    serde_json::from_value::<SourceMapping>(value).unwrap(),
                    mapping
                );
                let request = Request {
                    version: Version::V1,
                    request_id: "map-case".into(),
                    identity: Identity {
                        target: "fixture".into(),
                        session: "map-session".into(),
                        epoch: 1,
                    },
                    command: Command::LookupSourceMap {
                        location: SourceLocation {
                            source: generated(),
                            line: case["line"].as_u64().unwrap_or(0) as u32,
                            column: case["column"].as_u64().unwrap_or(0) as u32,
                        },
                    },
                };
                let response = Response {
                    version: Version::V1,
                    request_id: "map-case".into(),
                    identity: request.identity.clone(),
                    result: ResponseResult::Success {
                        result: CommandResult::LookupSourceMap {
                            mapping: Box::new(mapping.clone()),
                        },
                    },
                };
                correlate(&request, &response).unwrap();
                let wire = Wire::Response { response };
                assert!(validator.is_valid(&serde_json::to_value(&wire).unwrap()));
                assert_eq!(decode(&encode(&wire).unwrap()).unwrap(), wire);
                mapping.original.map_or(
                    Value::Null,
                    |location| json!({"line":location.line,"column":location.column}),
                )
            }),
            "read" => source_chunk(
                original(),
                ORIGINAL,
                case["offset"].as_u64().unwrap() as u32,
                case["max_bytes"].as_u64().unwrap() as u32,
            )
            .map(|chunk| json!({"text":chunk.text,"eof":chunk.eof})),
            action => panic!("unknown portable action {action}"),
        };
        if let Some(error) = case["error"].as_str() {
            assert_eq!(
                serde_json::to_value(actual.unwrap_err()).unwrap()["code"],
                error,
                "{}",
                case["name"]
            );
        } else {
            let expected = if case["action"] == "read" {
                json!({"text":case["text"],"eof":case["eof"]})
            } else {
                case["expected"].clone()
            };
            assert_eq!(actual.unwrap(), expected, "{}", case["name"]);
        }
    }
}

#[test]
fn utf16_is_not_utf8_or_unicode_scalar_count() {
    let prefix = ORIGINAL.split('é').next().unwrap();
    assert_eq!(prefix.encode_utf16().count(), 33);
    assert_eq!(prefix.chars().count(), 32);
    assert_eq!(prefix.len(), 35);
    assert_eq!(
        source_chunk(original(), ORIGINAL, 31, 5).unwrap().text,
        "😀"
    );
}
