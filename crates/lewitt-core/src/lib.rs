//! Hardware-independent domain model and serialized device worker.
//!
//! The core never loads vendor code and never opens USB devices directly.
//! Platform backends implement [`DeviceBackend`], and [`Controller`] keeps all
//! calls to a backend on one dedicated thread.

mod backend;
mod capabilities;
mod controller;
mod error;
mod mock;
mod model;
mod preset;
mod units;

pub use backend::DeviceBackend;
pub use capabilities::{CapabilityOs, capabilities_for};
pub use controller::{
    Controller, ControllerConfig, ControllerError, ControllerEvent, WorkerOperation,
};
pub use error::{BackendError, BackendResult};
pub use mock::{MockBackend, MockHandle, MockOperation, MockWrite};
pub use model::{
    BusId, ChannelId, ControlCommand, ControlDescriptor, ControlId, ControlType, ControlValue,
    DeviceCapabilities, DeviceEvent, DeviceId, DeviceInfo, DeviceSnapshot, MeterFrame, MeterId,
    MeterSample, STREAM_4X5_PRODUCT_ID, STREAM_4X5_VENDOR_ID,
};
pub use preset::{CURRENT_PRESET_SCHEMA, NativePreset, PresetError};
pub use units::{db_to_linear, linear_to_db};
