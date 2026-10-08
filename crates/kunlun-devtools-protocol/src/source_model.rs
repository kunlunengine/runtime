//! Immutable adapter-registered source metadata; no URL or path resolution.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceLocation {
    pub source: crate::SourceIdentity,
    /// Zero-based line.
    pub line: u32,
    /// Zero-based column measured in UTF-16 code units.
    pub column: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceDescriptor {
    pub source: crate::SourceIdentity,
    pub source_map: Option<crate::SourceIdentity>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceMapping {
    pub generated: SourceLocation,
    pub map: crate::SourceIdentity,
    /// None means the registered map has no mapped token at this location.
    pub original: Option<SourceLocation>,
}

pub(crate) fn validate_location(location: &SourceLocation) -> Result<(), crate::Error> {
    crate::validate_source(&location.source)
}

pub(crate) fn validate_descriptor(descriptor: &SourceDescriptor) -> Result<(), crate::Error> {
    crate::validate_source(&descriptor.source)?;
    if let Some(map) = &descriptor.source_map {
        crate::validate_source(map)?;
        same_endpoint(&descriptor.source, map)?;
    }
    Ok(())
}

pub(crate) fn validate_mapping(mapping: &SourceMapping) -> Result<(), crate::Error> {
    validate_location(&mapping.generated)?;
    crate::validate_source(&mapping.map)?;
    same_endpoint(&mapping.generated.source, &mapping.map)?;
    if let Some(original) = &mapping.original {
        validate_location(original)?;
        same_endpoint(&mapping.generated.source, &original.source)?;
    }
    Ok(())
}

fn same_endpoint(
    generated: &crate::SourceIdentity,
    other: &crate::SourceIdentity,
) -> Result<(), crate::Error> {
    if generated.target != other.target || generated.epoch != other.epoch {
        return Err(crate::Error::StaleEpoch);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(id: &str) -> crate::SourceIdentity {
        crate::SourceIdentity {
            target: "fixture".into(),
            epoch: 1,
            id: id.into(),
            url: format!("kunlun:fixture/{id}"),
            revision: "r1".into(),
        }
    }

    fn location(id: &str) -> SourceLocation {
        SourceLocation {
            source: source(id),
            line: 0,
            column: 33,
        }
    }

    #[test]
    fn descriptors_validate_all_identities_and_endpoints() {
        let mut descriptor = SourceDescriptor {
            source: source("module"),
            source_map: None,
        };
        assert_eq!(validate_descriptor(&descriptor), Ok(()));
        descriptor.source_map = Some(source("map"));
        assert_eq!(validate_descriptor(&descriptor), Ok(()));
        descriptor.source_map.as_mut().unwrap().epoch = 2;
        assert_eq!(
            validate_descriptor(&descriptor),
            Err(crate::Error::StaleEpoch)
        );
        descriptor.source_map = Some(source("map"));
        descriptor.source_map.as_mut().unwrap().target = "foreign".into();
        assert_eq!(
            validate_descriptor(&descriptor),
            Err(crate::Error::StaleEpoch)
        );
        descriptor.source_map.as_mut().unwrap().revision.clear();
        assert_eq!(
            validate_descriptor(&descriptor),
            Err(crate::Error::InvalidIdentity)
        );
        descriptor.source_map = None;
        descriptor.source.id.clear();
        assert_eq!(
            validate_descriptor(&descriptor),
            Err(crate::Error::InvalidIdentity)
        );
    }

    #[test]
    fn mappings_validate_all_identities_and_endpoints() {
        let base = SourceMapping {
            generated: location("module"),
            map: source("map"),
            original: Some(location("original")),
        };
        assert_eq!(validate_mapping(&base), Ok(()));
        let mut unmapped = base.clone();
        unmapped.original = None;
        assert_eq!(validate_mapping(&unmapped), Ok(()));
        for field in 0..3 {
            for mutation in 0..3 {
                let mut mapping = base.clone();
                let identity = match field {
                    0 => &mut mapping.generated.source,
                    1 => &mut mapping.map,
                    _ => &mut mapping.original.as_mut().unwrap().source,
                };
                let expected = match mutation {
                    0 => {
                        identity.epoch = 2;
                        crate::Error::StaleEpoch
                    }
                    1 => {
                        identity.target = "foreign".into();
                        crate::Error::StaleEpoch
                    }
                    _ => {
                        identity.id.clear();
                        crate::Error::InvalidIdentity
                    }
                };
                assert_eq!(validate_mapping(&mapping), Err(expected));
            }
        }
    }
}
