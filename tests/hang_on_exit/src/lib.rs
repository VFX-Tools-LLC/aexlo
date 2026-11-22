//! A plain pass-through effect whose library hangs when the process exits.
//!
//! It models plugins that start a worker thread of their own (a license
//! check, a cache flusher) and stop it from a static destructor or
//! `DLL_PROCESS_DETACH`: the destructor sets a stop flag and then waits for the
//! thread to confirm it has finished. On Windows, `ExitProcess` kills every
//! other thread *before* it runs those handlers, so the confirmation never
//! comes and the process hangs at exit, holding the loader lock. A host that
//! ends the process some other way (`TerminateProcess`, `_exit`) never runs the
//! handler and exits at once.
//!
//! The library also pins itself in memory, as plugins that leave a thread
//! running in their own code do, so the host's own unload never reaches the
//! handler; only the end of the process does.
//!
//! The effect itself is ordinary: no parameters, copies its input to its
//! output at 8 and 16 bpc. The hang is Windows-only (elsewhere a normal exit
//! leaves the other threads alive, so a destructor like this would finish).
//!
//! The wait gives up after [`GIVE_UP`], so a test harness that fails to kill a
//! hung process cannot leave it behind forever. That is far longer than any
//! test waits.

#![allow(non_snake_case)]
#![allow(clippy::missing_safety_doc)]

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use after_effects_sys::*;

/// Set by the worker thread while it runs.
static WORKER_ALIVE: AtomicBool = AtomicBool::new(false);
/// Asks the worker thread to finish.
static WORKER_STOP: AtomicBool = AtomicBool::new(false);

/// How long the exit handler waits for the worker before giving up.
#[cfg(windows)]
const GIVE_UP: Duration = Duration::from_secs(60);

/// Keep the library mapped for the life of the process, as plugins do that
/// leave a thread running in their own code: a `FreeLibrary` from the host
/// then never reaches `DLL_PROCESS_DETACH`; only process exit does.
#[cfg(windows)]
fn pin_module() {
	unsafe extern "system" {
		fn GetModuleHandleExW(flags: u32, address: *const c_void, module: *mut *mut c_void) -> i32;
	}
	const FROM_ADDRESS: u32 = 0x4;
	const PIN: u32 = 0x1;
	let mut module = std::ptr::null_mut();
	// SAFETY: a static of this module is a valid address inside it.
	unsafe { GetModuleHandleExW(FROM_ADDRESS | PIN, (&raw const WORKER_ALIVE).cast(), &mut module) };
}

fn start_worker() {
	if WORKER_ALIVE.swap(true, Ordering::SeqCst) {
		return;
	}
	#[cfg(windows)]
	pin_module();
	std::thread::spawn(|| {
		while !WORKER_STOP.load(Ordering::SeqCst) {
			std::thread::sleep(Duration::from_millis(5));
		}
		// Only reached when someone asks it to stop while the thread lives.
		WORKER_ALIVE.store(false, Ordering::SeqCst);
	});
}

/// What the plugin's static destructor does: stop the worker, wait for it.
#[cfg(windows)]
fn stop_worker_and_wait() {
	WORKER_STOP.store(true, Ordering::SeqCst);
	let started = std::time::Instant::now();
	while WORKER_ALIVE.load(Ordering::SeqCst) && started.elapsed() < GIVE_UP {
		std::thread::sleep(Duration::from_millis(5));
	}
}

/// `DLL_PROCESS_DETACH` with `lpReserved != NULL` means the process is
/// terminating (`ExitProcess`); a null one is a plain `FreeLibrary`, where the
/// worker is still alive and the wait ends at once.
#[cfg(windows)]
#[unsafe(no_mangle)]
pub extern "system" fn DllMain(_module: *mut c_void, reason: u32, reserved: *mut c_void) -> i32 {
	const DLL_PROCESS_DETACH: u32 = 0;
	if reason == DLL_PROCESS_DETACH && !reserved.is_null() {
		stop_worker_and_wait();
	}
	1
}

unsafe fn render(params: *mut *mut PF_ParamDef, output: *mut PF_LayerDef) -> PF_Err {
	let input = unsafe { &(**params).u.ld };
	let output = unsafe { &*output };
	let deep = output.world_flags & PF_WorldFlag_DEEP as PF_WorldFlags != 0;
	let bytes = if deep { 8 } else { 4 };
	let row = output.width.min(input.width) as usize * bytes;
	for y in 0..output.height.min(input.height) as usize {
		unsafe {
			let src = (input.data as *const u8).add(y * input.rowbytes as usize);
			let dst = (output.data as *mut u8).add(y * output.rowbytes as usize);
			std::ptr::copy_nonoverlapping(src, dst, row);
		}
	}
	PF_Err_NONE as PF_Err
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn EffectMain(
	cmd: PF_Cmd,
	_in_data: *mut PF_InData,
	out_data: *mut PF_OutData,
	params: *mut *mut PF_ParamDef,
	output: *mut PF_LayerDef,
	_extra: *mut c_void,
) -> PF_Err {
	#[allow(non_upper_case_globals)]
	match cmd as _ {
		PF_Cmd_GLOBAL_SETUP => {
			start_worker();
			let out = unsafe { &mut *out_data };
			out.my_version = 1 << 19;
			out.out_flags = (PF_OutFlag_DEEP_COLOR_AWARE | PF_OutFlag_PIX_INDEPENDENT) as PF_OutFlags;
			PF_Err_NONE as PF_Err
		}
		PF_Cmd_PARAMS_SETUP => {
			// Only the implicit input layer.
			unsafe { (*out_data).num_params = 1 };
			PF_Err_NONE as PF_Err
		}
		PF_Cmd_RENDER => unsafe { render(params, output) },
		_ => PF_Err_NONE as PF_Err,
	}
}
