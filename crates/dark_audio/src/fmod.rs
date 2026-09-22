//! The few FMOD Studio C API calls the engine uses, loaded from the runtime library at run time.
//!
//! This is the only unsafe code in the engine: calling a C library. FMOD is proprietary and is
//! not linked at build time, so the engine builds, tests and runs (silently) without it; a game
//! ships the runtime library it was licensed with.
#![allow(unsafe_code)]

use std::collections::HashMap;
use std::ffi::{CString, c_char, c_int, c_uint, c_void};
use std::path::{Path, PathBuf};

use libloading::Library;

use crate::AudioError;

type Handle = *mut c_void;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Vector {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub(crate) struct Attributes3d {
    pub position: Vector,
    pub velocity: Vector,
    pub forward: Vector,
    pub up: Vector,
}

impl Attributes3d {
    /// At `position`, still, facing +z with +y up (the listener's frame for a top-down game).
    pub fn at(position: Vector) -> Self {
        Self {
            position,
            velocity: Vector::default(),
            forward: Vector {
                x: 0.0,
                y: 0.0,
                z: 1.0,
            },
            up: Vector {
                x: 0.0,
                y: 1.0,
                z: 0.0,
            },
        }
    }
}

/// Entry points, copied out of the library; valid while `Studio::_library` lives.
struct Api {
    create: unsafe extern "C" fn(*mut Handle, c_uint) -> c_int,
    initialize: unsafe extern "C" fn(Handle, c_int, c_uint, c_uint, *mut c_void) -> c_int,
    load_bank: unsafe extern "C" fn(Handle, *const c_char, c_uint, *mut Handle) -> c_int,
    get_event: unsafe extern "C" fn(Handle, *const c_char, *mut Handle) -> c_int,
    create_instance: unsafe extern "C" fn(Handle, *mut Handle) -> c_int,
    set_3d: unsafe extern "C" fn(Handle, *const Attributes3d) -> c_int,
    start: unsafe extern "C" fn(Handle) -> c_int,
    release_instance: unsafe extern "C" fn(Handle) -> c_int,
    set_listener: unsafe extern "C" fn(Handle, c_int, *const Attributes3d, *const Vector) -> c_int,
    update: unsafe extern "C" fn(Handle) -> c_int,
    release: unsafe extern "C" fn(Handle) -> c_int,
}

/// An initialised FMOD Studio system with banks loaded.
pub(crate) struct Studio {
    api: Api,
    system: Handle,
    /// Event descriptions by path; `None` for paths the banks do not have (asked once).
    events: HashMap<String, Option<Handle>>,
    // Last, so it is dropped after `Drop::drop` has released the system.
    _library: Library,
}

fn check(call: &'static str, code: c_int) -> Result<(), AudioError> {
    if code == 0 {
        Ok(())
    } else {
        Err(AudioError::Fmod { call, code })
    }
}

impl Studio {
    /// Loads the library at `library`, starts FMOD Studio (`header_version` as FMOD numbers it,
    /// e.g. 0x00020230 for 2.02.30) and loads `banks` in order.
    pub fn open(
        library: &Path,
        banks: &[PathBuf],
        header_version: u32,
    ) -> Result<Self, AudioError> {
        // SAFETY: loading FMOD's runtime library runs its initialisers, which have no
        // preconditions of ours.
        let lib = unsafe { Library::new(library) }.map_err(|source| AudioError::Library {
            path: library.to_owned(),
            source,
        })?;
        macro_rules! symbol {
            ($name:literal) => {
                // SAFETY: the type matches the function's declaration in fmod_studio.h.
                *unsafe { lib.get(concat!($name, "\0").as_bytes()) }.map_err(|source| {
                    AudioError::Symbol {
                        name: $name,
                        source,
                    }
                })?
            };
        }
        let api = Api {
            create: symbol!("FMOD_Studio_System_Create"),
            initialize: symbol!("FMOD_Studio_System_Initialize"),
            load_bank: symbol!("FMOD_Studio_System_LoadBankFile"),
            get_event: symbol!("FMOD_Studio_System_GetEvent"),
            create_instance: symbol!("FMOD_Studio_EventDescription_CreateInstance"),
            set_3d: symbol!("FMOD_Studio_EventInstance_Set3DAttributes"),
            start: symbol!("FMOD_Studio_EventInstance_Start"),
            release_instance: symbol!("FMOD_Studio_EventInstance_Release"),
            set_listener: symbol!("FMOD_Studio_System_SetListenerAttributes"),
            update: symbol!("FMOD_Studio_System_Update"),
            release: symbol!("FMOD_Studio_System_Release"),
        };
        let mut system: Handle = std::ptr::null_mut();
        // SAFETY: `system` is a valid out-pointer.
        check("System_Create", unsafe {
            (api.create)(&mut system, header_version)
        })?;
        let studio = Self {
            api,
            system,
            events: HashMap::new(),
            _library: lib,
        };
        // 512 voices, normal Studio and Core flags, no driver data.
        // SAFETY: `system` came from System_Create and is initialised once.
        check("System_Initialize", unsafe {
            (studio.api.initialize)(studio.system, 512, 0, 0, std::ptr::null_mut())
        })?;
        for bank in banks {
            let path = CString::new(bank.to_string_lossy().as_bytes())
                .map_err(|_| AudioError::BadPath(bank.clone()))?;
            let mut handle: Handle = std::ptr::null_mut();
            // SAFETY: valid system, NUL-terminated path, valid out-pointer.
            check("LoadBankFile", unsafe {
                (studio.api.load_bank)(studio.system, path.as_ptr(), 0, &mut handle)
            })
            .map_err(|e| AudioError::Bank {
                path: bank.clone(),
                source: Box::new(e),
            })?;
        }
        Ok(studio)
    }

    /// Starts a one-shot of `event` at `at`; it releases itself when done. Unknown events are
    /// reported once and then ignored.
    pub fn play(&mut self, event: &str, at: Vector) -> Result<(), AudioError> {
        let description = match self.events.get(event) {
            Some(known) => *known,
            None => {
                let found = CString::new(event).ok().and_then(|path| {
                    let mut handle: Handle = std::ptr::null_mut();
                    // SAFETY: valid system, NUL-terminated path, valid out-pointer.
                    let code =
                        unsafe { (self.api.get_event)(self.system, path.as_ptr(), &mut handle) };
                    (code == 0).then_some(handle)
                });
                if found.is_none() {
                    tracing::warn!("no FMOD event {event} in the loaded banks");
                }
                self.events.insert(event.to_owned(), found);
                found
            }
        };
        let Some(description) = description else {
            return Ok(());
        };
        let mut instance: Handle = std::ptr::null_mut();
        // SAFETY: `description` came from GetEvent on this system; out-pointer valid.
        check("CreateInstance", unsafe {
            (self.api.create_instance)(description, &mut instance)
        })?;
        let attributes = Attributes3d::at(at);
        // SAFETY: `instance` was just created; the attributes outlive the calls. Released whatever
        // happens: after Start, FMOD frees the one-shot when it finishes; if Start failed, now.
        let started = unsafe {
            check("Set3DAttributes", (self.api.set_3d)(instance, &attributes))
                .and_then(|()| check("Start", (self.api.start)(instance)))
        };
        // SAFETY: as above; the instance is released exactly once.
        let released = check("Release", unsafe { (self.api.release_instance)(instance) });
        started.and(released)
    }

    /// Whether `event` was found in the banks (after being played or looked up).
    #[cfg(test)]
    pub fn found(&self, event: &str) -> bool {
        self.events.get(event).is_some_and(Option::is_some)
    }

    pub fn set_listener(&mut self, at: Vector) -> Result<(), AudioError> {
        let attributes = Attributes3d::at(at);
        // SAFETY: valid system; listener 0 always exists; null attenuation position means "the
        // listener's own".
        check("SetListenerAttributes", unsafe {
            (self.api.set_listener)(self.system, 0, &attributes, std::ptr::null())
        })
    }

    pub fn update(&mut self) -> Result<(), AudioError> {
        // SAFETY: valid system.
        check("Update", unsafe { (self.api.update)(self.system) })
    }
}

impl Drop for Studio {
    fn drop(&mut self) {
        // SAFETY: the system is released once, before the library is unloaded.
        let code = unsafe { (self.api.release)(self.system) };
        if code != 0 {
            tracing::warn!("FMOD System_Release failed ({code})");
        }
    }
}
