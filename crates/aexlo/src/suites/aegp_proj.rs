//! `AEGP Proj Suite`, wire version 9 (`AEGP_ProjSuite6`): the project as the
//! plugin sees it.
//!
//! aexlo has exactly one project, and it holds one comp (see
//! [`aegp_item`](super::aegp_item)). Its handle is the effect's `PF_ProgPtr`,
//! the same pointer as the comp, layer and item handles, so `AEGP_GetFirstProjItem`
//! takes it back. After Effects never asks a plugin to dispose of it.
//!
//! What the project model holds is answered:
//!
//! - the number of projects (1) and the project at index 0;
//! - the bit depth, which is the depth of the instance's input layer (the
//!   project depth, as in After Effects);
//! - the time display settings, which are the host's [`TimeDisplayPref`]
//!   (the same one `PF_GetTimeDisplayPref` reports), so a plugin that numbers
//!   frames from the project's "Frames" setting sees what the embedder chose.
//!
//! Everything else reports through [`unsupported`] and fails: the project has
//! no name, no path and no dirty flag (it is never loaded from or saved to a
//! file), its root folder is not modelled, and creating, opening, saving or
//! editing a project is refused.

use crate::core::diagnostics::diag;
use crate::host::app::{TimeDisplayMode, TimeDisplayPref, with_app_host_ffi};
use crate::observe::{UnsupportedKind, unsupported};
use crate::sys::*;

use super::layer::{Timeline, current_effect};

fn declined(name: &str) -> A_Err {
	unsupported(UnsupportedKind::Declined, format!("AEGP_ProjSuite/{name}"), "");
	A_Err_GENERIC
}

/// Whether `projectH` is the project of the effect that is running.
fn known(projectH: AEGP_ProjectH) -> bool {
	Timeline::of(projectH as _).is_some()
}

/// `AEGP_TimeDisplay3` for a host's time display preference.
fn time_display_of(pref: &TimeDisplayPref) -> AEGP_TimeDisplay3 {
	let clamp = |v: i32| v.clamp(A_char::MIN as i32, A_char::MAX as i32) as A_char;
	AEGP_TimeDisplay3 {
		display_mode: match pref.mode {
			TimeDisplayMode::Timecode => AEGP_TimeDisplay_TIMECODE,
			TimeDisplayMode::Frames => AEGP_TimeDisplay_FRAMES,
		} as AEGP_TimeDisplayMode,
		footage_display_mode: if pref.honor_source_timecode {
			AEGP_SourceTimecode_SOURCE_TIMECODE
		} else {
			AEGP_SourceTimecode_ZERO
		} as AEGP_SourceTimecodeDisplayMode,
		display_dropframeB: !pref.non_drop as A_Boolean,
		use_feet_framesB: pref.use_feet_frames as A_Boolean,
		timebaseC: clamp(pref.timebase),
		frames_per_footC: clamp(pref.frames_per_foot),
		frames_display_mode: match pref.frames_start {
			1 => AEGP_Frames_ONE_BASED,
			2 => AEGP_Frames_TIMECODE_CONVERSION,
			_ => AEGP_Frames_ZERO_BASED,
		} as AEGP_FramesDisplayMode,
	}
}

unsafe extern "C" fn get_num_projects(num_projPL: *mut A_long) -> A_Err {
	crate::core::ffi::guard("get_num_projects", || {
		diag!("AEGP_ProjSuite/AEGP_GetNumProjects");
		let Some(out) = (unsafe { num_projPL.as_mut() }) else {
			return PF_Err_BAD_CALLBACK_PARAM;
		};
		*out = 1;
		PF_Err_NONE
	})
}

unsafe extern "C" fn get_project_by_index(proj_indexL: A_long, projPH: *mut AEGP_ProjectH) -> A_Err {
	crate::core::ffi::guard("get_project_by_index", || {
		diag!("AEGP_ProjSuite/AEGP_GetProjectByIndex", "proj_indexL" => proj_indexL);
		let Some(out) = (unsafe { projPH.as_mut() }) else {
			return PF_Err_BAD_CALLBACK_PARAM;
		};
		*out = std::ptr::null_mut();
		if proj_indexL != 0 {
			return PF_Err_INVALID_INDEX;
		}
		let effect = current_effect();
		if Timeline::of(effect as _).is_none() {
			return PF_Err_BAD_CALLBACK_PARAM;
		}
		*out = effect as AEGP_ProjectH;
		PF_Err_NONE
	})
}

unsafe extern "C" fn get_project_name(_projH: AEGP_ProjectH, _nameZ: *mut A_char) -> A_Err {
	crate::core::ffi::guard("get_project_name", || declined("AEGP_GetProjectName"))
}

