//! The AEGP Proj Suite (v9, `AEGP_ProjSuite6`) through the real C ABI.
//!
//! `tests/probe_proj` is a raw-SDK effect that acquires the suite with
//! `SPBasicSuite::AcquireSuite`, reads the project and writes what it saw
//! into its output pixels. These tests check the probe saw what aexlo's one
//! fixed project holds, and was refused the rest.
//!
//! The host (and so the project's time display) is process-wide and set once,
//! which is why this lives in its own test binary. The probe is built with
//! `cargo build -p aexlo_probe_proj` on first use.

use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

use aexlo::{
	AppHost, Depth8, Depth16, Depth32, Host, Layer, PluginInstance, RenderPath, TimeDisplayMode, TimeDisplayPref,
};

/// Slot numbers, mirroring `tests/probe_proj/src/lib.rs`.
#[allow(dead_code)]
mod slot {
	pub const MAGIC_AT: usize = 0;
	pub const MAGIC: u16 = 0x5052;
	pub const ACQUIRE_ERR: usize = 1;
	pub const NUM_ERR: usize = 2;
	pub const NUM: usize = 3;
	pub const BY_INDEX_ERR: usize = 4;
	pub const BY_INDEX_HANDLE: usize = 5;
	pub const BY_INDEX_PAST_END_ERR: usize = 6;
	pub const BY_INDEX_PAST_END_HANDLE_NULL: usize = 7;
	pub const BY_INDEX_NEGATIVE_ERR: usize = 8;
	pub const TD_ERR: usize = 9;
	pub const TD_MODE: usize = 10;
	pub const TD_FOOTAGE_MODE: usize = 11;
	pub const TD_DROPFRAME: usize = 12;
	pub const TD_FEET_FRAMES: usize = 13;
	pub const TD_TIMEBASE: usize = 14;
	pub const TD_FRAMES_PER_FOOT: usize = 15;
	pub const TD_FRAMES_MODE: usize = 16;
	pub const DEPTH_ERR: usize = 17;
	pub const DEPTH: usize = 18;
	pub const NAME_ERR: usize = 19;
	pub const PATH_ERR: usize = 20;
	pub const ROOT_ERR: usize = 21;
	pub const DIRTY_ERR: usize = 22;
	pub const SET_DEPTH_ERR: usize = 23;
	pub const NEW_ERR: usize = 24;
	pub const ITEM_ERR: usize = 25;
	pub const ITEM_IS_COMP: usize = 26;
	pub const BAD_HANDLE_ERR: usize = 27;
	pub const TOTAL: usize = 28;
}

/// `PF_Err_INVALID_INDEX`, `PF_Err_BAD_CALLBACK_PARAM`, `A_Err_GENERIC`.
const ERR_INVALID_INDEX: u16 = 513;
const ERR_BAD_CALLBACK_PARAM: u16 = 516;
const ERR_GENERIC: u16 = 1;

/// A project that shows frame numbers counting from 1 at 24 fps with
/// drop-frame timecode, like a user who set Project Settings > Frames.
struct FramesFromOne;

impl AppHost for FramesFromOne {
	fn time_display_pref(&self) -> TimeDisplayPref {
		TimeDisplayPref {
			mode: TimeDisplayMode::Frames,
			timebase: 24,
			frames_start: 1,
			non_drop: false,
			honor_source_timecode: false,
			..TimeDisplayPref::default()
		}
	}
}

fn host() -> &'static Host {
	static HOST: OnceLock<Host> = OnceLock::new();
	HOST.get_or_init(|| Host::install(FramesFromOne).expect("host already installed"))
}

/// The probe's cdylib, built once per test process.
fn probe_path() -> &'static PathBuf {
	static PATH: OnceLock<PathBuf> = OnceLock::new();
	PATH.get_or_init(|| {
		let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
		let out = Command::new(cargo)
			.args([
				"build",
				"-p",
				"aexlo_probe_proj",
				"--message-format=json-render-diagnostics",
			])
			.current_dir(env!("CARGO_MANIFEST_DIR"))
			.output()
			.expect("spawning cargo");
		assert!(
			out.status.success(),
			"building the probe failed:\n{}",
			String::from_utf8_lossy(&out.stderr)
		);
		let stdout = String::from_utf8_lossy(&out.stdout);
		stdout
			.lines()
			.filter(|l| l.contains("\"compiler-artifact\"") && l.contains("aexlo_probe_proj"))
			.filter_map(|l| {
				let files = l.split("\"filenames\":[").nth(1)?.split(']').next()?;
				files
					.split(',')
					.map(|f| f.trim().trim_matches('"').replace("\\\\", "\\"))
					.find(|f| f.ends_with(".dll") || f.ends_with(".dylib") || f.ends_with(".so"))
			})
			.next_back()
			.map(PathBuf::from)
			.expect("no cdylib among the probe's build artifacts")
	})
}

