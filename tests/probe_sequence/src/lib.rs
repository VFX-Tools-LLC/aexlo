//! An effect that stores sequence data in `PF_Cmd_SEQUENCE_SETUP` and reads it
//! back through `PF_EffectSequenceDataSuite1`, the way a plugin does at render
//! time when After Effects hides `in_data->sequence_data` from it.
//!
//! Written against the raw SDK (no `after-effects` crate): every call goes
//! through `SPBasicSuite::AcquireSuite` and the suite table, as a shipped
//! plugin's would. The report is a list of 16-bit slots written into the output
//! world, one slot per pixel (8 bpc): red holds the high byte, green the low
//! byte. `tests/e2e/tests/effect_sequence_data_suite.rs` decodes it.

#![allow(non_snake_case)]
#![allow(clippy::missing_safety_doc)]

use std::ffi::c_void;

use after_effects_sys::*;

/// What the effect stores in its sequence data.
pub const MARKER: u32 = 0xC0DE_5EED;

/// Slot numbers.
pub mod slot {
	/// Always `MAGIC`, so a render that never ran the probe is recognisable.
	pub const MAGIC_AT: usize = 0;
	pub const MAGIC: u16 = 0x5351;
	/// `AcquireSuite("PF Effect Sequence Data Suite", 1)`; 0 when served.
	pub const ACQUIRE_ERR: usize = 1;
	/// `PF_GetConstSequenceData(effect_ref)`.
	pub const GET_ERR: usize = 2;
	/// 1 when the handle it returned is not null.
	pub const HANDLE_SET: usize = 3;
	/// The low and high 16 bits of the `u32` found through the handle.
	pub const MARKER_LOW: usize = 4;
	pub const MARKER_HIGH: usize = 5;
	/// 1 when the handle is the `in_data->sequence_data` handle the effect allocated
	/// (not a static: instances share the loaded library).
	pub const SAME_HANDLE: usize = 6;
	/// `PF_GetConstSequenceData(NULL)`: refused.
	pub const NULL_REF_ERR: usize = 7;
	/// `PF_GetConstSequenceData(effect_ref, NULL)`: refused, no crash.
	pub const NULL_OUT_ERR: usize = 8;
	pub const TOTAL: usize = 9;
}

fn clamp16(v: i64) -> u16 {
	v.clamp(0, 0xffff) as u16
}

unsafe fn probe(in_data: &PF_InData, slots: &mut [u16]) {
	slots[slot::MAGIC_AT] = slot::MAGIC;

	let basic = in_data.pica_basicP;
	let mut suite: *const c_void = std::ptr::null();
	let err = unsafe {
		(*basic).AcquireSuite.unwrap()(
			kPFEffectSequenceDataSuite.as_ptr() as *const _,
			kPFEffectSequenceDataSuiteVersion1 as i32,
			&mut suite,
		)
	};
	slots[slot::ACQUIRE_ERR] = clamp16(if err != 0 || !suite.is_null() {
		err as i64
	} else {
		0xfffe
	});
	if err != 0 || suite.is_null() {
		return;
	}
	let suite = unsafe { &*(suite as *const PF_EffectSequenceDataSuite1) };
	let get = suite.PF_GetConstSequenceData.unwrap();

	let mut handle: PF_ConstHandle = std::ptr::null();
	let err = unsafe { get(in_data.effect_ref, &mut handle) };
	slots[slot::GET_ERR] = clamp16(err as i64);
	slots[slot::HANDLE_SET] = !handle.is_null() as u16;
	if !handle.is_null() {
		let data = unsafe { *handle } as *const u32;
		if !data.is_null() {
			let value = unsafe { data.read_unaligned() };
			slots[slot::MARKER_LOW] = (value & 0xffff) as u16;
			slots[slot::MARKER_HIGH] = (value >> 16) as u16;
		}
		slots[slot::SAME_HANDLE] = (handle as usize == in_data.sequence_data as usize) as u16;
	}

	let mut unused: PF_ConstHandle = std::ptr::null();
	let err = unsafe { get(std::ptr::null_mut(), &mut unused) };
	slots[slot::NULL_REF_ERR] = clamp16(err as i64);
	let err = unsafe { get(in_data.effect_ref, std::ptr::null_mut()) };
	slots[slot::NULL_OUT_ERR] = clamp16(err as i64);
}

unsafe fn sequence_setup(in_data: &PF_InData, out_data: &mut PF_OutData) -> PF_Err {
	let utils = unsafe { &*in_data.utils };
	let handle = unsafe { utils.host_new_handle.unwrap()(std::mem::size_of::<u32>() as _) };
	if handle.is_null() {
		return PF_Err_OUT_OF_MEMORY as PF_Err;
	}
	let data = unsafe { utils.host_lock_handle.unwrap()(handle) } as *mut u32;
	unsafe { data.write_unaligned(MARKER) };
	unsafe { utils.host_unlock_handle.unwrap()(handle) };
	out_data.sequence_data = handle;
	PF_Err_NONE as PF_Err
}

unsafe fn render(in_data: *mut PF_InData, output: *mut PF_LayerDef) -> PF_Err {
	let (in_data, output) = unsafe { (&*in_data, &*output) };
	let mut slots = vec![0u16; slot::TOTAL];
	unsafe { probe(in_data, &mut slots) };

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
		PF_Cmd_SEQUENCE_SETUP => unsafe { sequence_setup(&*in_data, &mut *out_data) },
		PF_Cmd_RENDER => unsafe { render(in_data, output) },
		_ => PF_Err_NONE as PF_Err,
	}
}
