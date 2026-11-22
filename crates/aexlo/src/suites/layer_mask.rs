//! `AEGP Layer Mask Suite` (wire version 7, `AEGP_MaskSuite6`): the masks on
//! the effect's layer, read from the paths the embedder attached with
//! [`PluginInstance::set_mask_paths`](crate::PluginInstance::set_mask_paths).
//!
//! A layer with no masks answers `AEGP_GetLayerNumMasks` with 0. A mask handle
//! is a small boxed record (instance and index) that the plugin owns until
//! `AEGP_DisposeMask`. Only what the mask model holds is answered: invert, mode,
//! id, and "not locked, not roto bezier". Everything else (motion blur, feather
//! falloff, color) and every edit reports through [`unsupported`].
//!
//! The mask's outline is not served here. A plugin reads it through
//! `AEGP_GetNewMaskStream`, which the Stream Suite still refuses.

use std::collections::HashSet;
use std::sync::Mutex;

use crate::PluginInstance;
use crate::core::diagnostics::diag;
use crate::observe::{UnsupportedKind, unsupported};
use crate::sys::*;

/// What an `AEGP_MaskRefH` points at.
struct MaskRef {
	/// The effect (`PF_ProgPtr`) whose layer carries the mask.
	effect_ref: PF_ProgPtr,
	index: usize,
}

/// Addresses of the mask handles the plugin holds, so a stale or foreign
/// handle is refused rather than dereferenced.
static LIVE: Mutex<Option<HashSet<usize>>> = Mutex::new(None);

fn live() -> std::sync::MutexGuard<'static, Option<HashSet<usize>>> {
	LIVE.lock().unwrap_or_else(|e| e.into_inner())
}

/// The mask path behind a handle.
fn with_mask<T>(maskH: AEGP_MaskRefH, f: impl FnOnce(&crate::MaskPath) -> T) -> Option<T> {
	if !live().as_ref().is_some_and(|set| set.contains(&(maskH as usize))) {
		return None;
	}
	// SAFETY: the address is in `LIVE`, so it is a `MaskRef` not yet disposed.
	let mask = unsafe { (maskH as *const MaskRef).as_ref() }?;
	let instance = PluginInstance::get_instance_ptr(mask.effect_ref)?;
	// SAFETY: the handle's effect is the live instance running the plugin call.
	let instance = unsafe { instance.as_ref() };
	instance.mask_paths().get(mask.index).map(f)
}

fn declined(name: &str) -> A_Err {
	unsupported(UnsupportedKind::Declined, format!("AEGP_MaskSuite/{name}"), "");
	A_Err_GENERIC
}

unsafe extern "C" fn get_layer_num_masks(layerH: AEGP_LayerH, num_masksPL: *mut A_long) -> A_Err {
	crate::core::ffi::guard("get_layer_num_masks", || {
		diag!("AEGP_MaskSuite/AEGP_GetLayerNumMasks", "layerH" => format!("{:#x}", layerH as usize));
		let (Some(instance), Some(out)) = (PluginInstance::get_instance_ptr(layerH as PF_ProgPtr), unsafe {
			num_masksPL.as_mut()
		}) else {
			return PF_Err_BAD_CALLBACK_PARAM;
		};
		// SAFETY: layer handles are the live instance running the plugin call.
		*out = unsafe { instance.as_ref() }.mask_paths().len() as A_long;
		PF_Err_NONE
	})
}

