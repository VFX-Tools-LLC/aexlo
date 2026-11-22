//! `AEGP Footage Suite` v11 (`AEGP_FootageSuite5`), and the footage layers,
//! items and footage handles it works on.
//!
//! aexlo's project has one comp and no footage of its own. The embedder can
//! link a layer parameter to a footage item whose file is on disk
//! ([`PluginInstance::set_layer_param_footage_path`](crate::PluginInstance::set_layer_param_footage_path)),
//! and the plugin then follows the chain After Effects serves for it:
//!
//! 1. the parameter's stream value is a layer id ([`layer_id`]),
//! 2. `AEGP_GetLayerFromLayerID` turns it into a footage layer,
//! 3. `AEGP_GetLayerSourceItem` gives the footage item of that layer
//!    (`AEGP_ItemType_FOOTAGE`),
//! 4. `AEGP_GetMainFootageFromItem` gives its footage, and
//! 5. `AEGP_GetFootagePath` the file's path as a UTF-16 memory handle.
//!
//! A footage layer, item or footage is a *tagged handle*: an integer with bit
//! 62 set that carries the effect and the parameter index. No real pointer has
//! that bit (user-space addresses are below 2^47), so [`is_handle`] tells the
//! three apart from an effect's own handles without a table, and
//! `PluginInstance::get_instance_ptr` refuses them. They own nothing, so
//! there is nothing to dispose.
//!
//! Only what a plugin needs to find the file is served: type, name and id of
//! the item, and the file count and path of the footage. Everything else about
//! a footage item (size, duration, interpretation, ...) reports itself through
//! [`observe::unsupported`](crate::observe::unsupported) and fails.

use std::ffi::c_void;
use std::path::PathBuf;

use crate::sys::*;

use super::aegp_memory::handle_from_bytes;
use crate::PluginInstance;
use crate::core::diagnostics::diag;
use crate::observe::{UnsupportedKind, unsupported};

const TAG: u64 = 1 << 62;
const KIND_SHIFT: u32 = 60;
const INDEX_SHIFT: u32 = 48;
const INDEX_MASK: u64 = 0xFFF;
const PTR_MASK: u64 = (1 << INDEX_SHIFT) - 1;

/// What a tagged handle stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
	/// The layer behind a footage-linked layer parameter.
	Layer = 1,
	/// That layer's source item.
	Item = 2,
	/// The item's main footage.
	Footage = 3,
}

/// A decoded tagged handle: the effect and the layer parameter it is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Link {
	pub kind: Kind,
	pub effect_ref: PF_ProgPtr,
	pub index: usize,
}

/// The first footage layer id. The effect's own layer is id 1; the layer of
/// the footage behind parameter `index` is `LAYER_ID_BASE + index`.
const LAYER_ID_BASE: AEGP_LayerIDVal = 0x1000;
/// Item ids, as above.
const ITEM_ID_BASE: A_long = 0x2000;

/// Whether `handle` is a footage layer, item or footage.
pub(crate) fn is_handle(handle: *mut c_void) -> bool {
	(handle as usize as u64) & TAG != 0
}

/// The tagged handle for `kind` of the layer parameter `index` of the effect
/// `effect_ref`, or `None` when they do not fit (a 32-bit host, a pointer above
/// 48 bits, a parameter index of 4096 or more).
pub(crate) fn encode(kind: Kind, effect_ref: PF_ProgPtr, index: usize) -> Option<*mut c_void> {
	if std::mem::size_of::<usize>() < 8 {
		return None;
	}
	let ptr = effect_ref as usize as u64;
	if ptr == 0 || ptr & !PTR_MASK != 0 || index as u64 > INDEX_MASK {
		return None;
	}
	let bits = TAG | (kind as u64) << KIND_SHIFT | (index as u64) << INDEX_SHIFT | ptr;
	Some(bits as usize as *mut c_void)
}

