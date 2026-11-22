//! The AEGP Layer Mask Suite (v7, `AEGP_MaskSuite6`) through the real C ABI.
//!
//! `tests/probe_masks` is a raw-SDK effect that acquires the suite with
//! `SPBasicSuite::AcquireSuite`, reads the layer's masks and writes what it
//! saw into its output pixels. These tests attach masks with
//! `PluginInstance::set_mask_paths` and check the probe saw exactly them.
//!
//! The probe is built with `cargo build -p aexlo_probe_masks` on first use.

use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

use aexlo::{Depth8, Layer, MaskMode, MaskPath, MaskVertex, PluginInstance, RenderPath};

/// Slot numbers, mirroring `tests/probe_masks/src/lib.rs`.
#[allow(dead_code)]
mod slot {
	pub const MAGIC_AT: usize = 0;
	pub const MAGIC: u16 = 0x4d4b;
	pub const ACQUIRE_ERR: usize = 1;
	pub const LAYER_ERR: usize = 2;
	pub const NUM_MASKS_ERR: usize = 3;
	pub const NUM_MASKS: usize = 4;
	pub const BY_INDEX_PAST_END_ERR: usize = 5;
	pub const BY_INDEX_NEGATIVE_ERR: usize = 6;
	pub const BY_INDEX_PAST_END_HANDLE_NULL: usize = 7;
	pub const MASK_BASE: usize = 8;
	pub const MASK_STRIDE: usize = 8;
	pub const GET_ERR: usize = 0;
	pub const INVERT: usize = 1;
	pub const MODE: usize = 2;
	pub const ID: usize = 3;
	pub const LOCKED: usize = 4;
	pub const ROTO: usize = 5;
	pub const DISPOSE_ERR: usize = 6;
	pub const DOUBLE_DISPOSE_ERR: usize = 7;
	pub const TOTAL: usize = MASK_BASE + MASK_STRIDE * 3;
}

/// `PF_Err_INVALID_INDEX`, `PF_Err_BAD_CALLBACK_PARAM`.
const ERR_INVALID_INDEX: u16 = 513;
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
				"aexlo_probe_masks",
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
			.filter(|l| l.contains("\"compiler-artifact\"") && l.contains("aexlo_probe_masks"))
			.filter_map(|l| {
				let files = l.split("\"filenames\":[").nth(1)?.split(']').next()?;
				files
					.split(',')
					.map(|f| f.trim().trim_matches('"').replace("\\\\", "\\"))
					.find(|f| f.ends_with(".dll") || f.ends_with(".dylib") || f.ends_with(".so"))
			})
			.last()
			.map(PathBuf::from)
			.expect("no cdylib among the probe's build artifacts")
	})
}

fn corner_mask(id: u32, inverted: bool, mode: MaskMode) -> MaskPath {
	MaskPath {
		id,
		name: format!("Mask {id}"),
		vertices: vec![
			MaskVertex::corner(4.0, 4.0),
			MaskVertex::corner(40.0, 4.0),
			MaskVertex::corner(40.0, 30.0),
		],
		closed: true,
		inverted,
		mode,
	}
}

/// Render the probe with `masks` on its layer and decode its report.
fn report(masks: Vec<MaskPath>) -> Vec<u16> {
	let mut instance: PluginInstance = aexlo::Host::get().load(probe_path()).expect("loading the probe");
	instance.set_input_layer(Layer::<Depth8>::black(64, 48));
	instance.set_mask_paths(masks);
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

/// What the probe read for mask `i`.
fn mask(slots: &[u16], i: usize) -> [u16; 8] {
	let base = slot::MASK_BASE + i * slot::MASK_STRIDE;
	slots[base..base + slot::MASK_STRIDE].try_into().unwrap()
}

#[test]
fn a_layer_without_masks_has_zero_and_refuses_every_index() {
	let s = report(vec![]);
	assert_eq!(s[slot::ACQUIRE_ERR], 0, "the suite was not served");
	assert_eq!(s[slot::LAYER_ERR], 0);
	assert_eq!(s[slot::NUM_MASKS_ERR], 0);
	assert_eq!(s[slot::NUM_MASKS], 0);
	assert_eq!(s[slot::BY_INDEX_PAST_END_ERR], ERR_INVALID_INDEX);
	assert_eq!(s[slot::BY_INDEX_PAST_END_HANDLE_NULL], 1);
	assert_eq!(s[slot::BY_INDEX_NEGATIVE_ERR], ERR_INVALID_INDEX);
}

#[test]
fn one_mask_is_read_back_with_its_attributes() {
	let s = report(vec![corner_mask(7, true, MaskMode::Subtract)]);
	assert_eq!(s[slot::ACQUIRE_ERR], 0, "the suite was not served");
	assert_eq!(s[slot::NUM_MASKS], 1);
	assert_eq!(s[slot::BY_INDEX_PAST_END_ERR], ERR_INVALID_INDEX);
	assert_eq!(s[slot::BY_INDEX_NEGATIVE_ERR], ERR_INVALID_INDEX);
	// get err, invert, mode (PF_MaskMode_SUBTRACT = 2), id, locked, roto, dispose, double dispose
	assert_eq!(mask(&s, 0), [0, 1, 2, 7, 0, 0, 0, ERR_BAD_CALLBACK_PARAM]);
}

#[test]
fn three_masks_keep_their_own_invert_mode_and_id_in_order() {
	let s = report(vec![
		corner_mask(11, false, MaskMode::Add),
		corner_mask(22, true, MaskMode::Intersect),
		corner_mask(33, false, MaskMode::Difference),
	]);
	assert_eq!(s[slot::ACQUIRE_ERR], 0, "the suite was not served");
	assert_eq!(s[slot::NUM_MASKS], 3);
	// PF_MaskMode: ADD 1, INTERSECT 3, DIFFERENCE 6
	assert_eq!(mask(&s, 0), [0, 0, 1, 11, 0, 0, 0, ERR_BAD_CALLBACK_PARAM]);
	assert_eq!(mask(&s, 1), [0, 1, 3, 22, 0, 0, 0, ERR_BAD_CALLBACK_PARAM]);
	assert_eq!(mask(&s, 2), [0, 0, 6, 33, 0, 0, 0, ERR_BAD_CALLBACK_PARAM]);
	assert_eq!(s[slot::BY_INDEX_PAST_END_ERR], ERR_INVALID_INDEX);
}
