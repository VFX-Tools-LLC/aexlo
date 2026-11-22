//! An effect that reads the project through the real C ABI and reports what it
//! read, so a host test can check the AEGP Proj Suite end to end.
//!
//! Written against the raw SDK (no `after-effects` crate): every call goes
//! through `SPBasicSuite::AcquireSuite` and the suite table, as a shipped
//! plugin's would. The effect has no parameters beyond the input layer.
//!
//! The report is a list of 16-bit slots written into the output world, one
//! slot per pixel: the red channel holds the high byte, green the low byte,
//! alpha is opaque. [`slot`] documents the layout; `tests/e2e/tests/proj_suite.rs`
//! decodes it. Errors are reported as the raw `A_Err` the suite returned.
//! The output may be 8, 16 or 32 bpc; each byte is written at full scale of
//! the depth, so the host's 8 bpc conversion gives the byte back exactly.

#![allow(non_snake_case)]
#![allow(clippy::missing_safety_doc)]

use std::ffi::c_void;

use after_effects_sys::*;

pub mod slot {
	/// Always `MAGIC`, so a render that never ran the probe is recognisable.
	pub const MAGIC_AT: usize = 0;
	pub const MAGIC: u16 = 0x5052;
	/// `AcquireSuite("AEGP Proj Suite", 9)` result; 0 when served.
	pub const ACQUIRE_ERR: usize = 1;
	/// `AEGP_GetNumProjects` result and the count it wrote.
	pub const NUM_ERR: usize = 2;
	pub const NUM: usize = 3;
	/// `AEGP_GetProjectByIndex(0)` result, and whether the handle is non-null.
	pub const BY_INDEX_ERR: usize = 4;
	pub const BY_INDEX_HANDLE: usize = 5;
	/// `AEGP_GetProjectByIndex(1)`: past the only project, refused, handle left null.
	pub const BY_INDEX_PAST_END_ERR: usize = 6;
	pub const BY_INDEX_PAST_END_HANDLE_NULL: usize = 7;
	/// `AEGP_GetProjectByIndex(-1)`, refused.
	pub const BY_INDEX_NEGATIVE_ERR: usize = 8;
	/// `AEGP_GetProjectTimeDisplay` result, then its fields.
	pub const TD_ERR: usize = 9;
	pub const TD_MODE: usize = 10;
	pub const TD_FOOTAGE_MODE: usize = 11;
	pub const TD_DROPFRAME: usize = 12;
	pub const TD_FEET_FRAMES: usize = 13;
	pub const TD_TIMEBASE: usize = 14;
	pub const TD_FRAMES_PER_FOOT: usize = 15;
	pub const TD_FRAMES_MODE: usize = 16;
	/// `AEGP_GetProjectBitDepth` result and value (`AEGP_ProjBitDepth_*`).
	pub const DEPTH_ERR: usize = 17;
	pub const DEPTH: usize = 18;
	/// What the project does not model must be refused, not faked:
	/// `AEGP_GetProjectName`, `GetProjectPath`, `GetProjectRootFolder`,
	/// `ProjectIsDirty`, `SetProjectBitDepth`, `NewProject`.
	pub const NAME_ERR: usize = 19;
	pub const PATH_ERR: usize = 20;
	pub const ROOT_ERR: usize = 21;
	pub const DIRTY_ERR: usize = 22;
	pub const SET_DEPTH_ERR: usize = 23;
	pub const NEW_ERR: usize = 24;
	/// `AEGP_GetFirstProjItem(project)` through the Item Suite: the project
	/// handle is the one the Item Suite takes back, and the item it returns
	/// is the comp (the project's one item).
	pub const ITEM_ERR: usize = 25;
	pub const ITEM_IS_COMP: usize = 26;
	/// `AEGP_GetProjectTimeDisplay` through a handle that is not a project.
	pub const BAD_HANDLE_ERR: usize = 27;
	pub const TOTAL: usize = 28;
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

/// Read the project and fill `slots`.
unsafe fn probe(in_data: &PF_InData, slots: &mut [u16]) {
	slots[slot::MAGIC_AT] = slot::MAGIC;

	let proj: *const AEGP_ProjSuite6 = match unsafe { acquire(in_data, kAEGPProjSuite, kAEGPProjSuiteVersion6) } {
		Ok(s) => s,
		Err(e) => {
			slots[slot::ACQUIRE_ERR] = clamp16(e as i64 & 0xffff);
			return;
		}
	};
	let proj = unsafe { &*proj };

	let mut n: A_long = -1;
	let err = unsafe { proj.AEGP_GetNumProjects.unwrap()(&mut n) };
	slots[slot::NUM_ERR] = clamp16(err as i64);
	slots[slot::NUM] = clamp16(n as i64);

	let mut handle: AEGP_ProjectH = std::ptr::null_mut();
	let err = unsafe { proj.AEGP_GetProjectByIndex.unwrap()(0, &mut handle) };
	slots[slot::BY_INDEX_ERR] = clamp16(err as i64);
	slots[slot::BY_INDEX_HANDLE] = (!handle.is_null()) as u16;

	// Out-of-range indices are refused and leave the handle null.
	let mut other: AEGP_ProjectH = 1usize as AEGP_ProjectH;
	let err = unsafe { proj.AEGP_GetProjectByIndex.unwrap()(1, &mut other) };
	slots[slot::BY_INDEX_PAST_END_ERR] = clamp16(err as i64);
	slots[slot::BY_INDEX_PAST_END_HANDLE_NULL] = other.is_null() as u16;
	let mut other: AEGP_ProjectH = std::ptr::null_mut();
	let err = unsafe { proj.AEGP_GetProjectByIndex.unwrap()(-1, &mut other) };
	slots[slot::BY_INDEX_NEGATIVE_ERR] = clamp16(err as i64);
	if handle.is_null() {
		return;
	}

	let mut td: AEGP_TimeDisplay3 = unsafe { std::mem::zeroed() };
	let err = unsafe { proj.AEGP_GetProjectTimeDisplay.unwrap()(handle, &mut td) };
	slots[slot::TD_ERR] = clamp16(err as i64);
	slots[slot::TD_MODE] = clamp16(td.display_mode as i64);
	slots[slot::TD_FOOTAGE_MODE] = clamp16(td.footage_display_mode as i64);
	slots[slot::TD_DROPFRAME] = clamp16(td.display_dropframeB as i64);
	slots[slot::TD_FEET_FRAMES] = clamp16(td.use_feet_framesB as i64);
	slots[slot::TD_TIMEBASE] = clamp16(td.timebaseC as i64);
	slots[slot::TD_FRAMES_PER_FOOT] = clamp16(td.frames_per_footC as i64);
	slots[slot::TD_FRAMES_MODE] = clamp16(td.frames_display_mode as i64);

	let mut depth: AEGP_ProjBitDepth = -1;
	let err = unsafe { proj.AEGP_GetProjectBitDepth.unwrap()(handle, &mut depth) };
	slots[slot::DEPTH_ERR] = clamp16(err as i64);
	slots[slot::DEPTH] = clamp16(depth as i64);

	let mut name = [0 as A_char; 64];
	let err = unsafe { proj.AEGP_GetProjectName.unwrap()(handle, name.as_mut_ptr()) };
	slots[slot::NAME_ERR] = clamp16(err as i64);
	let mut path: AEGP_MemHandle = std::ptr::null_mut();
	let err = unsafe { proj.AEGP_GetProjectPath.unwrap()(handle, &mut path) };
	slots[slot::PATH_ERR] = clamp16(err as i64);
	let mut root: AEGP_ItemH = std::ptr::null_mut();
	let err = unsafe { proj.AEGP_GetProjectRootFolder.unwrap()(handle, &mut root) };
	slots[slot::ROOT_ERR] = clamp16(err as i64);
	let mut dirty: A_Boolean = 0;
	let err = unsafe { proj.AEGP_ProjectIsDirty.unwrap()(handle, &mut dirty) };
	slots[slot::DIRTY_ERR] = clamp16(err as i64);
	let err = unsafe { proj.AEGP_SetProjectBitDepth.unwrap()(handle, AEGP_ProjBitDepth_32 as AEGP_ProjBitDepth) };
	slots[slot::SET_DEPTH_ERR] = clamp16(err as i64);
	let mut created: AEGP_ProjectH = std::ptr::null_mut();
	let err = unsafe { proj.AEGP_NewProject.unwrap()(&mut created) };
	slots[slot::NEW_ERR] = clamp16(err as i64);

	// The Item Suite takes the project handle back.
	if let Ok(items) = unsafe { acquire::<AEGP_ItemSuite8>(in_data, kAEGPItemSuite, kAEGPItemSuiteVersion8) } {
		let items = unsafe { &*items };
		let mut item: AEGP_ItemH = std::ptr::null_mut();
		let err = unsafe { items.AEGP_GetFirstProjItem.unwrap()(handle, &mut item) };
		slots[slot::ITEM_ERR] = clamp16(err as i64);
		let mut ty: AEGP_ItemType = -1;
		if err == 0 && !item.is_null() {
			unsafe { items.AEGP_GetItemType.unwrap()(item, &mut ty) };
		}
		slots[slot::ITEM_IS_COMP] = (ty == AEGP_ItemType_COMP as AEGP_ItemType) as u16;
	}

	let mut td: AEGP_TimeDisplay3 = unsafe { std::mem::zeroed() };
	let err = unsafe { proj.AEGP_GetProjectTimeDisplay.unwrap()(std::ptr::null_mut(), &mut td) };
	slots[slot::BAD_HANDLE_ERR] = clamp16(err as i64);
}

/// Write `byte` as channel value at full scale of the output's depth.
unsafe fn put(px: *mut u8, bytes_per_channel: usize, channel: usize, byte: u8) {
	let at = unsafe { px.add(channel * bytes_per_channel) };
	match bytes_per_channel {
		1 => unsafe { at.write(byte) },
		2 => unsafe { (at as *mut u16).write_unaligned((byte as f32 / 255.0 * 32768.0).round() as u16) },
		_ => unsafe { (at as *mut f32).write_unaligned(byte as f32 / 255.0) },
	}
}

unsafe fn render(in_data: *mut PF_InData, output: *mut PF_LayerDef) -> PF_Err {
	let (in_data, output) = unsafe { (&*in_data, &*output) };
	let mut slots = vec![0u16; slot::TOTAL];
	unsafe { probe(in_data, &mut slots) };

	// One slot per pixel, row-major, ARGB: A, R = high byte, G = low byte.
	let bpc = (output.rowbytes / output.width.max(1) / 4).max(1) as usize;
	for (i, value) in slots.iter().enumerate() {
		let (x, y) = (i as i32 % output.width, i as i32 / output.width);
		if y >= output.height {
			break;
		}
		let px = unsafe { (output.data as *mut u8).add(y as usize * output.rowbytes as usize + x as usize * 4 * bpc) };
		unsafe {
			put(px, bpc, 0, 255);
			put(px, bpc, 1, (value >> 8) as u8);
			put(px, bpc, 2, (value & 0xff) as u8);
			put(px, bpc, 3, 0);
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
			out.out_flags = (PF_OutFlag_PIX_INDEPENDENT | PF_OutFlag_DEEP_COLOR_AWARE) as PF_OutFlags;
			out.out_flags2 = PF_OutFlag2_FLOAT_COLOR_AWARE as PF_OutFlags2;
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
