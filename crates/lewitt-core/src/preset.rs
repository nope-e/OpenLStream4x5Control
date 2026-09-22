use crate::{ControlCommand, ControlId, DeviceCapabilities};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use thiserror::Error;

pub const CURRENT_PRESET_SCHEMA: u32 = 1;

/// Native preset data. Excluded settings cannot be represented elsewhere in
/// this structure and are rejected if placed in `controls`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativePreset {
    pub schema_version: u32,
    pub name: String,
    pub controls: Vec<ControlCommand>,
}

impl NativePreset {
    pub fn validate(&self, capabilities: &DeviceCapabilities) -> Result<(), PresetError> {
        if self.schema_version != CURRENT_PRESET_SCHEMA {
            return Err(PresetError::UnsupportedSchema(self.schema_version));
        }
        if self.name.trim().is_empty() {
            return Err(PresetError::EmptyName);
        }
        let mut seen = BTreeSet::new();
        for command in &self.controls {
            if !command.control.is_preset_safe() {
                return Err(PresetError::ExcludedControl(command.control.clone()));
            }
            if !seen.insert(command.control.clone()) {
                return Err(PresetError::DuplicateControl(command.control.clone()));
            }
            let descriptor = capabilities
                .descriptor(&command.control)
                .ok_or_else(|| PresetError::UnsupportedControl(command.control.clone()))?;
            if !descriptor.writable {
                return Err(PresetError::ReadOnlyControl(command.control.clone()));
            }
            descriptor
                .validate(&command.value)
                .map_err(|reason| PresetError::InvalidValue {
                    control: command.control.clone(),
                    reason,
                })?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PresetError {
    #[error("unsupported preset schema version {0}")]
    UnsupportedSchema(u32),
    #[error("preset name must not be empty")]
    EmptyName,
    #[error("control is excluded from presets: {0:?}")]
    ExcludedControl(ControlId),
    #[error("control occurs more than once: {0:?}")]
    DuplicateControl(ControlId),
    #[error("device does not support control: {0:?}")]
    UnsupportedControl(ControlId),
    #[error("control is read-only: {0:?}")]
    ReadOnlyControl(ControlId),
    #[error("invalid value for {control:?}: {reason}")]
    InvalidValue { control: ControlId, reason: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ControlValue, MockBackend};

    fn capabilities() -> DeviceCapabilities {
        let (mut backend, _) = MockBackend::stream_4x5();
        crate::DeviceBackend::open(&mut backend, &crate::DeviceId::new("mock-stream-4x5"))
            .expect("mock device should open");
        crate::DeviceBackend::capabilities(&mut backend).expect("capabilities should be readable")
    }

    #[test]
    fn version_one_round_trips_as_json() {
        let preset = NativePreset {
            schema_version: CURRENT_PRESET_SCHEMA,
            name: "Voice".into(),
            controls: vec![ControlCommand {
                control: ControlId::DuckerEnabled,
                value: ControlValue::Boolean(true),
            }],
        };
        preset.validate(&capabilities()).expect("preset is valid");
        let json = serde_json::to_string(&preset).expect("preset should serialize");
        let decoded: NativePreset = serde_json::from_str(&json).expect("preset should deserialize");
        assert_eq!(decoded, preset);
    }

    #[test]
    fn clock_source_is_excluded() {
        let preset = NativePreset {
            schema_version: CURRENT_PRESET_SCHEMA,
            name: "Unsafe".into(),
            controls: vec![ControlCommand {
                control: ControlId::ClockSource,
                value: ControlValue::Choice("internal".into()),
            }],
        };
        assert_eq!(
            preset.validate(&capabilities()),
            Err(PresetError::ExcludedControl(ControlId::ClockSource))
        );
    }

    #[test]
    fn derived_mute_state_is_excluded() {
        let control = ControlId::InputMute {
            channel: crate::ChannelId::Input1,
        };
        let preset = NativePreset {
            schema_version: CURRENT_PRESET_SCHEMA,
            name: "Derived state".into(),
            controls: vec![ControlCommand {
                control: control.clone(),
                value: ControlValue::Boolean(true),
            }],
        };
        assert_eq!(
            preset.validate(&capabilities()),
            Err(PresetError::ExcludedControl(control))
        );
    }
}
