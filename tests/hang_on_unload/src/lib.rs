//! A plain pass-through effect whose library hangs when the host unloads it.
//!
//! It models a plugin that starts a worker thread of its own while it renders
//! (a denoiser's thread pool, a cache flusher) and stops it from a static
//! destructor or `DLL_PROCESS_DETACH`: the destructor asks the thread to stop
//! and then *joins* it. A plain `FreeLibrary` runs that handler under the
//! loader lock, and a thread cannot finish exiting without that same lock
//! (every exiting thread runs `DLL_THREAD_DETACH` for each loaded library), so
//! the join never returns and the thread that called `FreeLibrary` hangs.
//!
//! Lux's OpenImageDenoise CPU device does exactly this when its global engine
//! is destroyed, and a host that unloads a plugin after every run hangs there
//! after the render has already finished. (A process exit does not: the
//! handler's `lpReserved` is not null there and the other threads are gone, so
//! this one leaves it alone. `tests/hang_on_exit` covers that.)
//!
//! Unlike `hang_on_exit`, this library does NOT pin itself: the host's unload
//! must reach the handler.
//!
//! The effect itself is ordinary: no parameters, copies its input to its
//! output at 8 and 16 bpc. The hang is Windows-only.
//!
//! The join gives up after [`GIVE_UP`], so a test harness that fails to kill a
//! hung process cannot leave it behind forever. That is far longer than any
//! test waits.

#![allow(non_snake_case)]
#![allow(clippy::missing_safety_doc)]

use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use after_effects_sys::*;

/// Asks the worker thread to finish.
static WORKER_STOP: AtomicBool = AtomicBool::new(false);
/// The worker thread, started by the first render.
static WORKER: Mutex<Option<JoinHandle<()>>> = Mutex::new(None);

/// How long the unload handler waits for the worker before giving up.
#[cfg(windows)]
const GIVE_UP: Duration = Duration::from_secs(60);

fn start_worker() {
	let Ok(mut worker) = WORKER.lock() else { return };
	if worker.is_some() {
		return;
	}
	WORKER_STOP.store(false, Ordering::SeqCst);
	*worker = Some(std::thread::spawn(|| {
		while !WORKER_STOP.load(Ordering::SeqCst) {
			std::thread::sleep(Duration::from_millis(5));
		}
	}));
}

/// What the plugin's static destructor does: stop the worker and join it. The
/// join waits on the thread's handle, which is signalled only once the thread
/// has fully exited, and that needs the loader lock this call is holding.
#[cfg(windows)]
fn stop_worker_and_join() {
	use std::os::windows::io::AsRawHandle;
	unsafe extern "system" {
		fn WaitForSingleObject(handle: *mut c_void, millis: u32) -> u32;
	}
	WORKER_STOP.store(true, Ordering::SeqCst);
	let handle = match WORKER.try_lock() {
		Ok(mut worker) => worker.take(),
		Err(_) => None,
	};
	if let Some(handle) = handle {
		// SAFETY: the handle belongs to the live `JoinHandle` held here.
		unsafe { WaitForSingleObject(handle.as_raw_handle(), GIVE_UP.as_millis() as u32) };
	}
}

/// `DLL_PROCESS_DETACH` with a null `lpReserved` is a `FreeLibrary` that drops
/// the last reference. With a non-null one the process is ending and the
/// worker is already gone, so there is nothing to wait for.
#[cfg(windows)]
#[unsafe(no_mangle)]
pub extern "system" fn DllMain(_module: *mut c_void, reason: u32, reserved: *mut c_void) -> i32 {
	const DLL_PROCESS_DETACH: u32 = 0;
	if reason == DLL_PROCESS_DETACH && reserved.is_null() {
		stop_worker_and_join();
	}
	1
}

unsafe fn render(params: *mut *mut PF_ParamDef, output: *mut PF_LayerDef) -> PF_Err {
	start_worker();
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
