//! An effect that reads the layer's masks through the real C ABI and reports
//! what it read, so a host test can check the Layer Mask Suite end to end.
//!
//! Written against the raw SDK (no `after-effects` crate): every call goes
//! through `SPBasicSuite::AcquireSuite` and the suite tables, as a shipped
//! plugin's would. The effect has no parameters beyond the input layer.
//!
//! The report is a list of 16-bit slots written into the output world, one slot
//! per pixel (8 bpc): the red channel holds the high byte, green the low byte,
//! alpha is opaque. [`Slot`] documents the layout; `tests/e2e/tests/layer_mask_suite.rs`
//! decodes it. Errors are reported as the raw `A_Err` the suite returned.

#![allow(non_snake_case)]
#![allow(clippy::missing_safety_doc)]

use std::ffi::c_void;

use after_effects_sys::*;

/// Slot numbers. Per-mask groups start at `MASK_BASE + index * MASK_STRIDE`.
pub mod slot {
	/// Always `MAGIC`, so a render that never ran the probe is recognisable.
	pub const MAGIC_AT: usize = 0;
	pub const MAGIC: u16 = 0x4d4b;
	/// `AcquireSuite("AEGP Layer Mask Suite", 7)` result; 0 when served.
	pub const ACQUIRE_ERR: usize = 1;
	/// `AEGP_GetEffectLayer` result.
	pub const LAYER_ERR: usize = 2;
	/// `AEGP_GetLayerNumMasks` result.
	pub const NUM_MASKS_ERR: usize = 3;
	/// The mask count it wrote.
	pub const NUM_MASKS: usize = 4;
	/// `AEGP_GetLayerMaskByIndex(count)`: one past the last mask.
	pub const BY_INDEX_PAST_END_ERR: usize = 5;
	/// `AEGP_GetLayerMaskByIndex(-1)`.
	pub const BY_INDEX_NEGATIVE_ERR: usize = 6;
	/// `AEGP_GetLayerMaskByIndex` past the end must also leave the handle null.
	pub const BY_INDEX_PAST_END_HANDLE_NULL: usize = 7;
	pub const MASK_BASE: usize = 8;
	pub const MASK_STRIDE: usize = 8;
	/// How many masks are reported in detail.
	pub const MAX_MASKS: usize = 3;
	// Offsets inside one mask's group.
	pub const GET_ERR: usize = 0;
	pub const INVERT: usize = 1;
	pub const MODE: usize = 2;
	pub const ID: usize = 3;
	pub const LOCKED: usize = 4;
	pub const ROTO: usize = 5;
	/// First `AEGP_DisposeMask`.
	pub const DISPOSE_ERR: usize = 6;
	/// Second `AEGP_DisposeMask` on the same handle (must be refused).
	pub const DOUBLE_DISPOSE_ERR: usize = 7;
	pub const TOTAL: usize = MASK_BASE + MASK_STRIDE * MAX_MASKS;
}

fn clamp16(v: i64) -> u16 {
	v.clamp(0, 0xffff) as u16
}

unsafe fn acquire<T>(in_data: &PF_InData, name: &[u8], version: u32) -> Result<*const T, i32> {
	let basic = in_data.pica_basicP;
	if basic.is_null() {
		return Err(-1);
	}
	let Some(acquire) = (unsafe { (*basic).AcquireSuite }) else {
		return Err(-2);
	};
	let mut suite: *const c_void = std::ptr::null();
	let err = unsafe { acquire(name.as_ptr() as *const _, version as i32, &mut suite) };
	if err != 0 || suite.is_null() {
		return Err(if err != 0 { err } else { -3 });
	}
	Ok(suite as *const T)
}

