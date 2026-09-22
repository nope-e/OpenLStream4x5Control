use crate::{
    BackendError, BackendResult, ControlDescriptor, ControlId, ControlType, DeviceCapabilities,
    MeterId,
};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

const CATALOG_JSON: &str = include_str!("../../../profiles/stream4x5.json");
const CATALOG_SCHEMA_VERSION: u32 = 1;
static CATALOG: OnceLock<Result<Catalog, String>> = OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityOs {
    Windows,
    Linux,
}

impl CapabilityOs {
    const fn key(self) -> &'static str {
        match self {
            Self::Windows => "windows",
            Self::Linux => "linux",
        }
    }
}

#[derive(Debug, Deserialize)]
struct Catalog {
    schema_version: u32,
    controls: BTreeMap<String, CatalogControl>,
    profiles: BTreeMap<String, BTreeMap<String, Profile>>,
}

#[derive(Debug, Clone, Deserialize)]
struct CatalogControl {
    id: ControlId,
    value: ControlType,
}

#[derive(Debug, Deserialize)]
struct Profile {
    controls: Vec<String>,
    #[serde(default)]
    writable: BTreeSet<String>,
    #[serde(default)]
    meter_sources: Vec<MeterId>,
}

/// Builds the device capability list from the embedded JSON catalog.
///
/// An exact firmware entry wins; otherwise the OS wildcard entry is used. The
/// returned descriptors are the only capability state consumed by the GUI,
/// controller, presets, and platform backends.
pub fn capabilities_for(
    os: CapabilityOs,
    firmware: Option<&str>,
    model: impl Into<String>,
) -> BackendResult<DeviceCapabilities> {
    let catalog = catalog()?;

    let os_profiles =
        catalog
            .profiles
            .get(os.key())
            .ok_or_else(|| BackendError::ProtocolMismatch {
                details: format!("capability catalog has no {} profile", os.key()),
            })?;
    let profile = firmware
        .and_then(|version| os_profiles.get(version))
        .or_else(|| os_profiles.get("*"))
        .ok_or_else(|| BackendError::Unsupported {
            feature: format!(
                "{} firmware {} has no capability profile",
                os.key(),
                firmware.unwrap_or("unknown")
            ),
        })?;

    let mut ids = BTreeSet::new();
    let mut controls = Vec::with_capacity(profile.controls.len());
    for key in &profile.controls {
        let definition =
            catalog
                .controls
                .get(key)
                .ok_or_else(|| BackendError::ProtocolMismatch {
                    details: format!("capability profile references unknown control {key:?}"),
                })?;
        if !ids.insert(definition.id.clone()) {
            return Err(BackendError::ProtocolMismatch {
                details: format!(
                    "capability profile contains duplicate ID {:?}",
                    definition.id
                ),
            });
        }
        controls.push(ControlDescriptor {
            id: definition.id.clone(),
            value: definition.value.clone(),
            writable: profile.writable.contains(key),
        });
    }

    Ok(DeviceCapabilities {
        model: model.into(),
        controls,
        meter_sources: profile.meter_sources.clone(),
    })
}

fn catalog() -> BackendResult<&'static Catalog> {
    CATALOG
        .get_or_init(|| {
            let catalog: Catalog = serde_json::from_str(CATALOG_JSON)
                .map_err(|error| format!("embedded capability catalog is invalid JSON: {error}"))?;
            validate_catalog(&catalog).map_err(|error| error.to_string())?;
            Ok(catalog)
        })
        .as_ref()
        .map_err(|details| BackendError::ProtocolMismatch {
            details: details.clone(),
        })
}

fn validate_catalog(catalog: &Catalog) -> BackendResult<()> {
    if catalog.schema_version != CATALOG_SCHEMA_VERSION {
        return Err(BackendError::ProtocolMismatch {
            details: format!(
                "capability catalog schema {} is unsupported",
                catalog.schema_version
            ),
        });
    }
    for (key, definition) in &catalog.controls {
        validate_control_definition(key, definition)?;
    }
    for (os, firmware_profiles) in &catalog.profiles {
        for (firmware, profile) in firmware_profiles {
            let readable = profile.controls.iter().collect::<BTreeSet<_>>();
            if readable.len() != profile.controls.len() {
                return Err(BackendError::ProtocolMismatch {
                    details: format!("{os}/{firmware} capability profile repeats a control"),
                });
            }
            if let Some(key) = profile.writable.iter().find(|key| !readable.contains(key)) {
                return Err(BackendError::ProtocolMismatch {
                    details: format!(
                        "{os}/{firmware} marks unreadable control {key:?} as writable"
                    ),
                });
            }
        }
    }
    Ok(())
}

fn validate_control_definition(key: &str, definition: &CatalogControl) -> BackendResult<()> {
    match &definition.value {
        ControlType::Scalar {
            minimum,
            maximum,
            step,
        }
        | ControlType::Decibels {
            minimum,
            maximum,
            step,
        } => {
            if !minimum.is_finite()
                || !maximum.is_finite()
                || !step.is_finite()
                || minimum > maximum
                || *step <= 0.0
            {
                return Err(BackendError::ProtocolMismatch {
                    details: format!("control {key:?} has an invalid numeric range"),
                });
            }
        }
        ControlType::Choice { choices } if choices.is_empty() => {
            return Err(BackendError::ProtocolMismatch {
                details: format!("choice control {key:?} has no choices"),
            });
        }
        ControlType::Boolean | ControlType::Integer | ControlType::Choice { .. } => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BusId, ChannelId};

    #[test]
    fn exact_windows_firmware_profile_exposes_json_ranges_and_writes() {
        let capabilities = capabilities_for(CapabilityOs::Windows, Some("0x018A"), "Stream 4x5")
            .expect("embedded profile is valid");
        assert_eq!(capabilities.controls.len(), 16);
        let input = capabilities
            .descriptor(&ControlId::InputVolume {
                channel: ChannelId::Input1,
            })
            .expect("input gain exists");
        assert_eq!(input.numeric_range(), Some((-7.0, 48.0)));
        assert_eq!(input.step(), Some(1.0));
        assert!(input.writable);
        assert!(
            capabilities
                .descriptor(&ControlId::OutputVolume {
                    bus: BusId::Output5_6,
                })
                .is_some_and(|descriptor| descriptor.writable)
        );
        assert!(
            !capabilities
                .descriptor(&ControlId::SampleRate)
                .expect("sample rate exists")
                .writable
        );
    }

    #[test]
    fn unknown_windows_firmware_uses_json_fallback_without_writes() {
        let capabilities = capabilities_for(CapabilityOs::Windows, Some("0xFFFF"), "Stream 4x5")
            .expect("fallback profile exists");
        assert_eq!(capabilities.controls.len(), 16);
        assert!(
            capabilities
                .controls
                .iter()
                .all(|control| !control.writable)
        );
    }

    #[test]
    fn linux_profile_is_explicitly_empty() {
        let capabilities = capabilities_for(CapabilityOs::Linux, Some("0x018A"), "Stream 4x5")
            .expect("Linux fallback profile exists");
        assert!(capabilities.controls.is_empty());
        assert!(capabilities.meter_sources.is_empty());
    }
}