/// What `handle` says, or `None` for anything that is not a tagged handle.
pub(crate) fn decode(handle: *mut c_void) -> Option<Link> {
	if !is_handle(handle) {
		return None;
	}
	let bits = handle as usize as u64;
	let kind = match (bits >> KIND_SHIFT) & 0x3 {
		1 => Kind::Layer,
		2 => Kind::Item,
		3 => Kind::Footage,
		_ => return None,
	};
	Some(Link {
		kind,
		effect_ref: (bits & PTR_MASK) as usize as PF_ProgPtr,
		index: ((bits >> INDEX_SHIFT) & INDEX_MASK) as usize,
	})
}

/// `handle` decoded, when it is a tagged handle of `kind`.
pub(crate) fn decode_kind(handle: *mut c_void, kind: Kind) -> Option<Link> {
	decode(handle).filter(|link| link.kind == kind)
}

/// The file the layer parameter behind `link` is linked to, if it still is.
fn path_of(link: Link) -> Option<PathBuf> {
	let instance = PluginInstance::get_instance_ptr(link.effect_ref)?;
	// SAFETY: effect_ref is the live instance that is running the plugin call.
	unsafe { instance.as_ref() }
		.layer_param_footage_path(link.index)
		.map(PathBuf::from)
}

/// The layer id of the footage behind parameter `index`, or `None` when the
/// parameter is not linked to footage.
pub(crate) fn layer_id(effect_ref: PF_ProgPtr, index: usize) -> Option<AEGP_LayerIDVal> {
	path_of(Link {
		kind: Kind::Layer,
		effect_ref,
		index,
	})
	.map(|_| LAYER_ID_BASE + index as AEGP_LayerIDVal)
}

/// The footage layer with `id` in the comp of `effect_ref`, if one is linked.
pub(crate) fn layer_from_id(effect_ref: PF_ProgPtr, id: AEGP_LayerIDVal) -> Option<*mut c_void> {
	let index = usize::try_from(id.checked_sub(LAYER_ID_BASE)?).ok()?;
	path_of(Link {
		kind: Kind::Layer,
		effect_ref,
		index,
	})?;
	encode(Kind::Layer, effect_ref, index)
}

/// The source item of the footage layer `layer`.
pub(crate) fn source_item(layer: Link) -> Option<*mut c_void> {
	path_of(layer)?;
	encode(Kind::Item, layer.effect_ref, layer.index)
}

/// The id of the footage item `item`.
pub(crate) fn item_id(item: Link) -> A_long {
	ITEM_ID_BASE + item.index as A_long
}

/// The name After Effects gives a footage item: its file's name.
pub(crate) fn item_name(item: Link) -> Option<String> {
	let path = path_of(item)?;
	Some(
		path.file_name()
			.map_or_else(|| path.to_string_lossy(), |n| n.to_string_lossy())
			.into_owned(),
	)
}

/// A call about a footage item that aexlo does not answer. Reports it and
/// returns the error the plugin gets.
pub(crate) fn declined(name: &str) -> A_Err {
	unsupported(UnsupportedKind::Declined, name, "footage item");
	A_Err_GENERIC
}

/// The UTF-16, NUL-terminated bytes of `text`.
pub(crate) fn utf16_bytes(text: &str) -> Vec<u8> {
	let mut bytes = Vec::with_capacity((text.len() + 1) * 2);
	for unit in text.encode_utf16().chain(std::iter::once(0)) {
		bytes.extend_from_slice(&unit.to_le_bytes());
	}
	bytes
}

fn footage_link(footageH: AEGP_FootageH) -> Option<Link> {
	decode_kind(footageH as *mut c_void, Kind::Footage)
}

