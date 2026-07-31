//! Safe cross-platform adapter discovery through wgpu.

use wgpu::{DeviceType, Instance, InstanceDescriptor};

use super::DeviceInfo;
use super::engine::{adapter_is_compatible, supported_backends};

pub(super) fn devices() -> Vec<DeviceInfo> {
    let backends = supported_backends();
    let instance = Instance::new(&InstanceDescriptor {
        backends,
        ..Default::default()
    });
    instance
        .enumerate_adapters(backends)
        .into_iter()
        .filter(adapter_is_compatible)
        .map(|adapter| {
            let information = adapter.get_info();
            DeviceInfo {
                name: information.name,
                backend: information.backend.to_str().to_owned(),
                vendor_id: information.vendor,
                device_id: information.device,
                discrete: information.device_type == DeviceType::DiscreteGpu,
            }
        })
        .collect()
}
