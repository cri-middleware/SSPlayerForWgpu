//! The one test that fails when the machine has no GPU adapter.
//!
//! Every test that draws or advances a player returns early without an adapter,
//! and an early return reports `ok` — so on such a machine the whole GPU half of
//! the suite comes back green having verified nothing. This test is what says so.
//!
//! On Linux with no GPU driver, Mesa's software Vulkan device (lavapipe, package
//! `mesa-vulkan-drivers`) is enough; it is what CI runs the suite on.

#[test]
fn an_adapter_is_present() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()));
    let device = adapter.as_ref().ok().map(|adapter| pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())));
    assert!(
        matches!(device, Some(Ok(_))),
        "no GPU adapter: every GPU test in this crate returned early and verified nothing. \
         On Linux, install Mesa's lavapipe (mesa-vulkan-drivers)."
    );
}
