//! Private namespace VST3 module loading for macOS
//!
//! This module implements VST3 loading with proper symbol isolation to prevent
//! Objective-C class conflicts. It uses dlopen with RTLD_LOCAL to load plugins
//! in a private namespace, preventing symbol conflicts with system frameworks.
//!
//! According to VST3 specification:
//! - bundleEntry/bundleExit functions are REQUIRED on macOS
//! - Must call bundleEntry after loading and before GetPluginFactory
//! - Must call bundleExit before unloading
//! - Plugins with symbol conflicts require isolated loading

use super::{ModuleLoader, VstModule};
use crate::error::{Error, Result};
use core_foundation::{
    base::{Boolean, CFRelease, CFTypeRef, TCFType},
    bundle::{CFBundleCopyExecutableURL, CFBundleCreate, CFBundleRef},
    url::{CFURLCreateFromFileSystemRepresentation, CFURL},
};
use std::{
    ffi::{c_void, CString},
    path::Path,
    ptr,
};
use vst3::Steinberg::IPluginFactory;

/// Function signature for bundleEntry
type BundleEntryFunc = unsafe extern "C" fn(bundle: CFBundleRef) -> Boolean;

/// Function signature for bundleExit
type BundleExitFunc = unsafe extern "C" fn() -> Boolean;

/// Function signature for GetPluginFactory
type GetPluginFactoryFunc = unsafe extern "C" fn() -> *mut IPluginFactory;

/// Private namespace VST3 module implementation using dlopen with RTLD_LOCAL
pub struct PrivateNamespaceModule {
    /// dlopen handle with private namespace
    dl_handle: *mut c_void,
    /// CFBundle reference (for VST3 compliance)
    bundle: CFBundleRef,
    /// Path to the module (diagnostics / config record)
    #[allow(dead_code)]
    path: std::path::PathBuf,
    /// bundleExit function pointer (for cleanup)
    bundle_exit: Option<BundleExitFunc>,
    /// GetPluginFactory function pointer
    get_factory_fn: Option<GetPluginFactoryFunc>,
}

