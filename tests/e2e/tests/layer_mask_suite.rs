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
	pub const OUTLINE_BASE: usize = MASK_BASE + MASK_STRIDE * 3;
	pub const OUTLINE_STRIDE: usize = 40;
	pub const O_STREAM_ERR: usize = 0;
	pub const O_OPACITY_STREAM_ERR: usize = 1;
	pub const O_STREAM_TYPE: usize = 2;
	pub const O_VALUE_ERR: usize = 3;
	pub const O_OPEN: usize = 4;
	pub const O_NUM_SEGMENTS: usize = 5;
	pub const O_NUM_FEATHERS: usize = 6;
	pub const O_PAST_END_ERR: usize = 7;
	pub const O_DISPOSE_VALUE_ERR: usize = 8;
	pub const O_DISPOSE_STREAM_ERR: usize = 9;
	pub const O_VERTICES: usize = 10;
	pub const VERTEX_STRIDE: usize = 6;
	pub const TOTAL: usize = OUTLINE_BASE + OUTLINE_STRIDE * 3;
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
			.next_back()
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

/// The outline group the probe wrote for mask `i`.
fn outline(slots: &[u16], i: usize) -> &[u16] {
	let base = slot::OUTLINE_BASE + i * slot::OUTLINE_STRIDE;
	&slots[base..base + slot::OUTLINE_STRIDE]
}

/// Vertex `v` of an outline group: `(error, x, y, tan_in_x, tan_out_x, tan_out_y)`.
fn vertex(outline: &[u16], v: usize) -> (u16, f64, f64, f64, f64, f64) {
	let base = slot::O_VERTICES + v * slot::VERTEX_STRIDE;
	let px = |i: usize| (f64::from(outline[base + i]) - f64::from(0x8000)) / 16.0;
	(outline[base], px(1), px(2), px(3), px(4), px(5))
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

/// A closed mask with real tangents, and an open one, travel through
/// `AEGP_GetNewMaskStream` -> `AEGP_GetNewStreamValue` -> the Mask Outline Suite
/// and arrive with the exact vertices that were attached.
#[test]
fn outlines_arrive_with_the_vertices_that_were_attached() {
	let curved = MaskPath {
		id: 1,
		vertices: vec![
			MaskVertex {
				x: 10.5,
				y: 20.25,
				tan_in_x: -4.0,
				tan_in_y: 6.0,
				tan_out_x: 4.0,
				tan_out_y: -6.0,
			},
			MaskVertex::corner(50.0, 12.0),
			MaskVertex {
				x: 33.0,
				y: 44.75,
				tan_in_x: 7.5,
				tan_in_y: 0.0,
				tan_out_x: -7.5,
				tan_out_y: 0.0,
			},
		],
		closed: true,
		..Default::default()
	};
	let open = MaskPath {
		id: 2,
		vertices: vec![MaskVertex::corner(1.0, 2.0), MaskVertex::corner(60.0, 40.0)],
		closed: false,
		..Default::default()
	};
	let s = report(vec![curved.clone(), open]);

	let o = outline(&s, 0);
	assert_eq!(o[slot::O_STREAM_ERR], 0);
	assert_eq!(o[slot::O_STREAM_TYPE], 11, "AEGP_StreamType_MASK");
	assert_eq!(o[slot::O_VALUE_ERR], 0);
	assert_eq!(o[slot::O_OPEN], 0);
	assert_eq!(o[slot::O_NUM_SEGMENTS], 3);
	assert_eq!(o[slot::O_NUM_FEATHERS], 0);
	for (v, want) in curved.vertices.iter().enumerate() {
		assert_eq!(
			vertex(o, v),
			(0, want.x, want.y, want.tan_in_x, want.tan_out_x, want.tan_out_y),
			"vertex {v}"
		);
	}
	// Segment 2 ends at vertex 3, which wraps to vertex 0.
	assert_eq!(vertex(o, 3).1, 10.5);
	assert_eq!(vertex(o, 3).2, 20.25);
	assert_eq!(o[slot::O_PAST_END_ERR], ERR_INVALID_INDEX);
	assert_eq!((o[slot::O_DISPOSE_VALUE_ERR], o[slot::O_DISPOSE_STREAM_ERR]), (0, 0));

	let o = outline(&s, 1);
	assert_eq!(o[slot::O_OPEN], 1);
	assert_eq!(o[slot::O_NUM_SEGMENTS], 1);
	assert_eq!(vertex(o, 0), (0, 1.0, 2.0, 0.0, 0.0, 0.0));
	assert_eq!(vertex(o, 1), (0, 60.0, 40.0, 0.0, 0.0, 0.0));
}

/// A mask stream aexlo cannot answer from its mask model is refused (and
/// reported as unsupported), not answered with an invented value.
#[test]
fn mask_streams_the_model_does_not_hold_are_refused() {
	let s = report(vec![corner_mask(1, false, MaskMode::Add)]);
	let o = outline(&s, 0);
	assert_eq!(o[slot::O_STREAM_ERR], 0);
	assert_ne!(o[slot::O_OPACITY_STREAM_ERR], 0);
}
