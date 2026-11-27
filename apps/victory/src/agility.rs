//! The D3D12 Agility SDK for Windows 10 (the `agility` feature).
//!
//! Windows 10's own D3D12 runtime stops at shader model 6.5; the title's
//! translated shaders need 6.6. An exe that exports these two variables gets
//! the runtime named by them instead: `D3D12Core.dll` of SDK version
//! `D3D12SDKVersion`, from `D3D12SDKPath` relative to the exe. The DLL is
//! Microsoft's (NuGet package Microsoft.Direct3D.D3D12 1.620.2); the
//! launcher fetches it from there on Windows 10 and starts this build only
//! when it is in place, since the exports without the DLL stop the device
//! from being made at all (see `build.rs`).
//!
//! Based on Marvisak/VIctory PR #175 and trustno1-coder/VIctory-NVIDIA PR #10.

#![allow(non_upper_case_globals)]

/// The Agility SDK version the `D3D12` folder holds (1.620.x).
#[unsafe(no_mangle)]
pub static D3D12SDKVersion: u32 = 620;

/// A C string pointer the runtime reads in place.
#[repr(transparent)]
pub struct SdkPath(*const std::ffi::c_char);

// SAFETY: the pointer is to a static, immutable C string.
unsafe impl Sync for SdkPath {}

/// Where `D3D12Core.dll` is, relative to the exe.
#[unsafe(no_mangle)]
pub static D3D12SDKPath: SdkPath = SdkPath(c".\\D3D12\\".as_ptr());
