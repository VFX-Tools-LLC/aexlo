//! An effect that follows the chain a plugin uses to read a data file (a LUT,
//! a CLF) from a Layer parameter, and writes what it found into its output.
//!
//! The chain, as After Effects serves it:
//!
//! 1. `AEGP PF Interface Suite` `AEGP_GetNewEffectForEffect`
//! 2. `AEGP Stream Suite` `AEGP_GetNewEffectStreamByIndex`
//! 3. `AEGP_GetNewStreamValue`: the Layer parameter's layer id
//! 4. `AEGP Layer Suite` `AEGP_GetLayerFromLayerID` (in the effect layer's comp)
//! 5. `AEGP_GetLayerSourceItem`
//! 6. `AEGP Item Suite` `AEGP_GetItemType`, which must be `AEGP_ItemType_FOOTAGE`
//! 7. `AEGP Footage Suite` `AEGP_GetMainFootageFromItem`
//! 8. `AEGP_GetFootagePath`: a UTF-16 memory handle
//!
//! Parameter 1 is the only one: a Layer parameter named `Footage`.
//!
//! The report is the first row of the 8 bit output, one pixel per three
//! bytes (`R`, `G`, `B`; alpha is 255 everywhere):
//!
//! - pixel 0: `R` = stages completed (0..=11, 11 is the whole chain: suites, effect, stream, value, effect layer, comp, layer, source item, item type, footage, path), `G` = the
//!   low byte of the error code of the call that failed (0 if none; 100 + n if
//!   the n-th suite, counting from 1, could not be acquired), `B` = the low
//!   byte of the item type, once known.
//! - pixel 1: `R`, `G` = the path's length in bytes (little endian), `B` = 1
//!   if the stream value held a non-zero layer id.
//! - pixel 2 onwards: the path as UTF-8, three bytes a pixel.
//!
//! The output must be at least `2 + ceil(len / 3)` pixels wide.

#![allow(non_snake_case)]
#![allow(clippy::missing_safety_doc)]

use std::ffi::c_void;

use after_effects_sys::*;

fn name32(name: &str) -> [A_char; 32] {
	let mut out = [0 as A_char; 32];
	for (dst, src) in out.iter_mut().zip(name.bytes().take(31)) {
		*dst = src as A_char;
	}
	out
}

unsafe fn params_setup(in_data: *mut PF_InData, out_data: *mut PF_OutData) -> PF_Err {
	let in_data = unsafe { &*in_data };
	let mut def: PF_ParamDef = unsafe { std::mem::zeroed() };
	def.param_type = PF_Param_LAYER as PF_ParamType;
	def.name_do_not_use_directly = name32("Footage");
	def.uu.id = 1;
	def.u.ld.dephault = PF_LayerDefault_NONE as A_long;
	let err = match in_data.inter.add_param {
		Some(add) => unsafe { add(in_data.effect_ref, -1, &mut def) },
		None => PF_Err_BAD_CALLBACK_PARAM as PF_Err,
	};
	if err != PF_Err_NONE as PF_Err {
		return err;
	}
	unsafe { (*out_data).num_params = 2 };
	PF_Err_NONE as PF_Err
}

/// What the chain found.
#[derive(Default)]
struct Report {
	stages: u8,
	err: u8,
	item_type: u8,
	has_layer_id: bool,
	path: Vec<u8>,
}