unsafe extern "C" fn get_main_footage_from_item(itemH: AEGP_ItemH, footagePH: *mut AEGP_FootageH) -> A_Err {
	crate::core::ffi::guard("get_main_footage_from_item", || {
		diag!("AEGP_FootageSuite5/AEGP_GetMainFootageFromItem", "itemH" => format!("{:#x}", itemH as usize));
		let Some(out) = (unsafe { footagePH.as_mut() }) else {
			return PF_Err_BAD_CALLBACK_PARAM;
		};
		// "error if item isn't AEGP_ItemType_FOOTAGE": the comp is the only
		// other item aexlo has.
		let Some(item) = decode_kind(itemH as *mut c_void, Kind::Item) else {
			return PF_Err_BAD_CALLBACK_PARAM;
		};
		if path_of(item).is_none() {
			return PF_Err_BAD_CALLBACK_PARAM;
		}
		let Some(handle) = encode(Kind::Footage, item.effect_ref, item.index) else {
			return PF_Err_BAD_CALLBACK_PARAM;
		};
		*out = handle as AEGP_FootageH;
		PF_Err_NONE
	})
}

/// "error if has_proxy is false": a footage item here never has a proxy.
unsafe extern "C" fn get_proxy_footage_from_item(itemH: AEGP_ItemH, proxy_footagePH: *mut AEGP_FootageH) -> A_Err {
	crate::core::ffi::guard("get_proxy_footage_from_item", || {
		diag!("AEGP_FootageSuite5/AEGP_GetProxyFootageFromItem", "itemH" => format!("{:#x}", itemH as usize));
		if proxy_footagePH.is_null() || decode_kind(itemH as *mut c_void, Kind::Item).is_none() {
			return PF_Err_BAD_CALLBACK_PARAM;
		}
		A_Err_GENERIC
	})
}

/// The footage is one file: not a sequence, no auxiliary files.
unsafe extern "C" fn get_footage_num_files(
	footageH: AEGP_FootageH,
	num_main_filesPL0: *mut A_long,
	files_per_framePL0: *mut A_long,
) -> A_Err {
	crate::core::ffi::guard("get_footage_num_files", || {
		diag!("AEGP_FootageSuite5/AEGP_GetFootageNumFiles", "footageH" => format!("{:#x}", footageH as usize));
		let Some(footage) = footage_link(footageH) else {
			return PF_Err_BAD_CALLBACK_PARAM;
		};
		if path_of(footage).is_none() {
			return PF_Err_BAD_CALLBACK_PARAM;
		}
		if let Some(out) = unsafe { num_main_filesPL0.as_mut() } {
			*out = 1;
		}
		if let Some(out) = unsafe { files_per_framePL0.as_mut() } {
			*out = 1;
		}
		PF_Err_NONE
	})
}

/// The file's path, as UTF-16 with a NUL. `frame_numL` is 0 for a single file;
/// any file other than the main one (`AEGP_FOOTAGE_MAIN_FILE_INDEX`, 0) does
/// not exist, so its path is the empty string ("empty string if no file").
unsafe extern "C" fn get_footage_path(
	footageH: AEGP_FootageH,
	frame_numL: A_long,
	file_indexL: A_long,
	unicode_pathPH: *mut AEGP_MemHandle,
) -> A_Err {
	crate::core::ffi::guard("get_footage_path", || {
		diag!("AEGP_FootageSuite5/AEGP_GetFootagePath",
			"footageH" => format!("{:#x}", footageH as usize),
			"frame_numL" => frame_numL,
			"file_indexL" => file_indexL,
		);
		let Some(out) = (unsafe { unicode_pathPH.as_mut() }) else {
			return PF_Err_BAD_CALLBACK_PARAM;
		};
		let Some(path) = footage_link(footageH).and_then(path_of) else {
			return PF_Err_BAD_CALLBACK_PARAM;
		};
		if frame_numL != 0 {
			return PF_Err_INVALID_INDEX;
		}
		let text = if file_indexL == 0 {
			path.to_string_lossy().into_owned()
		} else {
			String::new()
		};
		*out = handle_from_bytes(utf16_bytes(&text));
		PF_Err_NONE
	})
}

