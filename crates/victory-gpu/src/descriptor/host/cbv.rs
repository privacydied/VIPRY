//! Renderer port 5 (`crate::port_cbv`): the host's views of the title's
//! constant buffers when shaders read them as raw bytes.

use super::*;

/// A raw view of `bytes` bytes from byte `offset` of a buffer.
fn raw_view(offset: u64, bytes: u64) -> D3D12_SHADER_RESOURCE_VIEW_DESC {
    D3D12_SHADER_RESOURCE_VIEW_DESC {
        Format: DXGI_FORMAT_R32_TYPELESS,
        ViewDimension: D3D12_SRV_DIMENSION_BUFFER,
        Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
        Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
            Buffer: D3D12_BUFFER_SRV {
                FirstElement: offset / 4,
                NumElements: (bytes / 4) as u32,
                StructureByteStride: 0,
                Flags: D3D12_BUFFER_SRV_FLAG_RAW,
            },
        },
    }
}

impl HostHeap {
    /// Makes slot `index` (a dynamic one) show the constant buffer of `size`
    /// bytes at `gpu` of `buffer` (title memory in place) with nothing
    /// copied: its raw twin a raw view of those bytes, which translated
    /// shaders read; the slot itself a constant buffer view of them where
    /// that can be (on 256 bytes), else a null one. Whether the twin was
    /// made (`false`: the caller copies the bytes as before).
    pub(crate) fn constant_raw_at(&self, index: u32, buffer: &ID3D12Resource, gpu: u64, size: u32) -> bool {
        let Some(host) = host() else { return false };
        let Some(twin) = self.twin(index) else { return false };
        // SAFETY: a live buffer.
        let (base, width) = unsafe { (buffer.GetGPUVirtualAddress(), buffer.GetDesc().Width) };
        let Some(offset) = gpu.checked_sub(base).filter(|&o| o < width && crate::port_cbv::raw_ok(o)) else {
            return false;
        };
        let bytes = u64::from(constant_bytes(size)).min(width - offset) & !3;
        if bytes == 0 {
            return false;
        }
        let raw = raw_view(offset, bytes);
        let cbv = D3D12_CONSTANT_BUFFER_VIEW_DESC {
            BufferLocation: if gpu.is_multiple_of(256) { gpu } else { 0 },
            SizeInBytes: if gpu.is_multiple_of(256) { constant_bytes(size).min(((width - offset) & !255) as u32) } else { 0 },
        };
        let cbv = if cbv.SizeInBytes == 0 { D3D12_CONSTANT_BUFFER_VIEW_DESC::default() } else { cbv };
        // SAFETY: slots of our heaps; the ranges lie in the live buffer (a
        // zero location makes a null constant buffer view).
        unsafe {
            host.device.CreateConstantBufferView(Some(&cbv), self.at(index));
            self.put(index);
            host.device.CreateShaderResourceView(buffer, Some(&raw), twin);
        }
        self.twin_made(index, true);
        self.keep(index, Some(buffer.clone()));
        true
    }
}

/// The constant buffer of `size` bytes at `gpu` of `buffer` as a constant
/// buffer view at `at` where it can be (on 256 bytes; else a null one) and a
/// raw view of its bytes at `twin`, which translated shaders read. `false`
/// (nothing made): not on 16 bytes.
pub(super) fn constant_twinned(
    buffer: &ID3D12Resource,
    gpu: u64,
    size: u32,
    at: D3D12_CPU_DESCRIPTOR_HANDLE,
    twin: Option<D3D12_CPU_DESCRIPTOR_HANDLE>,
) -> bool {
    let Some(host) = host() else { return false };
    // SAFETY: a live buffer.
    let (base, width) = unsafe { (buffer.GetGPUVirtualAddress(), buffer.GetDesc().Width) };
    let Some(offset) = gpu.checked_sub(base).filter(|&o| o < width && crate::port_cbv::raw_ok(o)) else {
        return false;
    };
    let room = width - offset;
    let cbv = if gpu.is_multiple_of(256) && room >= 256 {
        D3D12_CONSTANT_BUFFER_VIEW_DESC {
            BufferLocation: gpu,
            SizeInBytes: constant_bytes(size).min((room & !255) as u32),
        }
    } else {
        D3D12_CONSTANT_BUFFER_VIEW_DESC::default()
    };
    let bytes = u64::from(constant_bytes(size)).min(room) & !3;
    // SAFETY: slots of our heaps; the ranges lie in the live buffer.
    unsafe {
        host.device.CreateConstantBufferView(Some(&cbv), at);
        if let Some(twin) = twin {
            if bytes == 0 {
                null_buffer_view(twin);
            } else {
                host.device.CreateShaderResourceView(buffer, Some(&raw_view(offset, bytes)), twin);
            }
            crate::port_cbv::twinned();
        }
    }
    true
}