impl PrivateNamespaceModule {
    /// Load a VST3 bundle using private namespace isolation
    fn load_internal(path: &Path) -> Result<Self> {
        unsafe {
            log::info!("=== PRIVATE NAMESPACE VST3 MODULE LOADING START ===");
            log::info!(
                "Loading VST3 bundle with symbol isolation: {}",
                path.display()
            );

            // Handle both bundle paths and direct binary paths
            let bundle_path = if path.extension().and_then(|s| s.to_str()) == Some("vst3") {
                path.to_path_buf()
            } else {
                // Find the .vst3 bundle in parent directories
                let mut current = path;
                loop {
                    if let Some(ext) = current.extension() {
                        if ext == "vst3" {
                            break current.to_path_buf();
                        }
                    }
                    match current.parent() {
                        Some(parent) => current = parent,
                        None => {
                            return Err(Error::PluginLoadFailed(
                                "Could not find .vst3 bundle in path hierarchy".to_string(),
                            ))
                        }
                    }
                }
            };

            log::debug!("Using bundle path: {}", bundle_path.display());

            // Step 1: Create CFBundle reference for VST3 compliance
            log::debug!("Step 1: Creating CFBundle reference...");
            let bundle = Self::create_cfbundle(&bundle_path)?;
            log::debug!("CFBundle created successfully");

            // Step 2: Find the actual executable within the bundle
            log::debug!("Step 2: Finding executable binary...");
            let executable_path = Self::find_bundle_executable(bundle, &bundle_path)?;
            log::debug!("Found executable: {}", executable_path.display());

            // Step 3: Load with dlopen using RTLD_LOCAL for private namespace
            log::debug!("Step 3: Loading with dlopen RTLD_LOCAL...");
            let executable_cstring = CString::new(executable_path.to_string_lossy().as_bytes())
                .map_err(|e| Error::PluginLoadFailed(format!("Invalid executable path: {}", e)))?;

            // RTLD_LOCAL = 0x4, RTLD_LAZY = 0x1
            const RTLD_LOCAL: i32 = 0x4;
            const RTLD_LAZY: i32 = 0x1;
            let dl_handle = libc::dlopen(executable_cstring.as_ptr(), RTLD_LOCAL | RTLD_LAZY);

            if dl_handle.is_null() {
                CFRelease(bundle as CFTypeRef);
                let error_msg = std::ffi::CStr::from_ptr(libc::dlerror())
                    .to_string_lossy()
                    .to_string();
                return Err(Error::PluginLoadFailed(format!(
                    "dlopen failed: {}",
                    error_msg
                )));
            }
            log::debug!("dlopen successful with private namespace");

            // Step 4: Get bundleEntry function (REQUIRED)
            log::debug!("Step 4: Getting bundleEntry function...");
            let bundle_entry_name = CString::new("bundleEntry").unwrap();
            let bundle_entry_ptr = libc::dlsym(dl_handle, bundle_entry_name.as_ptr());

            if bundle_entry_ptr.is_null() {
                libc::dlclose(dl_handle);
                CFRelease(bundle as CFTypeRef);
                return Err(Error::PluginLoadFailed(
                    "Bundle does not export required 'bundleEntry' function".to_string(),
                ));
            }

            let bundle_entry: BundleEntryFunc = std::mem::transmute(bundle_entry_ptr);
            log::debug!("bundleEntry function found");

            // Step 5: Get bundleExit function (REQUIRED)
            log::debug!("Step 5: Getting bundleExit function...");
            let bundle_exit_name = CString::new("bundleExit").unwrap();
            let bundle_exit_ptr = libc::dlsym(dl_handle, bundle_exit_name.as_ptr());

            let bundle_exit: Option<BundleExitFunc> = if bundle_exit_ptr.is_null() {
                log::warn!("bundleExit function not found (unusual but proceeding)");
                None
            } else {
                log::debug!("bundleExit function found");
                Some(std::mem::transmute::<*mut c_void, BundleExitFunc>(
                    bundle_exit_ptr,
                ))
            };

            // Step 6: Call bundleEntry (MUST be called before GetPluginFactory)
            log::debug!("Step 6: Calling bundleEntry...");
            let entry_result = bundle_entry(bundle);
            if entry_result == 0 {
                libc::dlclose(dl_handle);
                CFRelease(bundle as CFTypeRef);
                return Err(Error::PluginLoadFailed(
                    "bundleEntry function returned false".to_string(),
                ));
            }
            log::debug!("bundleEntry called successfully");

            // Step 7: Get GetPluginFactory function (REQUIRED)
            log::debug!("Step 7: Getting GetPluginFactory function...");
            let factory_name = CString::new("GetPluginFactory").unwrap();
            let factory_ptr = libc::dlsym(dl_handle, factory_name.as_ptr());

            let get_factory_fn: Option<GetPluginFactoryFunc> = if factory_ptr.is_null() {
                // Cleanup on failure
                if let Some(exit_fn) = bundle_exit {
                    let _ = exit_fn();
                }
                libc::dlclose(dl_handle);
                CFRelease(bundle as CFTypeRef);
                return Err(Error::PluginLoadFailed(
                    "Failed to find GetPluginFactory function".to_string(),
                ));
            } else {
                log::debug!("GetPluginFactory function found");
                Some(std::mem::transmute::<*mut c_void, GetPluginFactoryFunc>(
                    factory_ptr,
                ))
            };

            log::info!("=== PRIVATE NAMESPACE VST3 MODULE LOADING COMPLETE ===");
            log::info!(
                "Bundle loaded with symbol isolation: {}",
                bundle_path.display()
            );

            Ok(PrivateNamespaceModule {
                dl_handle,
                bundle,
                path: bundle_path,
                bundle_exit,
                get_factory_fn,
            })
        }
    }

    /// Create a CFBundle reference from the bundle path
    unsafe fn create_cfbundle(bundle_path: &Path) -> Result<CFBundleRef> {
        let path_cstring = CString::new(bundle_path.to_string_lossy().as_bytes())
            .map_err(|e| Error::PluginLoadFailed(format!("Invalid bundle path: {}", e)))?;

        let url = CFURLCreateFromFileSystemRepresentation(
            ptr::null_mut(),
            path_cstring.as_ptr() as *const u8,
            path_cstring.as_bytes().len() as isize,
            1, // isDirectory - VST3 bundles are directories
        );

        if url.is_null() {
            return Err(Error::PluginLoadFailed(
                "Failed to create CFURL from bundle path".to_string(),
            ));
        }

        let bundle = CFBundleCreate(ptr::null_mut(), url);
        CFRelease(url as CFTypeRef);

        if bundle.is_null() {
            return Err(Error::PluginLoadFailed(
                "Failed to create CFBundle".to_string(),
            ));
        }

        Ok(bundle)
    }

