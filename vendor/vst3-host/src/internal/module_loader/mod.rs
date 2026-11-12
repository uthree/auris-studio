//! Platform-specific VST3 module loading implementations

use crate::error::Result;
use std::path::Path;
use vst3::Steinberg::IPluginFactory;

/// Trait representing a loaded VST3 module
pub trait VstModule: Send {
    /// Get the plugin factory from the module
    fn get_factory(&self) -> Result<*mut IPluginFactory>;

    /// Get the path to the module (diagnostics / config record)
    #[allow(dead_code)]
    fn path(&self) -> &Path;
}

/// Trait for platform-specific module loaders
pub trait ModuleLoader {
    /// Load a VST3 module from the given path
    fn load(path: &Path) -> Result<Box<dyn VstModule>>;
}

/// Detect if a plugin requires private namespace loading due to C symbol conflicts.
/// Only consulted by the macOS loader (private namespaces are a dyld feature).
#[cfg(target_os = "macos")]
fn requires_private_namespace(path: &Path) -> bool {
    if let Some(filename) = path.file_name().and_then(|f| f.to_str()) {
        let filename_lower = filename.to_lowercase();

        // Plugins with C symbol conflicts (not Objective-C)
        if filename_lower.contains("ozone") ||
           filename_lower.contains("rx") ||      // iZotope RX series
           filename_lower.contains("neutron") || // iZotope Neutron
           filename_lower.contains("insight")
        // iZotope Insight
        {
            log::info!(
                "Detected plugin requiring private namespace for C symbols: {}",
                filename
            );
            return true;
        }
    }

    false
}

/// Load a VST3 module using the appropriate platform-specific loader
pub fn load_module(path: &Path) -> Result<Box<dyn VstModule>> {
    #[cfg(target_os = "macos")]
    {
        if requires_private_namespace(path) {
            // Use private namespace loading for both Objective-C and C symbol conflicts
            log::info!(
                "Using private namespace loader for symbol isolation: {}",
                path.display()
            );
            private_namespace::PrivateNamespaceModuleLoader::load(path)
        } else {
            // Use standard CFBundle loader for plugins without conflicts
            log::debug!("Using standard CFBundle loader: {}", path.display());
            macos::MacOSModuleLoader::load(path)
        }
    }

    #[cfg(target_os = "windows")]
    {
        windows::WindowsModuleLoader::load(path)
    }

    #[cfg(target_os = "linux")]
    {
        linux::LinuxModuleLoader::load(path)
    }

    #[cfg(target_os = "android")]
    {
        linux::LinuxModuleLoader::load(path)
    }
}

/// One load per module file, shared by every handle that asks for it.
///
/// On macOS `CFBundleCreate` hands back the *same* bundle object for the same URL, and
/// `CFBundleLoadExecutable` is not reference counted: a second load is a no-op and the first
/// `CFBundleUnloadExecutable` unmaps the code for everybody. So a discovery pass that loaded a
/// bundle already hosting a live instance would, on dropping its handle, pull the instance's
/// vtables out from under it — the next call into the plugin then jumps into unmapped memory
/// (issue #216). Counting handles per file here, and unloading only when the last one goes,
/// is what the dyld and Win32 loaders already do for `dlopen` and `LoadLibrary`.
///
/// Always compiled (only the macOS loader uses it) so its tests run on every CI host.
#[allow(dead_code)]
pub(crate) mod shared {
    use crate::error::Result;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    struct Entry<T> {
        key: PathBuf,
        handles: usize,
        module: T,
    }

    /// Loaded modules keyed by file, each with the number of handles holding it.
    pub(crate) struct SharedLoads<T> {
        entries: Mutex<Vec<Entry<T>>>,
    }

    impl<T: Copy> SharedLoads<T> {
        /// An empty table, usable as a `static`.
        pub(crate) const fn new() -> Self {
            Self {
                entries: Mutex::new(Vec::new()),
            }
        }