/// Run the chain on parameter `index`; stop at the first failure.
unsafe fn follow(in_data: &PF_InData, index: i32, report: &mut Report) {
	macro_rules! step {
		($call:expr) => {{
			let err = $call;
			if err != PF_Err_NONE as PF_Err {
				report.err = err as u8;
				return;
			}
			report.stages += 1;
		}};
	}

	let basic = unsafe { &*in_data.pica_basicP };
	let acquire = basic.AcquireSuite.unwrap();
	let mut acquired = |n: u8, name: &[u8], version: u32| -> Option<*const c_void> {
		let mut out: *const c_void = std::ptr::null();
		let err = unsafe { acquire(name.as_ptr() as *const _, version as i32, &mut out) };
		if err != 0 || out.is_null() {
			report.err = 100 + n;
			return None;
		}
		Some(out)
	};
	let Some(iface) = acquired(1, b"AEGP PF Interface Suite\0", kAEGPPFInterfaceSuiteVersion1) else {
		return;
	};
	let Some(streams) = acquired(2, b"AEGP Stream Suite\0", kAEGPStreamSuiteVersion6) else {
		return;
	};
	let Some(layers) = acquired(3, b"AEGP Layer Suite\0", kAEGPLayerSuiteVersion9) else {
		return;
	};
	let Some(items) = acquired(4, b"AEGP Item Suite\0", kAEGPItemSuiteVersion9) else {
		return;
	};
	let Some(footage) = acquired(5, b"AEGP Footage Suite\0", kAEGPFootageSuiteVersion5) else {
		return;
	};
	let Some(memory) = acquired(6, b"AEGP Memory Suite\0", kAEGPMemorySuiteVersion1) else {
		return;
	};
	report.stages = 1;
	let iface = unsafe { &*(iface as *const AEGP_PFInterfaceSuite1) };
	let streams = unsafe { &*(streams as *const AEGP_StreamSuite6) };
	let layers = unsafe { &*(layers as *const AEGP_LayerSuite9) };
	let items = unsafe { &*(items as *const AEGP_ItemSuite9) };
	let footage = unsafe { &*(footage as *const AEGP_FootageSuite5) };
	let memory = unsafe { &*(memory as *const AEGP_MemorySuite1) };

	unsafe {
		let mut effect = std::ptr::null_mut();
		step!(iface.AEGP_GetNewEffectForEffect.unwrap()(
			0,
			in_data.effect_ref,
			&mut effect
		));
		let mut stream = std::ptr::null_mut();
		step!(streams.AEGP_GetNewEffectStreamByIndex.unwrap()(
			0,
			effect,
			index,
			&mut stream
		));
		let mut value: AEGP_StreamValue2 = std::mem::zeroed();
		let time = A_Time {
			value: in_data.current_time,
			scale: in_data.time_scale.max(1),
		};
		step!(streams.AEGP_GetNewStreamValue.unwrap()(
			0,
			stream,
			AEGP_LTimeMode_LayerTime as _,
			&time,
			0,
			&mut value
		));
		let layer_id = value.val.layer_id;
		report.has_layer_id = layer_id != 0;
		streams.AEGP_DisposeStreamValue.unwrap()(&mut value);
		streams.AEGP_DisposeStream.unwrap()(stream);
		if layer_id == 0 {
			// No layer behind the parameter ("None"): the stage count says
			// how far the chain went.
			return;
		}

		let mut effect_layer = std::ptr::null_mut();
		let mut comp = std::ptr::null_mut();
		let mut layer = std::ptr::null_mut();
		step!(iface.AEGP_GetEffectLayer.unwrap()(
			in_data.effect_ref,
			&mut effect_layer
		));
		step!(layers.AEGP_GetLayerParentComp.unwrap()(effect_layer, &mut comp));
		step!(layers.AEGP_GetLayerFromLayerID.unwrap()(comp, layer_id, &mut layer));
		let mut item = std::ptr::null_mut();
		step!(layers.AEGP_GetLayerSourceItem.unwrap()(layer, &mut item));
		let mut kind: AEGP_ItemType = 0;
		step!(items.AEGP_GetItemType.unwrap()(item, &mut kind));
		report.item_type = kind as u8;
		if kind != AEGP_ItemType_FOOTAGE as AEGP_ItemType {
			return;
		}
		let mut foot = std::ptr::null_mut();
		step!(footage.AEGP_GetMainFootageFromItem.unwrap()(item, &mut foot));
		let mut path_h = std::ptr::null_mut();
		step!(footage.AEGP_GetFootagePath.unwrap()(foot, 0, 0, &mut path_h));
		let mut locked: *mut c_void = std::ptr::null_mut();
		if memory.AEGP_LockMemHandle.unwrap()(path_h, &mut locked) == PF_Err_NONE as PF_Err && !locked.is_null() {
			let units = locked as *const u16;
			let len = (0..).take_while(|&i| *units.add(i) != 0).count();
			let text = String::from_utf16_lossy(std::slice::from_raw_parts(units, len));
			report.path = text.into_bytes();
			memory.AEGP_UnlockMemHandle.unwrap()(path_h);
		}
		memory.AEGP_FreeMemHandle.unwrap()(path_h);
	}
}

unsafe fn render(in_data: *mut PF_InData, output: *mut PF_LayerDef) -> PF_Err {
	let in_data = unsafe { &*in_data };
	let output = unsafe { &*output };
	let mut report = Report::default();
	unsafe { follow(in_data, 1, &mut report) };

	// Opaque black, then the report over the first row.
	for y in 0..output.height {
		let row = unsafe { (output.data as *mut u8).add(y as usize * output.rowbytes as usize) };
		for x in 0..output.width as usize {
			unsafe {
				row.add(x * 4).write(255);
				std::ptr::write_bytes(row.add(x * 4 + 1), 0, 3);
			}
		}
	}
	let mut bytes = vec![
		report.stages,
		report.err,
		report.item_type,
		report.path.len() as u8,
		(report.path.len() >> 8) as u8,
		report.has_layer_id as u8,
	];
	bytes.extend_from_slice(&report.path);
	let row = output.data as *mut u8;
	for (i, byte) in bytes.iter().enumerate() {
		let pixel = i / 3;
		if pixel >= output.width as usize {
			break;
		}
		// PF_Pixel8 is ARGB in memory.
		unsafe { row.add(pixel * 4 + 1 + i % 3).write(*byte) };
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
		PF_Cmd_PARAMS_SETUP => unsafe { params_setup(in_data, out_data) },
		PF_Cmd_RENDER => unsafe { render(in_data, output) },
		_ => PF_Err_NONE as PF_Err,
	}
}
