//! `PF_EffectSequenceDataSuite1` ("PF Effect Sequence Data Suite", version 1).
//!
//! After Effects renders with multi-frame rendering turned on hand an effect
//! a `NULL` `in_data->sequence_data` (or, with it, one it must not write), and
//! tell it to read the data through this suite instead. The suite has a single
//! call, `PF_GetConstSequenceData`, which returns a read-only handle to the
//! sequence data the effect allocated in `PF_Cmd_SEQUENCE_SETUP`.
//!
//! aexlo keeps one `sequence_data` handle per [`PluginInstance`], so the
//! answer is that same handle. It is the plugin's own allocation: a plugin
//! that dereferences it sees exactly what it stored.

use crate::PluginInstance;
use crate::core::diagnostics::diag;
use crate::sys::{
	PF_ConstHandle, PF_EffectSequenceDataSuite1, PF_Err, PF_Err_BAD_CALLBACK_PARAM, PF_Err_NONE, PF_ProgPtr,
};

/// `PF_GetConstSequenceData`: write the effect's sequence-data handle to
/// `*sequence_data`.
///
/// The handle is null when the effect has not allocated sequence data (it
/// never asked for any, or `PF_Cmd_SEQUENCE_SETUP` left it unset). That is
/// success with a null handle, which is how a plugin tells "no data" from a
/// failed call. A null `effect_ref` or out-pointer, or an `effect_ref` that is
/// not an effect aexlo hosts, is `PF_Err_BAD_CALLBACK_PARAM`.
unsafe extern "C" fn get_const_sequence_data(effect_ref: PF_ProgPtr, sequence_data: *mut PF_ConstHandle) -> PF_Err {
	crate::core::ffi::guard("PF_GetConstSequenceData", || {
		diag!("PF_EffectSequenceDataSuite1/PF_GetConstSequenceData",
			"effect_ref" => format!("{:#x}", effect_ref as usize),
		);
		if sequence_data.is_null() {
			return PF_Err_BAD_CALLBACK_PARAM;
		}
		let Some(instance) = PluginInstance::get_instance_ptr(effect_ref) else {
			// SAFETY: `sequence_data` was null-checked above.
			unsafe { *sequence_data = std::ptr::null() };
			return PF_Err_BAD_CALLBACK_PARAM;
		};
		// SAFETY: `effect_ref` is the live instance running the plugin call.
		let handle = unsafe { instance.as_ref() }.sequence_data_handle();
		// SAFETY: `sequence_data` was null-checked above.
		unsafe { *sequence_data = handle as PF_ConstHandle };
		PF_Err_NONE
	})
}

pub(super) const fn suite_1() -> PF_EffectSequenceDataSuite1 {
	PF_EffectSequenceDataSuite1 {
		PF_GetConstSequenceData: Some(get_const_sequence_data),
	}
}
