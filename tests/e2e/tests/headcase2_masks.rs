//! Headcase 2 (VFX Tools) reads its layer's mask outlines in
//! `PF_Cmd_SMART_PRE_RENDER` (`AEExtractEffectLayerMasks`: Layer Mask Suite,
//! Stream Suite mask outline stream, Mask Outline Suite). This test attaches
//! masks with `set_mask_paths`, runs a smart render, and checks from the
//! suite-call trace that the plugin walked every mask and received exactly the
//! vertices that were set.
//!
//! Needs the plugin: `fixtures/plugins/<platform>/Headcase2.<ext>`, or the
//! path in `AEXLO_HEADCASE2`. Skips when neither exists.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use aexlo::{
	Depth8, Layer, MaskMode, MaskPath, MaskVertex, ObserveLevel, Observer, RenderPath, SuiteCallEvent, UnsupportedEvent,
};

#[derive(Default)]
struct Trace {
	calls: Mutex<Vec<SuiteCallEvent>>,
	unsupported: Mutex<Vec<UnsupportedEvent>>,
	/// What the plugin mixed into the frame's cache key during pre-render.
	guid_mix: Mutex<Vec<u8>>,
}

impl Observer for Trace {
	fn suite_call(&self, event: &SuiteCallEvent) {
		self.calls.lock().unwrap().push(event.clone());
	}
	fn unsupported(&self, event: &UnsupportedEvent) {
		self.unsupported.lock().unwrap().push(event.clone());
	}
}

fn plugin() -> Option<PathBuf> {
	std::env::var_os("AEXLO_HEADCASE2")
		.map(PathBuf::from)
		.filter(|p| p.exists())
		.or_else(|| test_e2e::fixture("Headcase2"))
}

fn masks() -> Vec<MaskPath> {
	vec![
		MaskPath {
			id: 3,
			name: "curved".into(),
			vertices: vec![
				MaskVertex {
					x: 100.5,
					y: 80.25,
					tan_in_x: -20.0,
					tan_in_y: 15.0,
					tan_out_x: 20.0,
					tan_out_y: -15.0,
				},
				MaskVertex::corner(400.0, 60.0),
				MaskVertex {
					x: 330.0,
					y: 300.75,
					tan_in_x: 40.5,
					tan_in_y: 0.0,
					tan_out_x: -40.5,
					tan_out_y: 0.0,
				},
			],
			closed: true,
			inverted: true,
			mode: MaskMode::Subtract,
		},
		MaskPath {
			id: 9,
			name: "square".into(),
			vertices: vec![
				MaskVertex::corner(10.0, 10.0),
				MaskVertex::corner(200.0, 10.0),
				MaskVertex::corner(200.0, 150.0),
				MaskVertex::corner(10.0, 150.0),
			],
			closed: true,
			inverted: false,
			mode: MaskMode::Add,
		},
	]
}

/// Smart-render Headcase 2 with `masks` on its layer and return the trace.
fn run(masks: Vec<MaskPath>) -> Option<Trace> {
	let path = plugin()?;
	let mut instance = aexlo::Host::get().load(&path).expect("loading Headcase 2");
	let trace = Arc::new(Trace::default());
	instance.set_observer(Some(trace.clone()), ObserveLevel::Arguments);
	instance.set_input_layer(Layer::<Depth8>::black(640, 360));
	instance.set_mask_paths(masks);
	instance.render_with(RenderPath::Smart).expect("smart render failed");
	instance.set_observer(None, ObserveLevel::Calls);
	let trace = Arc::try_unwrap(trace).ok().expect("observer still shared");
	*trace.guid_mix.lock().unwrap() = instance.pre_render_guid_mix().to_vec();
	Some(trace)
}

fn calls(trace: &Trace, function: &str) -> Vec<SuiteCallEvent> {
	trace
		.calls
		.lock()
		.unwrap()
		.iter()
		.filter(|c| c.function == function)
		.cloned()
		.collect()
}

fn arg<'a>(call: &'a SuiteCallEvent, name: &str) -> &'a str {
	call.args
		.iter()
		.find(|(n, _)| *n == name)
		.map(|(_, v)| v.as_str())
		.unwrap_or_else(|| panic!("{} has no argument {name}: {:?}", call.function, call.args))
}

