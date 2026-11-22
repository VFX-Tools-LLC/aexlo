//! The Effect Sequence Data Suite (`PF_EffectSequenceDataSuite1`) through the
//! real C ABI.
//!
//! `tests/probe_sequence` is a raw-SDK effect that allocates sequence data in
//! `PF_Cmd_SEQUENCE_SETUP`, then at render acquires the suite with
//! `SPBasicSuite::AcquireSuite`, calls `PF_GetConstSequenceData` and writes
//! what it saw into its output pixels. Before this suite was served, the
//! acquire failed and the probe saw nothing.
//!
//! The probe is built with `cargo build -p aexlo_probe_sequence` on first use.

use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

use aexlo::{Depth8, Layer, PluginInstance, RenderPath};

/// Slot numbers, mirroring `tests/probe_sequence/src/lib.rs`.
mod slot {
	pub const MAGIC_AT: usize = 0;
	pub const MAGIC: u16 = 0x5351;
	pub const ACQUIRE_ERR: usize = 1;
	pub const GET_ERR: usize = 2;
	pub const HANDLE_SET: usize = 3;
	pub const MARKER_LOW: usize = 4;
	pub const MARKER_HIGH: usize = 5;
	pub const SAME_HANDLE: usize = 6;
	pub const NULL_REF_ERR: usize = 7;
	pub const NULL_OUT_ERR: usize = 8;
	pub const TOTAL: usize = 9;
}

/// What the probe stores in its sequence data.
const MARKER: u32 = 0xC0DE_5EED;
/// `PF_Err_BAD_CALLBACK_PARAM`.
const ERR_BAD_CALLBACK_PARAM: u16 = 516;

/// The probe's cdylib, built once per test process.
fn probe_path() -> &'static PathBuf {
	static PATH: OnceLock<PathBuf> = OnceLock::new();
	PATH.get_or_init(|| {
		let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
		let out = Command::new(cargo)
			.args([
				"build",
				"-p",
				"aexlo_probe_sequence",
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
			.filter(|l| l.contains("\"compiler-artifact\"") && l.contains("aexlo_probe_sequence"))
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

/// Render the probe once and decode its report.
fn report() -> Vec<u16> {
	let mut instance: PluginInstance = aexlo::Host::get().load(probe_path()).expect("loading the probe");
	instance.set_input_layer(Layer::<Depth8>::black(64, 48));
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

#[test]
fn the_suite_is_served() {
	let s = report();
	assert_eq!(
		s[slot::ACQUIRE_ERR],
		0,
		"PF Effect Sequence Data Suite v1 was not served"
	);
}

#[test]
fn the_const_handle_reaches_the_data_the_effect_allocated() {
	let s = report();
	assert_eq!(s[slot::ACQUIRE_ERR], 0, "the suite was not served");
	assert_eq!(s[slot::GET_ERR], 0);
	assert_eq!(s[slot::HANDLE_SET], 1, "the handle was null");
	let found = u32::from(s[slot::MARKER_HIGH]) << 16 | u32::from(s[slot::MARKER_LOW]);
	assert_eq!(found, MARKER, "the handle does not lead to the stored data");
	assert_eq!(
		s[slot::SAME_HANDLE],
		1,
		"it is not the instance's own sequence_data handle"
	);
}

#[test]
fn bad_arguments_are_refused() {
	let s = report();
	assert_eq!(s[slot::ACQUIRE_ERR], 0, "the suite was not served");
	assert_eq!(s[slot::NULL_REF_ERR], ERR_BAD_CALLBACK_PARAM);
	assert_eq!(s[slot::NULL_OUT_ERR], ERR_BAD_CALLBACK_PARAM);
}