        /// The module loaded from `key`, calling `load` only if no handle holds it yet.
        ///
        /// The table stays locked while `load` runs, so a load and an unload of the same file
        /// can never interleave.
        pub(crate) fn acquire(&self, key: &Path, load: impl FnOnce() -> Result<T>) -> Result<T> {
            let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(entry) = entries.iter_mut().find(|entry| entry.key == key) {
                entry.handles += 1;
                return Ok(entry.module);
            }
            let module = load()?;
            entries.push(Entry {
                key: key.to_path_buf(),
                handles: 1,
                module,
            });
            Ok(module)
        }

        /// Give back one handle on `key`, calling `unload` once the last one is returned.
        pub(crate) fn release(&self, key: &Path, unload: impl FnOnce(T)) {
            let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
            let Some(index) = entries.iter().position(|entry| entry.key == key) else {
                log::warn!(
                    "released a module that was never acquired: {}",
                    key.display()
                );
                return;
            };
            entries[index].handles -= 1;
            if entries[index].handles == 0 {
                let entry = entries.swap_remove(index);
                unload(entry.module);
            }
        }

        /// How many handles hold `key` (tests only).
        #[cfg(test)]
        fn handles(&self, key: &Path) -> usize {
            let entries = self.entries.lock().unwrap();
            entries
                .iter()
                .find(|entry| entry.key == key)
                .map_or(0, |entry| entry.handles)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::cell::Cell;

        #[test]
        fn a_second_handle_shares_the_first_load_and_outlives_it() {
            let loads = SharedLoads::<u32>::new();
            let key = Path::new("/Library/Audio/Plug-Ins/VST3/Synth.vst3");
            let loaded = Cell::new(0);
            let unloaded = Cell::new(0);

            // An instance is open, then a discovery pass loads the same bundle.
            let instance = loads.acquire(key, || {
                loaded.set(loaded.get() + 1);
                Ok(7)
            });
            let discovery = loads.acquire(key, || {
                loaded.set(loaded.get() + 1);
                Ok(8)
            });
            assert_eq!(instance.unwrap(), 7);
            assert_eq!(
                discovery.unwrap(),
                7,
                "the second handle gets the first load"
            );
            assert_eq!(loaded.get(), 1);

            // Discovery finishing must leave the instance's code mapped.
            loads.release(key, |_| unloaded.set(unloaded.get() + 1));
            assert_eq!(unloaded.get(), 0);
            assert_eq!(loads.handles(key), 1);

            loads.release(key, |module| {
                assert_eq!(module, 7);
                unloaded.set(unloaded.get() + 1);
            });
            assert_eq!(unloaded.get(), 1);
            assert_eq!(loads.handles(key), 0);

            // Once gone, the next acquire loads afresh.
            assert_eq!(loads.acquire(key, || Ok(9)).unwrap(), 9);
        }

        #[test]
        fn files_are_counted_apart_and_a_failed_load_holds_nothing() {
            let loads = SharedLoads::<u32>::new();
            let a = Path::new("/a.vst3");
            let b = Path::new("/b.vst3");
            assert!(loads
                .acquire(a, || Err(crate::Error::PluginLoadFailed("no".into())))
                .is_err());
            assert_eq!(loads.handles(a), 0);

            loads.acquire(a, || Ok(1)).unwrap();
            loads.acquire(b, || Ok(2)).unwrap();
            let mut unloaded = Vec::new();
            loads.release(a, |module| unloaded.push(module));
            assert_eq!(unloaded, [1]);
            assert_eq!(loads.handles(b), 1);
        }
    }
}

/// Pure, platform-independent binary-header parsing for architecture-mismatch diagnostics.
///
/// Lives here (always compiled) rather than in the cfg-gated `windows`/`linux` modules so its
/// unit tests run on every CI host, including macOS. The Windows (PE/COFF) and Linux (ELF)
/// loaders call these to turn an opaque "failed to load" into an actionable arch-mismatch
/// message, mirroring the macOS Mach-O diagnostic. Unused on macOS itself, hence the allow.
#[allow(dead_code)]
pub(crate) mod arch {
    /// The architecture this host binary was built for (the plugin must provide it too).
    #[cfg(target_arch = "aarch64")]
    pub(crate) const HOST_ARCH: &str = "arm64";
    #[cfg(target_arch = "x86_64")]
    pub(crate) const HOST_ARCH: &str = "x86_64";
    #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
    pub(crate) const HOST_ARCH: &str = "unknown";