#[test]
fn headcase2_walks_every_mask_and_receives_the_attached_vertices() {
	let Some(trace) = run(masks()) else {
		eprintln!("skipping: Headcase 2 is not present (set AEXLO_HEADCASE2)");
		return;
	};

	// It asked how many masks, and was told 2.
	assert!(!calls(&trace, "AEGP_GetLayerNumMasks").is_empty());
	// It opened mask 0 and mask 1, read mode and invert, and asked for the outline stream.
	let by_index = calls(&trace, "AEGP_GetLayerMaskByIndex");
	let indices: Vec<&str> = by_index.iter().map(|c| arg(c, "mask_indexL")).collect();
	assert_eq!(indices, ["0", "1"], "{by_index:?}");
	assert_eq!(calls(&trace, "AEGP_GetMaskMode").len(), 2);
	assert_eq!(calls(&trace, "AEGP_GetMaskInvert").len(), 2);
	assert_eq!(
		calls(&trace, "AEGP_GetNewMaskStream").len(),
		2 + 2 * 3,
		"outline plus opacity/feather/expansion per mask"
	);

	// Segments are walked as vertices `i` and `i + 1` (wrapping), per mask.
	let vertices: Vec<String> = calls(&trace, "AEGP_GetMaskOutlineVertexInfo")
		.iter()
		.map(|c| c.args.iter().find(|(n, _)| *n == "vertexP (out)").unwrap().1.clone())
		.collect();
	let want = |v: &MaskVertex| {
		format!(
			"x={} y={} tan_in=({}, {}) tan_out=({}, {})",
			v.x, v.y, v.tan_in_x, v.tan_in_y, v.tan_out_x, v.tan_out_y
		)
	};
	let mut expected = Vec::new();
	for mask in masks() {
		let n = mask.vertices.len();
		for seg in 0..n {
			expected.push(want(&mask.vertices[seg]));
			expected.push(want(&mask.vertices[(seg + 1) % n]));
		}
	}
	assert_eq!(vertices, expected);

	// What the plugin does with them: it flattens each outline to 8 samples per
	// segment, hashes the mode, invert and every sample, and mixes that into the
	// frame's cache key. Recompute that hash here, from the masks that were
	// attached, and compare it with the bytes the plugin handed to `GuidMixInPtr`.
	let mix = expected_cache_key(&masks());
	assert_eq!(
		*trace.guid_mix.lock().unwrap(),
		mix.to_le_bytes(),
		"cache key from the extracted vertices"
	);

	// Opacity, feather and expansion are not in the mask model: refused, and
	// reported, not invented. (The plugin then uses its defaults 100 / 0 / 0.)
	let declined: Vec<String> = trace
		.unsupported
		.lock()
		.unwrap()
		.iter()
		.filter(|u| u.name == "AEGP_StreamSuite/AEGP_GetNewMaskStream")
		.map(|u| u.detail.clone())
		.collect();
	assert_eq!(declined.len(), 6, "{declined:?}");
}

/// `hashPerSourceMaskSet` (examples/_projection_core/ae_mask.cpp), fed by the
/// extraction in the same file, for masks of a layer whose source item aexlo
/// does not serve (width = height = 0, slot 0), opacity 100, no feather or
/// expansion.
fn expected_cache_key(masks: &[MaskPath]) -> u32 {
	let mut h: u32 = 2166136261;
	let mut mix = |v: u32| {
		h ^= v;
		h = h.wrapping_mul(16777619);
	};
	let mixf = |mix: &mut dyn FnMut(u32), f: f32| mix((f64::from(f) * 16.0).round() as i64 as i32 as u32);
	mix(0);
	mix(0);
	mix(0);
	for m in masks {
		mix(m.mode as u32);
		mix(m.inverted as u32);
		mixf(&mut mix, (100.0f32 * 0.01f32) * 1000.0f32);
		mixf(&mut mix, 0.0);
		mixf(&mut mix, 0.0);
		let n = m.vertices.len();
		let mut points = Vec::new();
		for seg in 0..n {
			let (a, b) = (&m.vertices[seg], &m.vertices[(seg + 1) % n]);
			let p = [
				(a.x, a.y),
				(a.x + a.tan_out_x, a.y + a.tan_out_y),
				(b.x + b.tan_in_x, b.y + b.tan_in_y),
				(b.x, b.y),
			];
			for k in 0..8 {
				let t = f64::from(k) / 8.0;
				let u = 1.0 - t;
				let (c0, c1, c2, c3) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
				points.push((
					(c0 * p[0].0 + c1 * p[1].0 + c2 * p[2].0 + c3 * p[3].0) as f32,
					(c0 * p[0].1 + c1 * p[1].1 + c2 * p[2].1 + c3 * p[3].1) as f32,
				));
			}
		}
		mix(points.len() as u32);
		for (x, y) in points {
			mixf(&mut mix, x);
			mixf(&mut mix, y);
		}
	}
	h
}

#[test]
fn headcase2_without_masks_extracts_nothing() {
	let Some(trace) = run(vec![]) else {
		eprintln!("skipping: Headcase 2 is not present (set AEXLO_HEADCASE2)");
		return;
	};
	assert!(!calls(&trace, "AEGP_GetLayerNumMasks").is_empty());
	assert!(calls(&trace, "AEGP_GetLayerMaskByIndex").is_empty());
	assert!(calls(&trace, "AEGP_GetMaskOutlineVertexInfo").is_empty());
}
