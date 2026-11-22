//! `AEGP Mask Outline Suite` (wire version 5, `AEGP_MaskOutlineSuite3`): the
//! Bezier outline of a mask, read from the paths the embedder attached with
//! [`PluginInstance::set_mask_paths`](crate::PluginInstance::set_mask_paths).
//!
//! A plugin reaches an outline through the Stream Suite: `AEGP_GetNewMaskStream`
//! (outline) then `AEGP_GetNewStreamValue`, whose `val.mask` is an
//! `AEGP_MaskOutlineValH`. aexlo hands out a private copy of the
//! [`MaskPath`] as that handle (masks do not change over time here), and the
//! plugin gives it back with `AEGP_DisposeStreamValue`. Handles are tracked, so
//! a stale or foreign one is refused rather than dereferenced.
//!
//! Served: open/closed, segment count, vertex info (tangents relative to the
//! vertex, as `AEGP_MaskVertex` defines), and zero feather points (the mask
//! model has no variable-width feather). Every edit reports through
//! [`unsupported`].

use std::collections::HashSet;
use std::sync::Mutex;

use crate::MaskPath;
use crate::core::diagnostics::diag;
use crate::observe::{UnsupportedKind, unsupported};
use crate::sys::*;

/// Addresses of the outline copies the plugin holds.
static LIVE: Mutex<Option<HashSet<usize>>> = Mutex::new(None);

fn live() -> std::sync::MutexGuard<'static, Option<HashSet<usize>>> {
	LIVE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Hand the plugin a copy of `path` as an `AEGP_MaskOutlineValH`.
pub(crate) fn new_value(path: MaskPath) -> AEGP_MaskOutlineValH {
	let raw = Box::into_raw(Box::new(path));
	live().get_or_insert_with(HashSet::new).insert(raw as usize);
	raw as AEGP_MaskOutlineValH
}

/// Free an outline from [`new_value`]. `false` when `handle` is not one that
/// is live (never allocated here, or already disposed), in which case nothing
/// is touched.
pub(crate) fn dispose_value(handle: AEGP_MaskOutlineValH) -> bool {
	if !live().as_mut().is_some_and(|set| set.remove(&(handle as usize))) {
		return false;
	}
	// SAFETY: it was in `LIVE`, so it came from `new_value` and is freed only here.
	drop(unsafe { Box::from_raw(handle as *mut MaskPath) });
	true
}

/// Run `f` on the outline behind `handle`, if it is live.
fn with_outline<T>(handle: AEGP_MaskOutlineValH, f: impl FnOnce(&MaskPath) -> T) -> Option<T> {
	if !live().as_ref().is_some_and(|set| set.contains(&(handle as usize))) {
		return None;
	}
	// SAFETY: the address is in `LIVE`, so it is a `MaskPath` not yet disposed.
	unsafe { (handle as *const MaskPath).as_ref() }.map(f)
}

fn declined(name: &str) -> A_Err {
	unsupported(UnsupportedKind::Declined, format!("AEGP_MaskOutlineSuite/{name}"), "");
	A_Err_GENERIC
}

unsafe extern "C" fn is_mask_outline_open(mask_outlineH: AEGP_MaskOutlineValH, openPB: *mut A_Boolean) -> A_Err {
	crate::core::ffi::guard("is_mask_outline_open", || {
		diag!("AEGP_MaskOutlineSuite/AEGP_IsMaskOutlineOpen", "mask_outlineH" => format!("{:#x}", mask_outlineH as usize));
		let Some(out) = (unsafe { openPB.as_mut() }) else {
			return PF_Err_BAD_CALLBACK_PARAM;
		};
		match with_outline(mask_outlineH, |p| !p.closed as A_Boolean) {
			Some(open) => {
				*out = open;
				PF_Err_NONE
			}
			None => PF_Err_BAD_CALLBACK_PARAM,
		}
	})
}

unsafe extern "C" fn get_mask_outline_num_segments(
	mask_outlineH: AEGP_MaskOutlineValH,
	num_segmentsPL: *mut A_long,
) -> A_Err {
	crate::core::ffi::guard("get_mask_outline_num_segments", || {
		diag!("AEGP_MaskOutlineSuite/AEGP_GetMaskOutlineNumSegments", "mask_outlineH" => format!("{:#x}", mask_outlineH as usize));
		let Some(out) = (unsafe { num_segmentsPL.as_mut() }) else {
			return PF_Err_BAD_CALLBACK_PARAM;
		};
		match with_outline(mask_outlineH, |p| p.segment_count() as A_long) {
			Some(n) => {
				*out = n;
				PF_Err_NONE
			}
			None => PF_Err_BAD_CALLBACK_PARAM,
		}
	})
}