unsafe extern "C" fn get_layer_mask_by_index(
	layerH: AEGP_LayerH,
	mask_indexL: AEGP_MaskIndex,
	maskPH: *mut AEGP_MaskRefH,
) -> A_Err {
	crate::core::ffi::guard("get_layer_mask_by_index", || {
		diag!("AEGP_MaskSuite/AEGP_GetLayerMaskByIndex",
			"layerH" => format!("{:#x}", layerH as usize),
			"mask_indexL" => mask_indexL,
		);
		let Some(out) = (unsafe { maskPH.as_mut() }) else {
			return PF_Err_BAD_CALLBACK_PARAM;
		};
		*out = std::ptr::null_mut();
		let Some(instance) = PluginInstance::get_instance_ptr(layerH as PF_ProgPtr) else {
			return PF_Err_BAD_CALLBACK_PARAM;
		};
		// SAFETY: layer handles are the live instance running the plugin call.
		let count = unsafe { instance.as_ref() }.mask_paths().len();
		let Some(index) = usize::try_from(mask_indexL).ok().filter(|&i| i < count) else {
			return PF_Err_INVALID_INDEX;
		};
		let raw = Box::into_raw(Box::new(MaskRef {
			effect_ref: layerH as PF_ProgPtr,
			index,
		}));
		live().get_or_insert_with(HashSet::new).insert(raw as usize);
		*out = raw as AEGP_MaskRefH;
		PF_Err_NONE
	})
}

unsafe extern "C" fn dispose_mask(mask_refH: AEGP_MaskRefH) -> A_Err {
	crate::core::ffi::guard("dispose_mask", || {
		diag!("AEGP_MaskSuite/AEGP_DisposeMask", "mask_refH" => format!("{:#x}", mask_refH as usize));
		let known = live().as_mut().is_some_and(|set| set.remove(&(mask_refH as usize)));
		if !known {
			return PF_Err_BAD_CALLBACK_PARAM;
		}
		// SAFETY: it was in `LIVE`, so it came from `get_layer_mask_by_index`
		// and is freed only here.
		drop(unsafe { Box::from_raw(mask_refH as *mut MaskRef) });
		PF_Err_NONE
	})
}

/// Write `value(path)` for the mask behind `mask_refH` to `out`.
fn answer<T>(mask_refH: AEGP_MaskRefH, out: *mut T, value: impl FnOnce(&crate::MaskPath) -> T) -> A_Err {
	let Some(out) = (unsafe { out.as_mut() }) else {
		return PF_Err_BAD_CALLBACK_PARAM;
	};
	match with_mask(mask_refH, value) {
		Some(v) => {
			*out = v;
			PF_Err_NONE
		}
		None => PF_Err_BAD_CALLBACK_PARAM,
	}
}

unsafe extern "C" fn get_mask_invert(mask_refH: AEGP_MaskRefH, invertPB: *mut A_Boolean) -> A_Err {
	crate::core::ffi::guard("get_mask_invert", || {
		diag!("AEGP_MaskSuite/AEGP_GetMaskInvert", "mask_refH" => format!("{:#x}", mask_refH as usize));
		answer(mask_refH, invertPB, |p| p.inverted as A_Boolean)
	})
}

unsafe extern "C" fn get_mask_mode(mask_refH: AEGP_MaskRefH, modeP: *mut PF_MaskMode) -> A_Err {
	crate::core::ffi::guard("get_mask_mode", || {
		diag!("AEGP_MaskSuite/AEGP_GetMaskMode", "mask_refH" => format!("{:#x}", mask_refH as usize));
		answer(mask_refH, modeP, |p| p.mode.to_sys())
	})
}

unsafe extern "C" fn get_mask_id(mask_refH: AEGP_MaskRefH, id_valP: *mut AEGP_MaskIDVal) -> A_Err {
	crate::core::ffi::guard("get_mask_id", || {
		diag!("AEGP_MaskSuite/AEGP_GetMaskID", "mask_refH" => format!("{:#x}", mask_refH as usize));
		answer(mask_refH, id_valP, |p| p.id as AEGP_MaskIDVal)
	})
}

/// aexlo's masks are plain Bezier paths that are never locked.
unsafe extern "C" fn get_mask_lock_state(mask_refH: AEGP_MaskRefH, is_lockedPB: *mut A_Boolean) -> A_Err {
	crate::core::ffi::guard("get_mask_lock_state", || {
		diag!("AEGP_MaskSuite/AEGP_GetMaskLockState", "mask_refH" => format!("{:#x}", mask_refH as usize));
		answer(mask_refH, is_lockedPB, |_| 0)
	})
}

