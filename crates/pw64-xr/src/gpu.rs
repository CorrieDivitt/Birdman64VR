//! Vulkan instance and device created the way `XR_KHR_vulkan_enable2`
//! requires (the runtime creates both, from our create-infos, and picks the
//! physical device), then handed to wgpu through wgpu-hal's raw-handle
//! constructors. The create-infos mirror what wgpu-hal builds for itself
//! (`vulkan::Instance::init`, `Adapter::open`), so the device behaves like
//! one wgpu made.

use crate::XrError;
use ash::vk::{self, Handle};
use openxr as xr;
use std::ffi::CStr;
use wgpu::hal;
use wgpu::hal::api::Vulkan;

/// The wgpu objects of the OpenXR-chosen Vulkan device, plus the raw
/// handles the OpenXR session binding needs.
pub struct XrGpu {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub(crate) vk_instance: vk::Instance,
    pub(crate) vk_physical_device: vk::PhysicalDevice,
    pub(crate) vk_device: vk::Device,
    pub(crate) queue_family_index: u32,
}

fn gfx(what: &str, e: impl std::fmt::Display) -> XrError {
    XrError::Graphics(format!("{what}: {e}"))
}

impl XrGpu {
    /// `wanted_features`: wgpu features to enable where the device has them
    /// (the rest of the device descriptor is wgpu's default).
    pub(crate) fn new(
        xr_instance: &xr::Instance,
        system: xr::SystemId,
        wanted_features: wgpu::Features,
    ) -> Result<Self, XrError> {
        let entry = unsafe { ash::Entry::load() }.map_err(|e| gfx("Vulkan is not installed", e))?;
        let instance_api_version = match unsafe { entry.try_enumerate_instance_version() } {
            Ok(Some(v)) => v,
            Ok(None) => vk::API_VERSION_1_0,
            Err(e) => return Err(gfx("vkEnumerateInstanceVersion", e)),
        };
        // OpenXR needs Vulkan 1.1 at least; ask for 1.3 like wgpu-hal does.
        if instance_api_version < vk::API_VERSION_1_1 {
            return Err(XrError::Graphics(
                "the Vulkan driver is older than 1.1".into(),
            ));
        }
        let reqs = xr_instance.graphics_requirements::<xr::Vulkan>(system)?;
        let app_api = vk::API_VERSION_1_3;
        let app_xr = xr::Version::new(1, vk::api_version_minor(app_api) as u16, 0);
        if app_xr < reqs.min_api_version_supported {
            return Err(XrError::Unsupported(format!(
                "support for Vulkan 1.3 (it needs {})",
                reqs.min_api_version_supported
            )));
        }

        let flags = wgpu::InstanceFlags::empty();
        let extensions =
            hal::vulkan::Instance::desired_extensions(&entry, instance_api_version, flags)
                .map_err(|e| gfx("Vulkan instance extensions", e))?;
        let ext_ptrs: Vec<_> = extensions.iter().map(|e| e.as_ptr()).collect();
        let app_info = vk::ApplicationInfo::default()
            .application_name(c"Birdman64VR")
            .engine_name(c"wgpu-hal")
            .engine_version(2)
            .api_version(app_api);
        let create_info = vk::InstanceCreateInfo::default()
            .application_info(&app_info)
            .enabled_extension_names(&ext_ptrs);
        // SAFETY: the create-info and the loader's vkGetInstanceProcAddr are
        // valid for the call; openxr's and ash's pointer types are the same
        // Vulkan C types under different Rust names.
        let raw_instance = unsafe {
            let raw = xr_instance
                .create_vulkan_instance(
                    system,
                    #[allow(clippy::missing_transmute_annotations)]
                    std::mem::transmute(entry.static_fn().get_instance_proc_addr),
                    (&raw const create_info).cast(),
                )?
                .map_err(|e| gfx("vkCreateInstance", vk::Result::from_raw(e)))?;
            ash::Instance::load(entry.static_fn(), vk::Instance::from_raw(raw as _))
        };
        let vk_instance = raw_instance.handle();
        let vk_physical_device = vk::PhysicalDevice::from_raw(unsafe {
            xr_instance.vulkan_graphics_device(system, vk_instance.as_raw() as _)?
        } as _);

        // From here wgpu-hal owns the Vulkan instance (drop_callback None).
        let hal_instance = unsafe {
            hal::vulkan::Instance::from_raw(
                entry.clone(),
                raw_instance.clone(),
                instance_api_version,
                0,
                None,
                extensions,
                flags,
                wgpu::MemoryBudgetThresholds::default(),
                false,
                None,
            )
        }
        .map_err(|e| gfx("wgpu Vulkan instance", e))?;
        let exposed = hal_instance
            .expose_adapter(vk_physical_device)
            .ok_or_else(|| XrError::Graphics("wgpu cannot use the headset's GPU".into()))?;

        let features = exposed.features & wanted_features;
        let enabled_extensions: Vec<&'static CStr> =
            exposed.adapter.required_device_extensions(features);
        let mut phd_features = exposed
            .adapter
            .physical_device_features(&enabled_extensions, features);
        let queue_family_index =
            unsafe { raw_instance.get_physical_device_queue_family_properties(vk_physical_device) }
                .iter()
                .position(|q| q.queue_flags.contains(vk::QueueFlags::GRAPHICS))
                .ok_or_else(|| XrError::Graphics("no graphics queue".into()))? as u32;
        let queue_infos = [vk::DeviceQueueCreateInfo::default()
            .queue_family_index(queue_family_index)
            .queue_priorities(&[1.0])];
        let dev_ext_ptrs: Vec<_> = enabled_extensions.iter().map(|e| e.as_ptr()).collect();
        let device_info = vk::DeviceCreateInfo::default()
            .queue_create_infos(&queue_infos)
            .enabled_extension_names(&dev_ext_ptrs);
        let device_info = phd_features.add_to_device_create(device_info);
        // SAFETY: as for the instance above.
        let raw_device = unsafe {
            let raw = xr_instance
                .create_vulkan_device(
                    system,
                    #[allow(clippy::missing_transmute_annotations)]
                    std::mem::transmute(entry.static_fn().get_instance_proc_addr),
                    vk_physical_device.as_raw() as _,
                    (&raw const device_info).cast(),
                )?
                .map_err(|e| gfx("vkCreateDevice", vk::Result::from_raw(e)))?;
            ash::Device::load(raw_instance.fp_v1_0(), vk::Device::from_raw(raw as _))
        };
        let vk_device = raw_device.handle();
        // SAFETY: the device was created from this adapter with exactly
        // these extensions, features and queue family; wgpu-hal owns it.
        let open = unsafe {
            exposed.adapter.device_from_raw(
                raw_device,
                None,
                &enabled_extensions,
                features,
                &wgpu::MemoryHints::default(),
                queue_family_index,
                0,
            )
        }
        .map_err(|e| gfx("wgpu Vulkan device", e))?;

        // SAFETY: the hal objects were made from each other just above.
        let (instance, adapter, device, queue) = unsafe {
            let instance = wgpu::Instance::from_hal::<Vulkan>(hal_instance);
            let adapter = instance.create_adapter_from_hal(exposed);
            let (device, queue) = adapter
                .create_device_from_hal(
                    open,
                    &wgpu::DeviceDescriptor {
                        label: Some("openxr"),
                        required_features: features,
                        ..Default::default()
                    },
                )
                .map_err(|e| gfx("wgpu device", e))?;
            (instance, adapter, device, queue)
        };
        Ok(Self {
            instance,
            adapter,
            device,
            queue,
            vk_instance,
            vk_physical_device,
            vk_device,
            queue_family_index,
        })
    }
}