    // PE COFF machine values (always little-endian in the file).
    pub(crate) const PE_MACHINE_I386: u16 = 0x014c;
    pub(crate) const PE_MACHINE_AMD64: u16 = 0x8664;
    pub(crate) const PE_MACHINE_ARM64: u16 = 0xaa64;

    // ELF e_machine values.
    pub(crate) const ELF_MACHINE_386: u16 = 3;
    pub(crate) const ELF_MACHINE_X86_64: u16 = 62;
    pub(crate) const ELF_MACHINE_AARCH64: u16 = 183;

    /// Parse the COFF machine field of a PE image (a Windows `.dll`/`.vst3` binary).
    ///
    /// Layout: `MZ` at 0, `e_lfanew` (u32 LE) at 0x3C, `PE\0\0` at `e_lfanew`, machine (u16 LE)
    /// at `e_lfanew + 4`. Returns `None` if the magic is wrong or the data is truncated.
    pub(crate) fn detect_pe_machine(data: &[u8]) -> Option<u16> {
        if data.len() < 0x40 || data[0] != b'M' || data[1] != b'Z' {
            return None;
        }
        let e_lfanew =
            u32::from_le_bytes([data[0x3C], data[0x3D], data[0x3E], data[0x3F]]) as usize;
        let machine_off = e_lfanew.checked_add(4)?;
        if machine_off.checked_add(2)? > data.len() {
            return None;
        }
        if &data[e_lfanew..e_lfanew + 4] != b"PE\0\0" {
            return None;
        }
        Some(u16::from_le_bytes([
            data[machine_off],
            data[machine_off + 1],
        ]))
    }

    /// Parse the `e_machine` field of an ELF image (a Linux `.so` binary).
    ///
    /// Honors `EI_DATA` (offset 5: 1 = little-endian, 2 = big-endian) when reading the u16 at
    /// offset 18. Returns `None` on bad magic or truncation.
    pub(crate) fn detect_elf_machine(data: &[u8]) -> Option<u16> {
        if data.len() < 20 || &data[0..4] != b"\x7fELF" {
            return None;
        }
        let bytes = [data[18], data[19]];
        Some(if data[5] == 2 {
            u16::from_be_bytes(bytes)
        } else {
            u16::from_le_bytes(bytes)
        })
    }

    /// Human name for a PE machine value, or `None` if we don't map it.
    pub(crate) fn pe_machine_name(machine: u16) -> Option<&'static str> {
        match machine {
            PE_MACHINE_AMD64 => Some("x86_64"),
            PE_MACHINE_ARM64 => Some("arm64"),
            PE_MACHINE_I386 => Some("x86"),
            _ => None,
        }
    }

