//! A Layer parameter linked to a footage file leads a plugin to that file's
//! path through the AEGP Item and Footage Suites, as in After Effects.
//!
//! `tests/probe_footage` follows the chain
//! (`AEGP_GetNewEffectForEffect` -> `AEGP_GetNewEffectStreamByIndex` ->
//! `AEGP_GetNewStreamValue` -> `AEGP_GetLayerFromLayerID` ->
//! `AEGP_GetLayerSourceItem` -> `AEGP_GetItemType` ->
//! `AEGP_GetMainFootageFromItem` -> `AEGP_GetFootagePath`) and writes what it
//! found into the first row of its output; see its docs for the encoding.
mod common;

use std::path::Path;

/// Stages the probe completes when the whole chain works.
const WHOLE_CHAIN: u8 = 11;
/// Stages completed when the stream value held no layer id (suites, effect,
/// stream, value): the chain stops there, with no error.
const STOPPED_AT_LAYER_ID: u8 = 4;
/// `AEGP_ItemType_FOOTAGE`.
const ITEM_FOOTAGE: u8 = 4;

/// What the probe wrote into its output.
#[derive(Debug)]
struct Found {
	stages: u8,
	err: u8,
	item_type: u8,
	has_layer_id: bool,
	path: Vec<u8>,
}

impl Found {
	fn path(&self) -> String {
		String::from_utf8(self.path.clone()).expect("the probe writes UTF-8")
	}
}

/// Render the probe once with `layers = { Footage = <layer> }` (a TOML inline
/// table, or nothing to leave the parameter alone) and decode its report.
fn probe(name: &str, layer: Option<&str>) -> Found {
	let plugin = common::probe_footage_artifact();
	let dir = common::scratch(name);
	let layers = layer.map_or(String::new(), |spec| format!("layers = {{ Footage = {spec} }}\n"));
	let manifest = common::manifest(
		&dir,
		&format!(
			"[plugin]\nartifact = {}\n[defaults]\nsize = [96, 4]\ninput = {{ generate = 'gradient' }}\n[[preset]]\nname = 'probe'\n{layers}",
			common::toml_path(&plugin)
		),
	);
	let output = dir.join("out.png");
	let result = common::aexlo(
		&[
			"render",
			"--manifest",
			manifest.to_str().unwrap(),
			"--preset",
			"probe",
			"--output",
			output.to_str().unwrap(),
		],
		&dir,
	);
	assert!(
		result.status.success(),
		"{}\n{}",
		String::from_utf8_lossy(&result.stdout),
		String::from_utf8_lossy(&result.stderr)
	);
	let found = decode(&output);
	eprintln!("{name}: {found:?}");
	found
}

fn decode(png: &Path) -> Found {
	let image = image::open(png).unwrap().to_rgba8();
	let bytes: Vec<u8> = image.pixels().take(96).flat_map(|p| [p[0], p[1], p[2]]).collect();
	assert!(image.pixels().take(96).all(|p| p[3] == 255), "the report is opaque");
	let len = bytes[3] as usize | (bytes[4] as usize) << 8;
	Found {
		stages: bytes[0],
		err: bytes[1],
		item_type: bytes[2],
		has_layer_id: bytes[5] != 0,
		path: bytes[6..6 + len].to_vec(),
	}
}

fn toml_str(path: &Path) -> String {
	format!("{{ footage = {} }}", common::toml_path(path))
}

#[test]
fn a_linked_footage_file_reaches_the_plugin_as_its_exact_path() {
	let dir = common::scratch("footage_linked");
	// Spaces, accents, CJK and a character outside the BMP: the path crosses
	// the host as UTF-16 and comes back as UTF-8.
	let folder = dir.join("lut folder \u{e9}\u{3042}\u{1f3a8}");
	std::fs::create_dir_all(&folder).unwrap();
	let file = folder.join("my look \u{fc}.cube");
	std::fs::write(&file, "LUT_3D_SIZE 2\n").unwrap();

	let found = probe("footage_linked_run", Some(&toml_str(&file)));
	assert_eq!(found.stages, WHOLE_CHAIN, "{found:?}");
	assert_eq!(found.err, 0);
	assert_eq!(found.item_type, ITEM_FOOTAGE);
	assert!(found.has_layer_id);
	assert_eq!(found.path(), file.to_str().unwrap());
}

#[test]
fn a_plain_ascii_path_and_a_relative_one_resolve_against_the_manifest() {
	let found = probe("footage_relative", Some("{ footage = 'luts/film.cube' }"));
	assert_eq!(found.stages, WHOLE_CHAIN, "{found:?}");
	let path = found.path();
	assert!(path.ends_with("film.cube"), "{path}");
	assert!(path.contains("luts"), "{path}");
	assert!(Path::new(&path).is_absolute(), "{path}");
}

/// The file need not exist: a plugin's missing-file path is still reachable.
#[test]
fn a_footage_file_that_does_not_exist_still_resolves() {
	let missing = common::scratch("footage_missing_dir").join("nowhere.cube");
	assert!(!missing.exists());
	let found = probe("footage_missing", Some(&toml_str(&missing)));
	assert_eq!(found.stages, WHOLE_CHAIN, "{found:?}");
	assert_eq!(found.path(), missing.to_str().unwrap());
}

/// A Layer parameter nobody linked is "None": no layer id, and the chain
/// stops there without an error.
#[test]
fn an_unlinked_layer_parameter_has_no_footage() {
	let found = probe("footage_unlinked", None);
	assert!(!found.has_layer_id, "{found:?}");
	assert_eq!(found.stages, STOPPED_AT_LAYER_ID, "{found:?}");
	assert_eq!(found.err, 0, "{found:?}");
	assert!(found.path.is_empty());
}

/// A layer parameter given pixels is a layer of the comp, not a footage file:
/// the plugin must be able to tell, and nothing crashes.
#[test]
fn a_pixel_layer_is_not_footage() {
	let found = probe("footage_pixels", Some("{ generate = 'solid' }"));
	assert_eq!(found.stages, STOPPED_AT_LAYER_ID, "{found:?}");
	assert_eq!(found.err, 0, "{found:?}");
	assert!(!found.has_layer_id, "{found:?}");
	assert!(found.path.is_empty(), "{found:?}");
	assert_ne!(found.item_type, ITEM_FOOTAGE, "{found:?}");
}