    /// Find the executable binary within the VST3 bundle.
    ///
    /// The bundle's `Info.plist` names it in `CFBundleExecutable`, and that is the only
    /// authoritative answer: `Contents/MacOS` may hold helper tools, an unrelated symlink, or a
    /// second architecture's binary alongside the real one, and directory order is arbitrary.
    /// `CFBundleCopyExecutableURL` resolves the key for us (handling binary plists as well as
    /// XML). Only if the bundle doesn't answer — a malformed or missing `Info.plist` — does this
    /// fall back to picking the first plausible file in `Contents/MacOS`.
    unsafe fn find_bundle_executable(
        bundle: CFBundleRef,
        bundle_path: &Path,
    ) -> Result<std::path::PathBuf> {
        if let Some(path) = Self::declared_executable(bundle) {
            log::debug!("Using CFBundleExecutable: {}", path.display());
            return Ok(path);
        }
        log::warn!(
            "Bundle does not declare a usable CFBundleExecutable ({}); \
             falling back to the first file in Contents/MacOS",
            bundle_path.display()
        );

        // Standard macOS bundle structure: Contents/MacOS/
        let macos_dir = bundle_path.join("Contents").join("MacOS");

        if !macos_dir.exists() {
            return Err(Error::PluginLoadFailed(
                "VST3 bundle missing Contents/MacOS directory".to_string(),
            ));
        }

        // Find the first executable file
        let entries = std::fs::read_dir(&macos_dir)
            .map_err(|e| Error::PluginLoadFailed(format!("Cannot read MacOS directory: {}", e)))?;

        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    // Skip hidden files and known non-executables
                    if !name.starts_with('.') && !name.ends_with(".plist") {
                        log::debug!("Found potential executable: {}", path.display());
                        return Ok(path);
                    }
                }
            }
        }

        Err(Error::PluginLoadFailed(
            "No executable found in VST3 bundle".to_string(),
        ))
    }

    /// The executable a bundle declares via `CFBundleExecutable`, as a filesystem path.
    /// `None` when the key is absent or names a file that isn't there.
    unsafe fn declared_executable(bundle: CFBundleRef) -> Option<std::path::PathBuf> {
        let url = CFBundleCopyExecutableURL(bundle);
        if url.is_null() {
            return None;
        }
        // Create rule: the CFURL wrapper takes ownership of the +1 reference.
        let url: CFURL = TCFType::wrap_under_create_rule(url);
        url.to_path().filter(|p| p.is_file())
    }
}

// SAFETY: dlopen handles are thread-safe and CFBundleRef is immutable after creation
unsafe impl Send for PrivateNamespaceModule {}