    /// Human name for an ELF machine value, or `None` if we don't map it.
    pub(crate) fn elf_machine_name(machine: u16) -> Option<&'static str> {
        match machine {
            ELF_MACHINE_X86_64 => Some("x86_64"),
            ELF_MACHINE_AARCH64 => Some("arm64"),
            ELF_MACHINE_386 => Some("x86"),
            _ => None,
        }
    }

    /// If `binary_arch` is known and differs from the host, an actionable sentence; else `None`.
    ///
    /// Returns `None` when the host arch is unknown (an exotic target we don't map) — we can't
    /// reliably claim a mismatch or suggest a "universal/unknown" build.
    pub(crate) fn mismatch_detail(kind: &str, binary_arch: Option<&str>) -> Option<String> {
        if HOST_ARCH == "unknown" {
            return None;
        }
        match binary_arch {
            Some(arch) if arch != HOST_ARCH => Some(format!(
                "Failed to load {kind}: plugin is {arch}-only but this host is {HOST_ARCH}. \
                 Use a universal/{HOST_ARCH} build of the plugin."
            )),
            _ => None,
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// Minimal PE: MZ, e_lfanew=0x80, PE\0\0 header, machine at +4.
        fn pe_fixture(machine: u16) -> Vec<u8> {
            let mut data = vec![0u8; 0x88];
            data[0] = b'M';
            data[1] = b'Z';
            data[0x3C..0x40].copy_from_slice(&0x80u32.to_le_bytes());
            data[0x80..0x84].copy_from_slice(b"PE\0\0");
            data[0x84..0x86].copy_from_slice(&machine.to_le_bytes());
            data
        }

        /// Minimal ELF: magic, EI_DATA endianness at 5, e_machine at 18.
        fn elf_fixture(machine: u16, big_endian: bool) -> Vec<u8> {
            let mut data = vec![0u8; 20];
            data[0..4].copy_from_slice(b"\x7fELF");
            data[5] = if big_endian { 2 } else { 1 };
            let m = if big_endian {
                machine.to_be_bytes()
            } else {
                machine.to_le_bytes()
            };
            data[18..20].copy_from_slice(&m);
            data
        }

        #[test]
        fn pe_machine_detected() {
            assert_eq!(
                detect_pe_machine(&pe_fixture(PE_MACHINE_AMD64)),
                Some(PE_MACHINE_AMD64)
            );
            assert_eq!(
                detect_pe_machine(&pe_fixture(PE_MACHINE_ARM64)),
                Some(PE_MACHINE_ARM64)
            );
            assert_eq!(pe_machine_name(PE_MACHINE_AMD64), Some("x86_64"));
            assert_eq!(pe_machine_name(PE_MACHINE_ARM64), Some("arm64"));
        }

        #[test]
        fn pe_rejects_garbage_and_truncation() {
            assert_eq!(detect_pe_machine(&[0u8; 4]), None);
            assert_eq!(
                detect_pe_machine(b"not a pe file at all..............."),
                None
            );
            let mut short = pe_fixture(PE_MACHINE_AMD64);
            short.truncate(0x82); // cut off the machine field
            assert_eq!(detect_pe_machine(&short), None);
        }

        #[test]
        fn elf_machine_detected_both_endians() {
            assert_eq!(
                detect_elf_machine(&elf_fixture(ELF_MACHINE_X86_64, false)),
                Some(ELF_MACHINE_X86_64)
            );
            assert_eq!(
                detect_elf_machine(&elf_fixture(ELF_MACHINE_AARCH64, false)),
                Some(ELF_MACHINE_AARCH64)
            );
            // Big-endian EI_DATA must be honored.
            assert_eq!(
                detect_elf_machine(&elf_fixture(ELF_MACHINE_AARCH64, true)),
                Some(ELF_MACHINE_AARCH64)
            );
            assert_eq!(elf_machine_name(ELF_MACHINE_X86_64), Some("x86_64"));
            assert_eq!(elf_machine_name(ELF_MACHINE_AARCH64), Some("arm64"));
        }

        #[test]
        fn elf_rejects_garbage() {
            assert_eq!(detect_elf_machine(&[0u8; 8]), None);
            assert_eq!(detect_elf_machine(b"\x7fELF"), None); // too short
        }

        #[test]
        fn mismatch_detail_is_actionable() {
            // A binary whose arch differs from the host yields a message naming both.
            let other = if HOST_ARCH == "arm64" {
                "x86_64"
            } else {
                "arm64"
            };
            let msg =
                mismatch_detail("DLL", Some(other)).expect("mismatch should produce a detail");
            assert!(msg.contains(other), "names the binary arch: {msg}");
            assert!(msg.contains(HOST_ARCH), "names the host arch: {msg}");
            // Matching arch (or unknown) yields nothing.
            assert_eq!(mismatch_detail("DLL", Some(HOST_ARCH)), None);
            assert_eq!(mismatch_detail("DLL", None), None);
        }
    }
}

// Platform-specific modules
#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "macos")]
pub mod private_namespace;

#[cfg(target_os = "windows")]
pub mod windows;

#[cfg(any(target_os = "linux", target_os = "android"))]
pub mod linux;