/// Render the probe after `set_input` and decode its report.
fn report_with(set_input: impl FnOnce(&mut PluginInstance)) -> Vec<u16> {
	let mut instance: PluginInstance = host().load(probe_path()).expect("loading the probe");
	set_input(&mut instance);
	instance.render_with(RenderPath::Legacy).expect("probe render failed");

	let (w, h) = instance.output_size();
	let mut rgba = vec![0u8; (w * h * 4) as usize];
	instance.write_output_rgba8(&mut rgba).unwrap();
	let slots: Vec<u16> = rgba
		.chunks(4)
		.take(slot::TOTAL)
		.map(|p| u16::from(p[0]) << 8 | u16::from(p[1]))
		.collect();
	assert_eq!(slots[slot::MAGIC_AT], slot::MAGIC, "the probe did not run");
	slots
}

fn report() -> Vec<u16> {
	report_with(|i| i.set_input_layer(Layer::<Depth8>::black(64, 48)))
}

#[test]
fn there_is_one_project_and_it_is_index_zero() {
	let s = report();
	assert_eq!(s[slot::ACQUIRE_ERR], 0, "the suite was not served");
	assert_eq!((s[slot::NUM_ERR], s[slot::NUM]), (0, 1));
	assert_eq!((s[slot::BY_INDEX_ERR], s[slot::BY_INDEX_HANDLE]), (0, 1));
	assert_eq!(s[slot::BY_INDEX_PAST_END_ERR], ERR_INVALID_INDEX);
	assert_eq!(s[slot::BY_INDEX_PAST_END_HANDLE_NULL], 1);
	assert_eq!(s[slot::BY_INDEX_NEGATIVE_ERR], ERR_INVALID_INDEX);
}

/// What the host's `time_display_pref` says is what `AEGP_GetProjectTimeDisplay`
/// answers: Frames display, 24 fps, drop-frame on, frames numbered from 1, no
/// source timecode. ProjectIt and Wrap It 2 add 1 to the frame number in
/// exactly this case.
#[test]
fn time_display_is_the_hosts_preference() {
	let s = report();
	assert_eq!(s[slot::ACQUIRE_ERR], 0, "the suite was not served");
	assert_eq!(s[slot::TD_ERR], 0);
	assert_eq!(s[slot::TD_MODE], 1, "AEGP_TimeDisplay_FRAMES");
	assert_eq!(s[slot::TD_FOOTAGE_MODE], 0, "AEGP_SourceTimecode_ZERO");
	assert_eq!(s[slot::TD_DROPFRAME], 1);
	assert_eq!(s[slot::TD_FEET_FRAMES], 0);
	assert_eq!(s[slot::TD_TIMEBASE], 24);
	assert_eq!(s[slot::TD_FRAMES_PER_FOOT], 16);
	assert_eq!(s[slot::TD_FRAMES_MODE], 1, "AEGP_Frames_ONE_BASED");
}

/// The project depth is the depth of the render, as in After Effects.
#[test]
fn bit_depth_follows_the_project_depth() {
	let depth = |s: &[u16]| (s[slot::DEPTH_ERR], s[slot::DEPTH]);
	let s = report();
	assert_eq!(s[slot::ACQUIRE_ERR], 0, "the suite was not served");
	assert_eq!(depth(&s), (0, 0), "AEGP_ProjBitDepth_8");
	let s = report_with(|i| i.set_input_layer(Layer::<Depth16>::black(64, 48)));
	assert_eq!(depth(&s), (0, 1), "AEGP_ProjBitDepth_16");
	let s = report_with(|i| i.set_input_layer(Layer::<Depth32>::black(64, 48)));
	assert_eq!(depth(&s), (0, 2), "AEGP_ProjBitDepth_32");
}

/// A project that was never loaded from or saved to a file has no name, path
/// or dirty flag, and aexlo cannot edit it: those calls fail, they do not
/// pretend. (They also report through `observe::unsupported`.)
#[test]
fn what_the_project_does_not_model_is_refused() {
	let s = report();
	assert_eq!(s[slot::ACQUIRE_ERR], 0, "the suite was not served");
	for (name, at) in [
		("GetProjectName", slot::NAME_ERR),
		("GetProjectPath", slot::PATH_ERR),
		("GetProjectRootFolder", slot::ROOT_ERR),
		("ProjectIsDirty", slot::DIRTY_ERR),
		("SetProjectBitDepth", slot::SET_DEPTH_ERR),
		("NewProject", slot::NEW_ERR),
	] {
		assert_eq!(s[at], ERR_GENERIC, "{name} must be refused");
	}
	assert_eq!(s[slot::BAD_HANDLE_ERR], ERR_BAD_CALLBACK_PARAM);
}

/// The project handle is the one the Item Suite takes back, and the project's
/// one item is the comp.
#[test]
fn the_item_suite_walks_from_the_project_handle() {
	let s = report();
	assert_eq!(s[slot::ACQUIRE_ERR], 0, "the suite was not served");
	assert_eq!(s[slot::ITEM_ERR], 0);
	assert_eq!(s[slot::ITEM_IS_COMP], 1);
}