/// Footage calls aexlo does not answer: the footage is a file's path and
/// nothing more. Each reports itself and fails.
macro_rules! declined_fn {
	($fn_name:ident, $name:literal, $($arg:ident: $ty:ty),* $(,)?) => {
		#[allow(unused_variables)]
		unsafe extern "C" fn $fn_name($($arg: $ty),*) -> A_Err {
			crate::core::ffi::guard(stringify!($fn_name), || {
				diag!(concat!("AEGP_FootageSuite5/", $name));
				unsupported(UnsupportedKind::Declined, concat!("AEGP_FootageSuite5/", $name), "");
				A_Err_GENERIC
			})
		}
	};
}

declined_fn!(get_footage_signature, "AEGP_GetFootageSignature",
	footageH: AEGP_FootageH, sigP: *mut AEGP_FootageSignature);
declined_fn!(new_footage, "AEGP_NewFootage",
	aegp_plugin_id: AEGP_PluginID, pathZ: *const A_UTF16Char, layer_infoP0: *const AEGP_FootageLayerKey,
	sequence_optionsP0: *const AEGP_FileSequenceImportOptions, interp_style: AEGP_InterpretationStyle,
	reserved: *mut c_void, footagePH: *mut AEGP_FootageH);
declined_fn!(add_footage_to_project, "AEGP_AddFootageToProject",
	footageH: AEGP_FootageH, folderH: AEGP_ItemH, added_itemPH0: *mut AEGP_ItemH);
declined_fn!(set_item_proxy_footage, "AEGP_SetItemProxyFootage",
	footageH: AEGP_FootageH, itemH: AEGP_ItemH);
declined_fn!(replace_item_main_footage, "AEGP_ReplaceItemMainFootage",
	footageH: AEGP_FootageH, itemH: AEGP_ItemH);
declined_fn!(dispose_footage, "AEGP_DisposeFootage", footageH: AEGP_FootageH);
declined_fn!(get_footage_interpretation, "AEGP_GetFootageInterpretation",
	itemH: AEGP_ItemH, proxyB: A_Boolean, interpP: *mut AEGP_FootageInterp);
declined_fn!(set_footage_interpretation, "AEGP_SetFootageInterpretation",
	itemH: AEGP_ItemH, proxyB: A_Boolean, interpP: *const AEGP_FootageInterp);
declined_fn!(get_footage_layer_key, "AEGP_GetFootageLayerKey",
	footageH: AEGP_FootageH, layerKeyP: *mut AEGP_FootageLayerKey);
declined_fn!(new_placeholder_footage, "AEGP_NewPlaceholderFootage",
	plugin_id: AEGP_PluginID, nameZ: *const A_char, width: A_long, height: A_long,
	durationPT: *const A_Time, footagePH: *mut AEGP_FootageH);
declined_fn!(new_placeholder_footage_with_path, "AEGP_NewPlaceholderFootageWithPath",
	plugin_id: AEGP_PluginID, pathZ: *const A_UTF16Char, path_platform: AEGP_Platform, file_type: AEIO_FileType,
	widthL: A_long, heightL: A_long, durationPT: *const A_Time, footagePH: *mut AEGP_FootageH);
declined_fn!(new_solid_footage, "AEGP_NewSolidFootage",
	nameZ: *const A_char, width: A_long, height: A_long, colorP: *const AEGP_ColorVal, footagePH: *mut AEGP_FootageH);
declined_fn!(get_solid_footage_color, "AEGP_GetSolidFootageColor",
	itemH: AEGP_ItemH, proxyB: A_Boolean, colorP: *mut AEGP_ColorVal);
declined_fn!(set_solid_footage_color, "AEGP_SetSolidFootageColor",
	itemH: AEGP_ItemH, proxyB: A_Boolean, colorP: *const AEGP_ColorVal);
declined_fn!(set_solid_footage_dimensions, "AEGP_SetSolidFootageDimensions",
	itemH: AEGP_ItemH, proxyB: A_Boolean, widthL: A_long, heightL: A_long);