unsafe extern "C" fn get_project_path(_projH: AEGP_ProjectH, _unicode_pathPH: *mut AEGP_MemHandle) -> A_Err {
	crate::core::ffi::guard("get_project_path", || declined("AEGP_GetProjectPath"))
}

unsafe extern "C" fn get_project_root_folder(_projH: AEGP_ProjectH, _root_folderPH: *mut AEGP_ItemH) -> A_Err {
	crate::core::ffi::guard("get_project_root_folder", || declined("AEGP_GetProjectRootFolder"))
}

unsafe extern "C" fn save_project_to_path(_projH: AEGP_ProjectH, _pathZ: *const A_UTF16Char) -> A_Err {
	crate::core::ffi::guard("save_project_to_path", || declined("AEGP_SaveProjectToPath"))
}

unsafe extern "C" fn get_project_time_display(projH: AEGP_ProjectH, time_displayP: *mut AEGP_TimeDisplay3) -> A_Err {
	crate::core::ffi::guard("get_project_time_display", || {
		diag!("AEGP_ProjSuite/AEGP_GetProjectTimeDisplay", "projH" => format!("{:#x}", projH as usize));
		let Some(out) = (unsafe { time_displayP.as_mut() }) else {
			return PF_Err_BAD_CALLBACK_PARAM;
		};
		if !known(projH) {
			return PF_Err_BAD_CALLBACK_PARAM;
		}
		with_app_host_ffi(|host| {
			*out = time_display_of(&host.time_display_pref());
			PF_Err_NONE
		})
	})
}

unsafe extern "C" fn set_project_time_display(
	_projH: AEGP_ProjectH,
	_time_displayP: *const AEGP_TimeDisplay3,
) -> A_Err {
	crate::core::ffi::guard("set_project_time_display", || declined("AEGP_SetProjectTimeDisplay"))
}

unsafe extern "C" fn project_is_dirty(_projH: AEGP_ProjectH, _is_dirtyPB: *mut A_Boolean) -> A_Err {
	crate::core::ffi::guard("project_is_dirty", || declined("AEGP_ProjectIsDirty"))
}

unsafe extern "C" fn save_project_as(_projH: AEGP_ProjectH, _pathZ: *const A_UTF16Char) -> A_Err {
	crate::core::ffi::guard("save_project_as", || declined("AEGP_SaveProjectAs"))
}

unsafe extern "C" fn new_project(_new_projectPH: *mut AEGP_ProjectH) -> A_Err {
	crate::core::ffi::guard("new_project", || declined("AEGP_NewProject"))
}

unsafe extern "C" fn open_project_from_path(_pathZ: *const A_UTF16Char, _projectPH: *mut AEGP_ProjectH) -> A_Err {
	crate::core::ffi::guard("open_project_from_path", || declined("AEGP_OpenProjectFromPath"))
}

unsafe extern "C" fn get_project_bit_depth(projectH: AEGP_ProjectH, bit_depthP: *mut AEGP_ProjBitDepth) -> A_Err {
	crate::core::ffi::guard("get_project_bit_depth", || {
		diag!("AEGP_ProjSuite/AEGP_GetProjectBitDepth", "projectH" => format!("{:#x}", projectH as usize));
		let Some(out) = (unsafe { bit_depthP.as_mut() }) else {
			return PF_Err_BAD_CALLBACK_PARAM;
		};
		let Some(instance) = crate::PluginInstance::get_instance_ptr(projectH as PF_ProgPtr) else {
			return PF_Err_BAD_CALLBACK_PARAM;
		};
		// SAFETY: the project handle is the live instance running the plugin call.
		*out = match unsafe { instance.as_ref() }.input_depth() {
			crate::PixelDepthKind::U8 => AEGP_ProjBitDepth_8,
			crate::PixelDepthKind::U16 => AEGP_ProjBitDepth_16,
			crate::PixelDepthKind::F32 => AEGP_ProjBitDepth_32,
		} as AEGP_ProjBitDepth;
		PF_Err_NONE
	})
}

unsafe extern "C" fn set_project_bit_depth(_projectH: AEGP_ProjectH, _bit_depth: AEGP_ProjBitDepth) -> A_Err {
	crate::core::ffi::guard("set_project_bit_depth", || declined("AEGP_SetProjectBitDepth"))
}

