//! Loading VST3 bundles on Linux, where the plugin is a shared object in
//! `Contents/<arch>-linux`.

use std::path::{Path, PathBuf};

use libloading::os::unix::{Library as UnixLibrary, RTLD_LOCAL, RTLD_NOW};

/// The folder with the bundle's binaries for this CPU.
pub fn binary_dir(bundle: &Path) -> PathBuf {
    let arch = match std::env::consts::ARCH {
        "x86" => "i386",
        arch => arch,
    };
    bundle.join("Contents").join(format!("{arch}-linux"))
}

/// The bundle's shared object: the one named after the bundle, or else the
/// only one in the folder, since plugins may ship helper libraries next to it.
pub(super) fn executable_path(bundle: &Path) -> Result<PathBuf, String> {
    let directory = binary_dir(bundle);
    if let Some(stem) = bundle.file_stem() {
        let named = directory.join(format!("{}.so", stem.to_string_lossy()));
        if named.is_file() {
            return Ok(named);
        }
    }
    let entries = std::fs::read_dir(&directory).map_err(|e| format!("cannot read {}: {e}", directory.display()))?;
    let binaries: Vec<PathBuf> =
        entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "so") && p.is_file()).collect();
    match binaries.as_slice() {
        [binary] => Ok(binary.clone()),
        [] => Err(format!("no .so in {}", directory.display())),
        _ => Err(format!("more than one .so in {}", directory.display())),
    }
}

type ModuleEntry = unsafe extern "C" fn(*mut std::ffi::c_void) -> bool;
type ModuleExit = unsafe extern "C" fn() -> bool;

/// A plugin's shared object, initialized with `ModuleEntry`. Dropping it
/// calls `ModuleExit`, which only happens when loading the factory fails:
/// loaded modules stay cached for the life of the process.
pub(super) struct Library {
    pub(super) inner: UnixLibrary,
    exit: ModuleExit,
}

impl Library {
    pub(super) fn open(path: &Path) -> Result<Self, String> {
        let library = unsafe { UnixLibrary::open(Some(path), RTLD_NOW | RTLD_LOCAL) }.map_err(|e| e.to_string())?;
        let entry = *unsafe { library.get::<ModuleEntry>(b"ModuleEntry\0") }.map_err(|e| format!("no ModuleEntry: {e}"))?;
        let exit = *unsafe { library.get::<ModuleExit>(b"ModuleExit\0") }.map_err(|e| format!("no ModuleExit: {e}"))?;
        // ModuleEntry takes the dlopen handle.
        let handle = library.into_raw();
        let library = unsafe { UnixLibrary::from_raw(handle) };
        if !unsafe { entry(handle) } {
            return Err("ModuleEntry failed".into());
        }
        Ok(Library { inner: library, exit })
    }
}

impl Drop for Library {
    fn drop(&mut self) {
        // Runs before `inner` unloads the library.
        unsafe { (self.exit)() };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A temporary `Example.vst3` bundle.
    struct Bundle(PathBuf);

    impl Bundle {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!("daw-vst3-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(binary_dir(&root.join("Example.vst3"))).unwrap();
            Bundle(root)
        }

        fn path(&self) -> PathBuf {
            self.0.join("Example.vst3")
        }

        fn binary(&self, name: &str) -> PathBuf {
            binary_dir(&self.path()).join(name)
        }

        /// Compile C source into the bundle's `Example.so`.
        fn compile(&self, source: &str) -> PathBuf {
            let source_path = self.0.join("plugin.c");
            std::fs::write(&source_path, source).unwrap();
            let binary = self.binary("Example.so");
            let output = std::process::Command::new("cc")
                .args(["-shared", "-fPIC", "-o"])
                .arg(&binary)
                .arg(&source_path)
                .arg("-ldl")
                .output()
                .expect("these tests need a C compiler");
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
            binary
        }
    }

    impl Drop for Bundle {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn finds_the_bundle_binary() {
        let bundle = Bundle::new("find");
        assert!(executable_path(&bundle.path()).unwrap_err().contains("no .so"));
        std::fs::write(bundle.binary("Other.so"), []).unwrap();
        assert_eq!(executable_path(&bundle.path()).unwrap(), bundle.binary("Other.so"));
        std::fs::write(bundle.binary("Helper.so"), []).unwrap();
        assert!(executable_path(&bundle.path()).unwrap_err().contains("more than one"));
        std::fs::write(bundle.binary("Example.so"), []).unwrap();
        assert_eq!(executable_path(&bundle.path()).unwrap(), bundle.binary("Example.so"));
    }

    #[test]
    fn module_entry_gets_the_dlopen_handle_and_exit_runs_on_drop() {
        let bundle = Bundle::new("entry");
        let marker = bundle.0.join("exited");
        let binary = bundle.compile(&format!(
            r#"
            #include <stdbool.h>
            #include <dlfcn.h>
            #include <stdio.h>
            int marker;
            bool ModuleEntry(void *handle) {{ return dlsym(handle, "marker") == &marker; }}
            bool ModuleExit(void) {{ fclose(fopen("{}", "w")); return true; }}
            "#,
            marker.display()
        ));
        let library = Library::open(&binary).unwrap();
        assert!(!marker.exists());
        drop(library);
        assert!(marker.exists());
    }

    #[test]
    fn failed_factory_load_exits_the_module() {
        let bundle = Bundle::new("factory");
        let marker = bundle.0.join("exited");
        bundle.compile(&format!(
            r#"
            #include <stdbool.h>
            #include <stdio.h>
            bool ModuleEntry(void *handle) {{ return true; }}
            bool ModuleExit(void) {{ fclose(fopen("{}", "w")); return true; }}
            void *GetPluginFactory(void) {{ return 0; }}
            "#,
            marker.display()
        ));
        let error = super::super::load_module(&bundle.path()).err().unwrap().to_string();
        assert!(error.contains("GetPluginFactory returned null"), "{error}");
        assert!(marker.exists());
    }
}