unsafe extern "C" fn get_mask_outline_vertex_info(
	mask_outlineH: AEGP_MaskOutlineValH,
	which_pointL: AEGP_VertexIndex,
	vertexP: *mut AEGP_MaskVertex,
) -> A_Err {
	crate::core::ffi::guard("get_mask_outline_vertex_info", || {
		let Some(out) = (unsafe { vertexP.as_mut() }) else {
			return PF_Err_BAD_CALLBACK_PARAM;
		};
		let vertex = with_outline(mask_outlineH, |p| {
			usize::try_from(which_pointL).ok().and_then(|i| p.vertex(i)).copied()
		});
		let result = match vertex {
			None => PF_Err_BAD_CALLBACK_PARAM,
			Some(None) => PF_Err_INVALID_INDEX,
			Some(Some(v)) => {
				*out = AEGP_MaskVertex {
					x: v.x,
					y: v.y,
					tan_in_x: v.tan_in_x,
					tan_in_y: v.tan_in_y,
					tan_out_x: v.tan_out_x,
					tan_out_y: v.tan_out_y,
				};
				PF_Err_NONE
			}
		};
		diag!("AEGP_MaskOutlineSuite/AEGP_GetMaskOutlineVertexInfo",
			"mask_outlineH" => format!("{:#x}", mask_outlineH as usize),
			"which_pointL" => which_pointL,
			"vertexP (out)" => if result == PF_Err_NONE {
				format!(
					"x={} y={} tan_in=({}, {}) tan_out=({}, {})",
					out.x, out.y, out.tan_in_x, out.tan_in_y, out.tan_out_x, out.tan_out_y
				)
			} else {
				"-".to_string()
			};
			result: result,
		);
		result
	})
}

/// The mask model has no variable-width feather points.
unsafe extern "C" fn get_mask_outline_num_feathers(
	mask_outlineH: AEGP_MaskOutlineValH,
	num_feathersPL: *mut A_long,
) -> A_Err {
	crate::core::ffi::guard("get_mask_outline_num_feathers", || {
		diag!("AEGP_MaskOutlineSuite/AEGP_GetMaskOutlineNumFeathers", "mask_outlineH" => format!("{:#x}", mask_outlineH as usize));
		let Some(out) = (unsafe { num_feathersPL.as_mut() }) else {
			return PF_Err_BAD_CALLBACK_PARAM;
		};
		match with_outline(mask_outlineH, |_| 0) {
			Some(n) => {
				*out = n;
				PF_Err_NONE
			}
			None => PF_Err_BAD_CALLBACK_PARAM,
		}
	})
}

unsafe extern "C" fn get_mask_outline_feather_info(
	mask_outlineH: AEGP_MaskOutlineValH,
	_which_featherL: AEGP_FeatherIndex,
	_featherP: *mut AEGP_MaskFeather,
) -> A_Err {
	crate::core::ffi::guard("get_mask_outline_feather_info", || {
		diag!("AEGP_MaskOutlineSuite/AEGP_GetMaskOutlineFeatherInfo", "mask_outlineH" => format!("{:#x}", mask_outlineH as usize));
		// There are no feather points, so every index is out of range.
		match with_outline(mask_outlineH, |_| ()) {
			Some(()) => PF_Err_INVALID_INDEX,
			None => PF_Err_BAD_CALLBACK_PARAM,
		}
	})
}

unsafe extern "C" fn set_mask_outline_open(_mask_outlineH: AEGP_MaskOutlineValH, _openB: A_Boolean) -> A_Err {
	crate::core::ffi::guard("set_mask_outline_open", || declined("AEGP_SetMaskOutlineOpen"))
}

unsafe extern "C" fn set_mask_outline_vertex_info(
	_mask_outlineH: AEGP_MaskOutlineValH,
	_which_pointL: AEGP_VertexIndex,
	_vertexP: *const AEGP_MaskVertex,
) -> A_Err {
	crate::core::ffi::guard("set_mask_outline_vertex_info", || {
		declined("AEGP_SetMaskOutlineVertexInfo")
	})
}

unsafe extern "C" fn create_vertex(_mask_outlineH: AEGP_MaskOutlineValH, _insert_position: AEGP_VertexIndex) -> A_Err {
	crate::core::ffi::guard("create_vertex", || declined("AEGP_CreateVertex"))
}

unsafe extern "C" fn delete_vertex(_mask_outlineH: AEGP_MaskOutlineValH, _index: AEGP_VertexIndex) -> A_Err {
	crate::core::ffi::guard("delete_vertex", || declined("AEGP_DeleteVertex"))
}

unsafe extern "C" fn set_mask_outline_feather_info(
	_mask_outlineH: AEGP_MaskOutlineValH,
	_which_featherL: AEGP_VertexIndex,
	_featherP: *const AEGP_MaskFeather,
) -> A_Err {
	crate::core::ffi::guard("set_mask_outline_feather_info", || {
		declined("AEGP_SetMaskOutlineFeatherInfo")
	})
}