pub(super) const fn suite_6() -> AEGP_ProjSuite6 {
	AEGP_ProjSuite6 {
		AEGP_GetNumProjects: Some(get_num_projects),
		AEGP_GetProjectByIndex: Some(get_project_by_index),
		AEGP_GetProjectName: Some(get_project_name),
		AEGP_GetProjectPath: Some(get_project_path),
		AEGP_GetProjectRootFolder: Some(get_project_root_folder),
		AEGP_SaveProjectToPath: Some(save_project_to_path),
		AEGP_GetProjectTimeDisplay: Some(get_project_time_display),
		AEGP_SetProjectTimeDisplay: Some(set_project_time_display),
		AEGP_ProjectIsDirty: Some(project_is_dirty),
		AEGP_SaveProjectAs: Some(save_project_as),
		AEGP_NewProject: Some(new_project),
		AEGP_OpenProjectFromPath: Some(open_project_from_path),
		AEGP_GetProjectBitDepth: Some(get_project_bit_depth),
		AEGP_SetProjectBitDepth: Some(set_project_bit_depth),
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn one_project_at_index_zero_only() {
		let s = suite_6();
		let mut n: A_long = 0;
		assert_eq!(unsafe { s.AEGP_GetNumProjects.unwrap()(&mut n) }, PF_Err_NONE);
		assert_eq!(n, 1);
		// Outside a plugin call there is no effect, hence no project to hand out.
		let mut h: AEGP_ProjectH = 1usize as AEGP_ProjectH;
		assert_eq!(
			unsafe { s.AEGP_GetProjectByIndex.unwrap()(0, &mut h) },
			PF_Err_BAD_CALLBACK_PARAM
		);
		assert!(h.is_null());
		assert_eq!(
			unsafe { s.AEGP_GetProjectByIndex.unwrap()(1, &mut h) },
			PF_Err_INVALID_INDEX
		);
	}

	#[test]
	fn a_null_project_handle_is_refused() {
		let s = suite_6();
		let fake: AEGP_ProjectH = std::ptr::null_mut();
		let mut td: AEGP_TimeDisplay3 = unsafe { std::mem::zeroed() };
		assert_eq!(
			unsafe { s.AEGP_GetProjectTimeDisplay.unwrap()(fake, &mut td) },
			PF_Err_BAD_CALLBACK_PARAM
		);
		let mut depth: AEGP_ProjBitDepth = 7;
		assert_eq!(
			unsafe { s.AEGP_GetProjectBitDepth.unwrap()(fake, &mut depth) },
			PF_Err_BAD_CALLBACK_PARAM
		);
		assert_eq!(depth, 7);
	}

	#[test]
	fn unmodelled_state_and_edits_are_refused() {
		use std::ptr::{null, null_mut};
		let s = suite_6();
		let proj: AEGP_ProjectH = null_mut();
		assert_eq!(
			unsafe { s.AEGP_GetProjectName.unwrap()(proj, null_mut()) },
			A_Err_GENERIC
		);
		assert_eq!(
			unsafe { s.AEGP_GetProjectPath.unwrap()(proj, null_mut()) },
			A_Err_GENERIC
		);
		assert_eq!(
			unsafe { s.AEGP_GetProjectRootFolder.unwrap()(proj, null_mut()) },
			A_Err_GENERIC
		);
		assert_eq!(
			unsafe { s.AEGP_ProjectIsDirty.unwrap()(proj, null_mut()) },
			A_Err_GENERIC
		);
		assert_eq!(unsafe { s.AEGP_NewProject.unwrap()(null_mut()) }, A_Err_GENERIC);
		assert_eq!(unsafe { s.AEGP_SetProjectBitDepth.unwrap()(proj, 0) }, A_Err_GENERIC);
		assert_eq!(
			unsafe { s.AEGP_SetProjectTimeDisplay.unwrap()(proj, null()) },
			A_Err_GENERIC
		);
	}

	#[test]
	fn time_display_follows_the_host_preference() {
		let d = time_display_of(&TimeDisplayPref::default());
		assert_eq!(d.display_mode, AEGP_TimeDisplay_TIMECODE as AEGP_TimeDisplayMode);
		assert_eq!(d.timebaseC, 30);
		assert_eq!(d.display_dropframeB, 0);
		assert_eq!(d.frames_display_mode, AEGP_Frames_ZERO_BASED as AEGP_FramesDisplayMode);

		let d = time_display_of(&TimeDisplayPref {
			mode: TimeDisplayMode::Frames,
			timebase: 24,
			frames_start: 1,
			non_drop: false,
			honor_source_timecode: false,
			use_feet_frames: true,
			..TimeDisplayPref::default()
		});
		assert_eq!(d.display_mode, AEGP_TimeDisplay_FRAMES as AEGP_TimeDisplayMode);
		assert_eq!(d.timebaseC, 24);
		assert_eq!(d.display_dropframeB, 1);
		assert_eq!(d.use_feet_framesB, 1);
		assert_eq!(
			d.footage_display_mode,
			AEGP_SourceTimecode_ZERO as AEGP_SourceTimecodeDisplayMode
		);
		assert_eq!(d.frames_display_mode, AEGP_Frames_ONE_BASED as AEGP_FramesDisplayMode);
	}
}
