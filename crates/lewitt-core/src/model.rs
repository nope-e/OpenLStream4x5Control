use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

pub const STREAM_4X5_VENDOR_ID: u16 = 0x29C2;
pub const STREAM_4X5_PRODUCT_ID: u16 = 0x0011;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DeviceId(String);

impl DeviceId {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for DeviceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub id: DeviceId,
    pub display_name: String,
    pub vendor_id: u16,
    pub product_id: u16,
    pub firmware_version: Option<String>,
}

impl DeviceInfo {
    #[must_use]
    pub fn is_stream_4x5(&self) -> bool {
        self.vendor_id == STREAM_4X5_VENDOR_ID && self.product_id == STREAM_4X5_PRODUCT_ID
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelId {
    Input1,
    Input2,
    Input3_4,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BusId {
    Output1_2,
    Output3_4,
    Output5_6,
    MixA,
    MixB,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlId {
    InputVolume {
        channel: ChannelId,
    },
    /// Read-only state derived from an input fader's mute endpoint.
    InputMute {
        channel: ChannelId,
    },
    PhantomPower {
        channel: ChannelId,
    },
    HighPass {
        channel: ChannelId,
    },
    PhaseInvert {
        channel: ChannelId,
    },
    OutputVolume {
        bus: BusId,
    },
    MonitorVolume,
    MixerWeight {
        source: ChannelId,
        destination: BusId,
    },
    DuckerEnabled,
    DuckerThreshold,
    SampleRate,
    ClockSource,
    AsioBufferSize,
}

impl ControlId {
    /// Controls intentionally excluded from the native preset format.
    #[must_use]
    pub fn is_preset_safe(&self) -> bool {
        !matches!(
            self,
            Self::InputMute { .. } | Self::SampleRate | Self::ClockSource | Self::AsioBufferSize
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum ControlValue {
    Boolean(bool),
    Scalar(f32),
    Decibels(f32),
    Integer(i32),
    Choice(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ControlCommand {
    pub control: ControlId,
    pub value: ControlValue,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlType {
    Boolean,
    Scalar {
        minimum: f32,
        maximum: f32,
        step: f32,
    },
    Decibels {
        minimum: f32,
        maximum: f32,
        step: f32,
    },
    Integer,
    Choice {
        choices: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ControlDescriptor {
    pub id: ControlId,
    pub value: ControlType,
    pub writable: bool,
}

impl ControlDescriptor {
    #[must_use]
    pub fn is_continuous(&self) -> bool {
        matches!(
            &self.value,
            ControlType::Scalar { .. } | ControlType::Decibels { .. }
        )
    }

    #[must_use]
    pub fn numeric_range(&self) -> Option<(f32, f32)> {
        match &self.value {
            ControlType::Scalar {
                minimum, maximum, ..
            }
            | ControlType::Decibels {
                minimum, maximum, ..
            } => Some((*minimum, *maximum)),
            ControlType::Boolean | ControlType::Integer | ControlType::Choice { .. } => None,
        }
    }

    #[must_use]
    pub fn step(&self) -> Option<f32> {
        match &self.value {
            ControlType::Scalar { step, .. } | ControlType::Decibels { step, .. } => Some(*step),
            ControlType::Boolean | ControlType::Integer | ControlType::Choice { .. } => None,
        }
    }

    pub fn validate(&self, value: &ControlValue) -> Result<(), String> {
        match (&self.value, value) {
            (ControlType::Boolean, ControlValue::Boolean(_))
            | (ControlType::Integer, ControlValue::Integer(_)) => Ok(()),
            (
                ControlType::Scalar {
                    minimum,
                    maximum,
                    step,
                },
                ControlValue::Scalar(value),
            )
            | (
                ControlType::Decibels {
                    minimum,
                    maximum,
                    step,
                },
                ControlValue::Decibels(value),
            ) => validate_numeric(*value, *minimum, *maximum, *step),
            (ControlType::Choice { choices }, ControlValue::Choice(choice)) => choices
                .iter()
                .any(|candidate| candidate == choice)
                .then_some(())
                .ok_or_else(|| format!("unknown choice {choice:?}")),
            (expected, received) => Err(format!("expected {expected:?}, received {received:?}")),
        }
    }
}

fn validate_numeric(value: f32, minimum: f32, maximum: f32, step: f32) -> Result<(), String> {
    if !value.is_finite() || value < minimum || value > maximum {
        return Err(format!("value {value} is outside [{minimum}, {maximum}]"));
    }
    let steps = (value - minimum) / step;
    if (steps - steps.round()).abs() > 1.0e-4 {
        return Err(format!("value {value} is not aligned to step {step}"));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceCapabilities {
    pub model: String,
    pub controls: Vec<ControlDescriptor>,
    pub meter_sources: Vec<MeterId>,
}

impl DeviceCapabilities {
    #[must_use]
    pub fn descriptor(&self, control: &ControlId) -> Option<&ControlDescriptor> {
        self.controls
            .iter()
            .find(|candidate| &candidate.id == control)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DeviceSnapshot {
    pub revision: u64,
    pub controls: BTreeMap<ControlId, ControlValue>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MeterId {
    Input { channel: ChannelId },
    Output { bus: BusId },
    Mix { bus: BusId },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MeterSample {
    pub source: MeterId,
    pub peak_dbfs: f32,
    pub rms_dbfs: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct MeterFrame {
    pub sequence: u64,
    pub samples: Vec<MeterSample>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DeviceEvent {
    Connected { device: DeviceInfo },
    Disconnected,
    StateChanged { controls: Vec<ControlId> },
    ControlCenterConflict,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeric_control_rejects_non_finite_and_misaligned_values() {
        let descriptor = ControlDescriptor {
            id: ControlId::MonitorVolume,
            value: ControlType::Decibels {
                minimum: -60.0,
                maximum: 12.0,
                step: 0.5,
            },
            writable: true,
        };
        assert!(
            descriptor
                .validate(&ControlValue::Decibels(f32::NAN))
                .is_err()
        );
        assert!(
            descriptor
                .validate(&ControlValue::Decibels(-11.75))
                .is_err()
        );
        assert_eq!(descriptor.validate(&ControlValue::Decibels(-11.5)), Ok(()));
    }

    #[test]
    fn stream_4x5_identity_is_exact() {
        let device = DeviceInfo {
            id: DeviceId::new("test"),
            display_name: "Stream 4x5".into(),
            vendor_id: STREAM_4X5_VENDOR_ID,
            product_id: STREAM_4X5_PRODUCT_ID,
            firmware_version: None,
        };
        assert!(device.is_stream_4x5());
    }
}