/// Read every mask the layer has and fill `slots`.
unsafe fn probe(in_data: &PF_InData, slots: &mut [u16]) {
	slots[slot::MAGIC_AT] = slot::MAGIC;

	let masks: *const AEGP_MaskSuite6 = match unsafe { acquire(in_data, kAEGPMaskSuite, kAEGPMaskSuiteVersion6) } {
		Ok(s) => s,
		Err(e) => {
			slots[slot::ACQUIRE_ERR] = clamp16(if e == 0 { 0xfffe } else { e as i64 & 0xffff });
			return;
		}
	};
	let interface: *const AEGP_PFInterfaceSuite1 =
		match unsafe { acquire(in_data, kAEGPPFInterfaceSuite, kAEGPPFInterfaceSuiteVersion1) } {
			Ok(s) => s,
			Err(_) => {
				slots[slot::LAYER_ERR] = 0xfffd;
				return;
			}
		};
	let (masks, interface) = unsafe { (&*masks, &*interface) };

	let mut layer: AEGP_LayerH = std::ptr::null_mut();
	let err = unsafe { interface.AEGP_GetEffectLayer.unwrap()(in_data.effect_ref, &mut layer) };
	slots[slot::LAYER_ERR] = clamp16(err as i64);
	if err != 0 {
		return;
	}

	let mut count: A_long = -1;
	let err = unsafe { masks.AEGP_GetLayerNumMasks.unwrap()(layer, &mut count) };
	slots[slot::NUM_MASKS_ERR] = clamp16(err as i64);
	slots[slot::NUM_MASKS] = clamp16(count as i64);
	if err != 0 {
		return;
	}

	// Out-of-range indices are refused and leave the handle null.
	let mut handle: AEGP_MaskRefH = 1usize as AEGP_MaskRefH;
	let err = unsafe { masks.AEGP_GetLayerMaskByIndex.unwrap()(layer, count, &mut handle) };
	slots[slot::BY_INDEX_PAST_END_ERR] = clamp16(err as i64);
	slots[slot::BY_INDEX_PAST_END_HANDLE_NULL] = handle.is_null() as u16;
	let mut handle: AEGP_MaskRefH = std::ptr::null_mut();
	let err = unsafe { masks.AEGP_GetLayerMaskByIndex.unwrap()(layer, -1, &mut handle) };
	slots[slot::BY_INDEX_NEGATIVE_ERR] = clamp16(err as i64);

	for index in 0..(count.max(0) as usize).min(slot::MAX_MASKS) {
		let base = slot::MASK_BASE + index * slot::MASK_STRIDE;
		let mut handle: AEGP_MaskRefH = std::ptr::null_mut();
		let err = unsafe { masks.AEGP_GetLayerMaskByIndex.unwrap()(layer, index as AEGP_MaskIndex, &mut handle) };
		slots[base + slot::GET_ERR] = clamp16(err as i64);
		if err != 0 || handle.is_null() {
			continue;
		}

		let mut invert: A_Boolean = 9;
		unsafe { masks.AEGP_GetMaskInvert.unwrap()(handle, &mut invert) };
		slots[base + slot::INVERT] = clamp16(invert as i64);

		let mut mode: PF_MaskMode = -1;
		unsafe { masks.AEGP_GetMaskMode.unwrap()(handle, &mut mode) };
		slots[base + slot::MODE] = clamp16(mode as i64);

		let mut id: AEGP_MaskIDVal = -1;
		unsafe { masks.AEGP_GetMaskID.unwrap()(handle, &mut id) };
		slots[base + slot::ID] = clamp16(id as i64);

		let mut locked: A_Boolean = 9;
		unsafe { masks.AEGP_GetMaskLockState.unwrap()(handle, &mut locked) };
		slots[base + slot::LOCKED] = clamp16(locked as i64);

		let mut roto: A_Boolean = 9;
		unsafe { masks.AEGP_GetMaskIsRotoBezier.unwrap()(handle, &mut roto) };
		slots[base + slot::ROTO] = clamp16(roto as i64);

		let err = unsafe { masks.AEGP_DisposeMask.unwrap()(handle) };
		slots[base + slot::DISPOSE_ERR] = clamp16(err as i64);
		let err = unsafe { masks.AEGP_DisposeMask.unwrap()(handle) };
		slots[base + slot::DOUBLE_DISPOSE_ERR] = clamp16(err as i64);
	}
}

unsafe fn render(in_data: *mut PF_InData, output: *mut PF_LayerDef) -> PF_Err {
	let (in_data, output) = unsafe { (&*in_data, &*output) };
	let mut slots = vec![0u16; slot::TOTAL];
	unsafe { probe(in_data, &mut slots) };

	// One slot per pixel, row-major, 8 bpc ARGB: A, R = high byte, G = low byte.
	for (i, value) in slots.iter().enumerate() {
		let (x, y) = (i as i32 % output.width, i as i32 / output.width);
		if y >= output.height {
			break;
		}
		let px = unsafe { (output.data as *mut u8).add(y as usize * output.rowbytes as usize + x as usize * 4) };
		unsafe {
			px.write(255);
			px.add(1).write((value >> 8) as u8);
			px.add(2).write((value & 0xff) as u8);
			px.add(3).write(0);
		}
	}
	PF_Err_NONE as PF_Err
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn EffectMain(
	cmd: PF_Cmd,
	in_data: *mut PF_InData,
	out_data: *mut PF_OutData,
	_params: *mut *mut PF_ParamDef,
	output: *mut PF_LayerDef,
	_extra: *mut c_void,
) -> PF_Err {
	#[allow(non_upper_case_globals)]
	match cmd as _ {
		PF_Cmd_GLOBAL_SETUP => {
			let out = unsafe { &mut *out_data };
			out.my_version = 1 << 19;
			out.out_flags = PF_OutFlag_PIX_INDEPENDENT as PF_OutFlags;
			PF_Err_NONE as PF_Err
		}
		PF_Cmd_PARAMS_SETUP => {
			unsafe { (*out_data).num_params = 1 };
			PF_Err_NONE as PF_Err
		}
		PF_Cmd_RENDER => unsafe { render(in_data, output) },
		_ => PF_Err_NONE as PF_Err,
	}
}
