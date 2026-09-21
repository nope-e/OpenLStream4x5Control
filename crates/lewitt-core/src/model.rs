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
    /// Independent mute state for a physical output path.
    ///
    /// The Windows control center implements this in the host DSP filter; it
    /// is not the six-byte state area in the device's private settings block.
    /// Backends without an equivalent provider must omit this capability.
    OutputMute {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlKind {
    Continuous,
    Discrete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueKind {
    Boolean,
    Scalar,
    Decibels,
    Integer,
    Choice,
}

impl ValueKind {
    #[must_use]
    pub fn matches(self, value: &ControlValue) -> bool {
        matches!(
            (self, value),
            (Self::Boolean, ControlValue::Boolean(_))
                | (Self::Scalar, ControlValue::Scalar(_))
                | (Self::Decibels, ControlValue::Decibels(_))
                | (Self::Integer, ControlValue::Integer(_))
                | (Self::Choice, ControlValue::Choice(_))
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct NumericRange {
    pub minimum: f32,
    pub maximum: f32,
    pub step: Option<f32>,
}

impl NumericRange {
    #[must_use]
    pub fn contains(self, value: f32) -> bool {
        value.is_finite() && value >= self.minimum && value <= self.maximum
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlAccess {
    ReadOnly,
    Writable,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ControlDescriptor {
    pub id: ControlId,
    pub kind: ControlKind,
    pub value_kind: ValueKind,
    pub access: ControlAccess,
    pub range: Option<NumericRange>,
    #[serde(default)]
    pub choices: Vec<String>,
}

impl ControlDescriptor {
    pub fn validate(&self, value: &ControlValue) -> Result<(), String> {
        if !self.value_kind.matches(value) {
            return Err(format!(
                "expected {:?}, received {value:?}",
                self.value_kind
            ));
        }

        if let Some(range) = self.range {
            match value {
                ControlValue::Scalar(value) | ControlValue::Decibels(value) => {
                    if !range.contains(*value) {
                        return Err(format!(
                            "value {value} is outside [{}, {}]",
                            range.minimum, range.maximum
                        ));
                    }
                }
                ControlValue::Integer(value) => {
                    let value = f64::from(*value);
                    if value < f64::from(range.minimum) || value > f64::from(range.maximum) {
                        return Err(format!(
                            "value {value} is outside [{}, {}]",
                            range.minimum, range.maximum
                        ));
                    }
                }
                ControlValue::Boolean(_) | ControlValue::Choice(_) => {}
            }
        }
        if let ControlValue::Choice(choice) = value
            && !self.choices.iter().any(|candidate| candidate == choice)
        {
            return Err(format!("unknown choice {choice:?}"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum FirmwareStatus {
    Verified { profile: String },
    UnknownReadOnly { version: Option<String> },
    Unsupported { version: Option<String> },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceCapabilities {
    pub model: String,
    pub firmware: FirmwareStatus,
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

    #[must_use]
    pub fn writes_enabled(&self) -> bool {
        matches!(self.firmware, FirmwareStatus::Verified { .. })
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
    fn numeric_range_rejects_non_finite_values() {
        let range = NumericRange {
            minimum: -60.0,
            maximum: 12.0,
            step: None,
        };
        assert!(!range.contains(f32::NAN));
        assert!(!range.contains(f32::INFINITY));
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