declined_fn!(get_footage_sound_data_format, "AEGP_GetFootageSoundDataFormat",
	footageH: AEGP_FootageH, sound_formatP: *mut AEGP_SoundDataFormat);
declined_fn!(get_footage_sequence_import_options, "AEGP_GetFootageSequenceImportOptions",
	footageH: AEGP_FootageH, optionsP: *mut AEGP_FileSequenceImportOptions);

pub(crate) const fn suite_5() -> AEGP_FootageSuite5 {
	AEGP_FootageSuite5 {
		AEGP_GetMainFootageFromItem: Some(get_main_footage_from_item),
		AEGP_GetProxyFootageFromItem: Some(get_proxy_footage_from_item),
		AEGP_GetFootageNumFiles: Some(get_footage_num_files),
		AEGP_GetFootagePath: Some(get_footage_path),
		AEGP_GetFootageSignature: Some(get_footage_signature),
		AEGP_NewFootage: Some(new_footage),
		AEGP_AddFootageToProject: Some(add_footage_to_project),
		AEGP_SetItemProxyFootage: Some(set_item_proxy_footage),
		AEGP_ReplaceItemMainFootage: Some(replace_item_main_footage),
		AEGP_DisposeFootage: Some(dispose_footage),
		AEGP_GetFootageInterpretation: Some(get_footage_interpretation),
		AEGP_SetFootageInterpretation: Some(set_footage_interpretation),
		AEGP_GetFootageLayerKey: Some(get_footage_layer_key),
		AEGP_NewPlaceholderFootage: Some(new_placeholder_footage),
		AEGP_NewPlaceholderFootageWithPath: Some(new_placeholder_footage_with_path),
		AEGP_NewSolidFootage: Some(new_solid_footage),
		AEGP_GetSolidFootageColor: Some(get_solid_footage_color),
		AEGP_SetSolidFootageColor: Some(set_solid_footage_color),
		AEGP_SetSolidFootageDimensions: Some(set_solid_footage_dimensions),
		AEGP_GetFootageSoundDataFormat: Some(get_footage_sound_data_format),
		AEGP_GetFootageSequenceImportOptions: Some(get_footage_sequence_import_options),
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn handles_round_trip_and_never_look_like_pointers() {
		let effect = 0x0000_1234_5678_9abc_usize as PF_ProgPtr;
		for kind in [Kind::Layer, Kind::Item, Kind::Footage] {
			let handle = encode(kind, effect, 462).unwrap();
			assert!(is_handle(handle));
			assert_eq!(
				decode(handle),
				Some(Link {
					kind,
					effect_ref: effect,
					index: 462
				})
			);
		}
		// A real pointer is not a handle.
		let value = 5u32;
		assert!(!is_handle(&value as *const u32 as *mut c_void));
		assert!(decode(&value as *const u32 as *mut c_void).is_none());
		assert!(encode(Kind::Layer, effect, 4096).is_none());
		assert!(encode(Kind::Layer, std::ptr::null_mut(), 1).is_none());
	}

	#[test]
	fn utf16_is_nul_terminated_little_endian() {
		assert_eq!(utf16_bytes("a\u{e9}"), vec![b'a', 0, 0xe9, 0, 0, 0]);
	}

	#[test]
	fn unlinked_handles_answer_nothing() {
		let suite = suite_5();
		let mut footage: AEGP_FootageH = std::ptr::null_mut();
		// Not a tagged item at all.
		let err = unsafe { suite.AEGP_GetMainFootageFromItem.unwrap()(0x10 as AEGP_ItemH, &mut footage) };
		assert_eq!(err, PF_Err_BAD_CALLBACK_PARAM);
		let mut path: AEGP_MemHandle = std::ptr::null_mut();
		let err = unsafe { suite.AEGP_GetFootagePath.unwrap()(std::ptr::null_mut(), 0, 0, &mut path) };
		assert_eq!(err, PF_Err_BAD_CALLBACK_PARAM);
	}
}
