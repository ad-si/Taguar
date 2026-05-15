use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use taguar::{read_descriptions_from_path, write_descriptions_to_path};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Copies a fixture into a unique temp path so tests can mutate it
/// independently and in parallel.
fn copy_fixture(name: &str) -> PathBuf {
  let src = Path::new(env!("CARGO_MANIFEST_DIR"))
    .join("tests")
    .join("fixtures")
    .join(name);
  let n = COUNTER.fetch_add(1, Ordering::Relaxed);
  let stem = Path::new(name).file_stem().unwrap().to_string_lossy();
  let ext = Path::new(name).extension().unwrap().to_string_lossy();
  let dest = std::env::temp_dir().join(format!(
    "taguar-test-{}-{n}-{}.{ext}",
    std::process::id(),
    stem,
  ));
  std::fs::copy(&src, &dest).expect("copy fixture");
  dest
}

#[test]
fn vorbis_round_trips_multiple_descriptions() {
  let path = copy_fixture("silence.ogg");
  let descs =
    vec!["alpha".to_string(), "beta".to_string(), "gamma".to_string()];
  write_descriptions_to_path(&path, &descs).unwrap();
  assert_eq!(read_descriptions_from_path(&path).unwrap(), descs);
}

#[test]
fn flac_round_trips_multiple_descriptions() {
  let path = copy_fixture("silence.flac");
  let descs = vec!["one".to_string(), "two".to_string()];
  write_descriptions_to_path(&path, &descs).unwrap();
  assert_eq!(read_descriptions_from_path(&path).unwrap(), descs);
}

#[test]
fn m4a_round_trips_single_description() {
  let path = copy_fixture("silence.m4a");
  let descs = vec!["only one".to_string()];
  write_descriptions_to_path(&path, &descs).unwrap();
  assert_eq!(read_descriptions_from_path(&path).unwrap(), descs);
}

#[test]
fn empty_entries_are_dropped_on_save() {
  let path = copy_fixture("silence.ogg");
  write_descriptions_to_path(
    &path,
    &[
      "keep".to_string(),
      String::new(),
      "also keep".to_string(),
      String::new(),
    ],
  )
  .unwrap();
  assert_eq!(
    read_descriptions_from_path(&path).unwrap(),
    vec!["keep".to_string(), "also keep".to_string()],
  );
}

#[test]
fn clearing_all_descriptions_removes_the_field() {
  let path = copy_fixture("silence.ogg");
  write_descriptions_to_path(
    &path,
    &["initial".to_string(), "values".to_string()],
  )
  .unwrap();
  write_descriptions_to_path(&path, &[String::new(), String::new()]).unwrap();
  // After clearing, the Vorbis placeholder semantics in `read_descriptions`
  // kick in: one empty entry so the UI shows an editor.
  assert_eq!(
    read_descriptions_from_path(&path).unwrap(),
    vec![String::new()],
  );
}

#[test]
fn overwriting_replaces_existing_descriptions() {
  let path = copy_fixture("silence.ogg");
  write_descriptions_to_path(
    &path,
    &["first set".to_string(), "extra".to_string()],
  )
  .unwrap();
  write_descriptions_to_path(&path, &["second set".to_string()]).unwrap();
  assert_eq!(
    read_descriptions_from_path(&path).unwrap(),
    vec!["second set".to_string()],
  );
}

#[test]
fn mp3_id3v2_silently_drops_descriptions() {
  // ID3v2 has no `Description` mapping, so writes are dropped — verify the
  // call still succeeds and reading returns nothing.
  let path = copy_fixture("silence.mp3");
  write_descriptions_to_path(&path, &["ignored".to_string()]).unwrap();
  assert!(read_descriptions_from_path(&path).unwrap().is_empty());
}