impl VstModule for PrivateNamespaceModule {
    fn get_factory(&self) -> Result<*mut IPluginFactory> {
        if let Some(get_factory_fn) = self.get_factory_fn {
            let factory = unsafe { get_factory_fn() };
            if factory.is_null() {
                Err(Error::PluginLoadFailed(
                    "GetPluginFactory returned null".to_string(),
                ))
            } else {
                Ok(factory)
            }
        } else {
            Err(Error::PluginLoadFailed(
                "GetPluginFactory function not available".to_string(),
            ))
        }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for PrivateNamespaceModule {
    fn drop(&mut self) {
        unsafe {
            log::debug!("=== PRIVATE NAMESPACE VST3 MODULE CLEANUP START ===");

            // Step 1: Call bundleExit if available (REQUIRED)
            if let Some(bundle_exit) = self.bundle_exit {
                log::debug!("Calling bundleExit...");
                let exit_result = bundle_exit();
                if exit_result != 0 {
                    log::debug!("bundleExit called successfully");
                } else {
                    log::warn!("bundleExit returned false");
                }
            }

            // Step 2: Close the dlopen handle
            log::debug!("Closing dlopen handle...");
            if libc::dlclose(self.dl_handle) != 0 {
                let error_msg = std::ffi::CStr::from_ptr(libc::dlerror()).to_string_lossy();
                log::warn!("dlclose failed: {}", error_msg);
            } else {
                log::debug!("dlopen handle closed successfully");
            }

            // Step 3: Release CFBundle
            log::debug!("Releasing CFBundle...");
            CFRelease(self.bundle as CFTypeRef);
            log::debug!("CFBundle released");

            log::debug!("=== PRIVATE NAMESPACE VST3 MODULE CLEANUP COMPLETE ===");
        }
    }
}

/// Private namespace module loader implementation
pub struct PrivateNamespaceModuleLoader;

impl ModuleLoader for PrivateNamespaceModuleLoader {
    fn load(path: &Path) -> Result<Box<dyn VstModule>> {
        let module = PrivateNamespaceModule::load_internal(path)?;
        Ok(Box::new(module))
    }
}

#[cfg(test)]
mod bundle_executable_tests {
    use super::*;
    use std::fs;

    /// Build a throwaway `.vst3` bundle whose `Info.plist` names `declared` as the executable,
    /// alongside a decoy binary that a directory scan could pick instead.
    fn fixture_bundle(name: &str, declared: &str, decoy: &str) -> std::path::PathBuf {
        let bundle = std::env::temp_dir().join(format!(
            "vst3-exe-{name}-{}-{}.vst3",
            std::process::id(),
            name
        ));
        let _ = fs::remove_dir_all(&bundle);
        let macos = bundle.join("Contents").join("MacOS");
        fs::create_dir_all(&macos).expect("create bundle dirs");
        fs::write(macos.join(decoy), b"decoy").expect("write decoy");
        fs::write(macos.join(declared), b"real").expect("write executable");
        fs::write(
            bundle.join("Contents").join("Info.plist"),
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key>
    <string>{declared}</string>
    <key>CFBundleIdentifier</key>
    <string>com.example.{name}</string>
    <key>CFBundlePackageType</key>
    <string>BNDL</string>
</dict>
</plist>
"#
            ),
        )
        .expect("write Info.plist");
        bundle
    }

    /// The bundle declares which binary is the plugin; `Contents/MacOS` may hold others, and
    /// directory order is arbitrary. Picking the first file found loads the wrong binary (or a
    /// helper tool that exports no `bundleEntry` at all).
    #[test]
    fn prefers_the_executable_named_by_cfbundleexecutable() {
        let bundle = fixture_bundle("declared", "TheRealPlugin", "AAA_helper_tool");
        unsafe {
            let cf = PrivateNamespaceModule::create_cfbundle(&bundle).expect("create CFBundle");
            let exe = PrivateNamespaceModule::find_bundle_executable(cf, &bundle)
                .expect("resolve executable");
            CFRelease(cf as CFTypeRef);
            assert_eq!(
                exe.file_name().and_then(|n| n.to_str()),
                Some("TheRealPlugin")
            );
        }
        let _ = fs::remove_dir_all(&bundle);
    }

    /// A bundle with no usable `CFBundleExecutable` still loads: the directory scan remains as
    /// the fallback.
    #[test]
    fn falls_back_to_scanning_when_the_plist_declares_nothing() {
        let bundle =
            std::env::temp_dir().join(format!("vst3-exe-bare-{}.vst3", std::process::id()));
        let _ = fs::remove_dir_all(&bundle);
        let macos = bundle.join("Contents").join("MacOS");
        fs::create_dir_all(&macos).expect("create bundle dirs");
        fs::write(macos.join("OnlyBinary"), b"real").expect("write executable");

        unsafe {
            let cf = PrivateNamespaceModule::create_cfbundle(&bundle).expect("create CFBundle");
            let exe = PrivateNamespaceModule::find_bundle_executable(cf, &bundle)
                .expect("resolve executable");
            CFRelease(cf as CFTypeRef);
            assert_eq!(exe.file_name().and_then(|n| n.to_str()), Some("OnlyBinary"));
        }
        let _ = fs::remove_dir_all(&bundle);
    }
}