unsafe extern "C" fn get_mask_is_roto_bezier(mask_refH: AEGP_MaskRefH, is_roto_bezierPB: *mut A_Boolean) -> A_Err {
	crate::core::ffi::guard("get_mask_is_roto_bezier", || {
		diag!("AEGP_MaskSuite/AEGP_GetMaskIsRotoBezier", "mask_refH" => format!("{:#x}", mask_refH as usize));
		answer(mask_refH, is_roto_bezierPB, |_| 0)
	})
}

unsafe extern "C" fn set_mask_invert(_mask_refH: AEGP_MaskRefH, _invertB: A_Boolean) -> A_Err {
	crate::core::ffi::guard("set_mask_invert", || declined("AEGP_SetMaskInvert"))
}

unsafe extern "C" fn set_mask_mode(_maskH: AEGP_MaskRefH, _mode: PF_MaskMode) -> A_Err {
	crate::core::ffi::guard("set_mask_mode", || declined("AEGP_SetMaskMode"))
}

unsafe extern "C" fn get_mask_motion_blur_state(_mask_refH: AEGP_MaskRefH, _blur_stateP: *mut AEGP_MaskMBlur) -> A_Err {
	crate::core::ffi::guard("get_mask_motion_blur_state", || declined("AEGP_GetMaskMotionBlurState"))
}

unsafe extern "C" fn set_mask_motion_blur_state(_mask_refH: AEGP_MaskRefH, _blur_state: AEGP_MaskMBlur) -> A_Err {
	crate::core::ffi::guard("set_mask_motion_blur_state", || declined("AEGP_SetMaskMotionBlurState"))
}

unsafe extern "C" fn get_mask_feather_falloff(
	_mask_refH: AEGP_MaskRefH,
	_feather_falloffP: *mut AEGP_MaskFeatherFalloff,
) -> A_Err {
	crate::core::ffi::guard("get_mask_feather_falloff", || declined("AEGP_GetMaskFeatherFalloff"))
}

unsafe extern "C" fn set_mask_feather_falloff(
	_mask_refH: AEGP_MaskRefH,
	_feather_falloffP: AEGP_MaskFeatherFalloff,
) -> A_Err {
	crate::core::ffi::guard("set_mask_feather_falloff", || declined("AEGP_SetMaskFeatherFalloff"))
}

unsafe extern "C" fn create_new_mask(
	_layerH: AEGP_LayerH,
	_mask_refPH: *mut AEGP_MaskRefH,
	_mask_indexPL0: *mut A_long,
) -> A_Err {
	crate::core::ffi::guard("create_new_mask", || declined("AEGP_CreateNewMask"))
}

unsafe extern "C" fn delete_mask_from_layer(_mask_refH: AEGP_MaskRefH) -> A_Err {
	crate::core::ffi::guard("delete_mask_from_layer", || declined("AEGP_DeleteMaskFromLayer"))
}

unsafe extern "C" fn get_mask_color(_mask_refH: AEGP_MaskRefH, _colorP: *mut AEGP_ColorVal) -> A_Err {
	crate::core::ffi::guard("get_mask_color", || declined("AEGP_GetMaskColor"))
}

unsafe extern "C" fn set_mask_color(_mask_refH: AEGP_MaskRefH, _colorP: *const AEGP_ColorVal) -> A_Err {
	crate::core::ffi::guard("set_mask_color", || declined("AEGP_SetMaskColor"))
}

unsafe extern "C" fn set_mask_lock_state(_mask_refH: AEGP_MaskRefH, _lockB: A_Boolean) -> A_Err {
	crate::core::ffi::guard("set_mask_lock_state", || declined("AEGP_SetMaskLockState"))
}