unsafe extern "C" fn create_mask_outline_feather(
	_mask_outlineH: AEGP_MaskOutlineValH,
	_featherP0: *const AEGP_MaskFeather,
	_insert_positionP: *mut AEGP_FeatherIndex,
) -> A_Err {
	crate::core::ffi::guard("create_mask_outline_feather", || {
		declined("AEGP_CreateMaskOutlineFeather")
	})
}

unsafe extern "C" fn delete_mask_outline_feather(
	_mask_outlineH: AEGP_MaskOutlineValH,
	_index: AEGP_FeatherIndex,
) -> A_Err {
	crate::core::ffi::guard("delete_mask_outline_feather", || {
		declined("AEGP_DeleteMaskOutlineFeather")
	})
}

pub(super) const fn suite_3() -> AEGP_MaskOutlineSuite3 {
	AEGP_MaskOutlineSuite3 {
		AEGP_IsMaskOutlineOpen: Some(is_mask_outline_open),
		AEGP_SetMaskOutlineOpen: Some(set_mask_outline_open),
		AEGP_GetMaskOutlineNumSegments: Some(get_mask_outline_num_segments),
		AEGP_GetMaskOutlineVertexInfo: Some(get_mask_outline_vertex_info),
		AEGP_SetMaskOutlineVertexInfo: Some(set_mask_outline_vertex_info),
		AEGP_CreateVertex: Some(create_vertex),
		AEGP_DeleteVertex: Some(delete_vertex),
		AEGP_GetMaskOutlineNumFeathers: Some(get_mask_outline_num_feathers),
		AEGP_GetMaskOutlineFeatherInfo: Some(get_mask_outline_feather_info),
		AEGP_SetMaskOutlineFeatherInfo: Some(set_mask_outline_feather_info),
		AEGP_CreateMaskOutlineFeather: Some(create_mask_outline_feather),
		AEGP_DeleteMaskOutlineFeather: Some(delete_mask_outline_feather),
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::MaskVertex;

	fn triangle() -> MaskPath {
		MaskPath {
			vertices: vec![
				MaskVertex::corner(1.0, 2.0),
				MaskVertex {
					x: 10.0,
					y: 2.0,
					tan_in_x: -3.0,
					tan_out_x: 3.0,
					..Default::default()
				},
				MaskVertex::corner(5.0, 9.0),
			],
			closed: true,
			..Default::default()
		}
	}

	#[test]
	fn a_closed_outline_wraps_its_last_vertex_and_is_disposed_once() {
		let s = suite_3();
		let h = new_value(triangle());
		let (mut open, mut n) = (9, -1);
		unsafe {
			assert_eq!(s.AEGP_IsMaskOutlineOpen.unwrap()(h, &mut open), PF_Err_NONE);
			assert_eq!(s.AEGP_GetMaskOutlineNumSegments.unwrap()(h, &mut n), PF_Err_NONE);
		}
		assert_eq!((open, n), (0, 3));

		let mut v: AEGP_MaskVertex = unsafe { std::mem::zeroed() };
		unsafe { s.AEGP_GetMaskOutlineVertexInfo.unwrap()(h, 1, &mut v) };
		assert_eq!((v.x, v.y, v.tan_in_x, v.tan_out_x), (10.0, 2.0, -3.0, 3.0));
		// Index == segment count wraps to vertex 0; beyond it is out of range.
		unsafe { s.AEGP_GetMaskOutlineVertexInfo.unwrap()(h, 3, &mut v) };
		assert_eq!((v.x, v.y), (1.0, 2.0));
		assert_eq!(
			unsafe { s.AEGP_GetMaskOutlineVertexInfo.unwrap()(h, 4, &mut v) },
			PF_Err_INVALID_INDEX
		);
		assert_eq!(
			unsafe { s.AEGP_GetMaskOutlineVertexInfo.unwrap()(h, -1, &mut v) },
			PF_Err_INVALID_INDEX
		);

		assert!(dispose_value(h));
		assert!(!dispose_value(h));
		assert_eq!(
			unsafe { s.AEGP_GetMaskOutlineNumSegments.unwrap()(h, &mut n) },
			PF_Err_BAD_CALLBACK_PARAM
		);
	}

	#[test]
	fn foreign_handles_are_refused() {
		let s = suite_3();
		let fake = 0x1000 as AEGP_MaskOutlineValH;
		let mut n = 0;
		assert_eq!(
			unsafe { s.AEGP_GetMaskOutlineNumSegments.unwrap()(fake, &mut n) },
			PF_Err_BAD_CALLBACK_PARAM
		);
		assert!(!dispose_value(fake));
	}

	#[test]
	fn edits_are_declined() {
		let s = suite_3();
		assert_eq!(
			unsafe { s.AEGP_CreateVertex.unwrap()(std::ptr::null_mut(), 0) },
			A_Err_GENERIC
		);
	}
}