unsafe extern "C" fn set_mask_is_roto_bezier(_mask_refH: AEGP_MaskRefH, _is_roto_bezierB: A_Boolean) -> A_Err {
	crate::core::ffi::guard("set_mask_is_roto_bezier", || declined("AEGP_SetMaskIsRotoBezier"))
}

unsafe extern "C" fn duplicate_mask(
	_orig_mask_refH: AEGP_MaskRefH,
	_duplicate_mask_refPH: *mut AEGP_MaskRefH,
) -> A_Err {
	crate::core::ffi::guard("duplicate_mask", || declined("AEGP_DuplicateMask"))
}

pub(super) const fn suite_6() -> AEGP_MaskSuite6 {
	AEGP_MaskSuite6 {
		AEGP_GetLayerNumMasks: Some(get_layer_num_masks),
		AEGP_GetLayerMaskByIndex: Some(get_layer_mask_by_index),
		AEGP_DisposeMask: Some(dispose_mask),
		AEGP_GetMaskInvert: Some(get_mask_invert),
		AEGP_SetMaskInvert: Some(set_mask_invert),
		AEGP_GetMaskMode: Some(get_mask_mode),
		AEGP_SetMaskMode: Some(set_mask_mode),
		AEGP_GetMaskMotionBlurState: Some(get_mask_motion_blur_state),
		AEGP_SetMaskMotionBlurState: Some(set_mask_motion_blur_state),
		AEGP_GetMaskFeatherFalloff: Some(get_mask_feather_falloff),
		AEGP_SetMaskFeatherFalloff: Some(set_mask_feather_falloff),
		AEGP_GetMaskID: Some(get_mask_id),
		AEGP_CreateNewMask: Some(create_new_mask),
		AEGP_DeleteMaskFromLayer: Some(delete_mask_from_layer),
		AEGP_GetMaskColor: Some(get_mask_color),
		AEGP_SetMaskColor: Some(set_mask_color),
		AEGP_GetMaskLockState: Some(get_mask_lock_state),
		AEGP_SetMaskLockState: Some(set_mask_lock_state),
		AEGP_GetMaskIsRotoBezier: Some(get_mask_is_roto_bezier),
		AEGP_SetMaskIsRotoBezier: Some(set_mask_is_roto_bezier),
		AEGP_DuplicateMask: Some(duplicate_mask),
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn a_layer_that_is_not_an_instance_is_refused() {
		let mut n: A_long = 7;
		let err = unsafe { suite_6().AEGP_GetLayerNumMasks.unwrap()(std::ptr::null_mut(), &mut n) };
		assert_eq!(err, PF_Err_BAD_CALLBACK_PARAM);
		let mut h: AEGP_MaskRefH = std::ptr::null_mut();
		let err = unsafe { suite_6().AEGP_GetLayerMaskByIndex.unwrap()(std::ptr::null_mut(), 0, &mut h) };
		assert_eq!(err, PF_Err_BAD_CALLBACK_PARAM);
		assert!(h.is_null());
	}

	#[test]
	fn foreign_handles_are_not_dereferenced_or_freed() {
		let mut mode: PF_MaskMode = 0;
		let fake = 0x1000 as AEGP_MaskRefH;
		assert_eq!(
			unsafe { suite_6().AEGP_GetMaskMode.unwrap()(fake, &mut mode) },
			PF_Err_BAD_CALLBACK_PARAM
		);
		assert_eq!(
			unsafe { suite_6().AEGP_DisposeMask.unwrap()(fake) },
			PF_Err_BAD_CALLBACK_PARAM
		);
	}

	#[test]
	fn edits_and_unmodelled_state_are_refused() {
		let s = suite_6();
		assert_eq!(
			unsafe { s.AEGP_CreateNewMask.unwrap()(std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()) },
			A_Err_GENERIC
		);
		assert_eq!(
			unsafe { s.AEGP_SetMaskInvert.unwrap()(std::ptr::null_mut(), 1) },
			A_Err_GENERIC
		);
	}
}
