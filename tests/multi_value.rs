use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use lofty::prelude::ItemKey;
use taguar::{read_values_from_path, write_values_to_path};

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
    "taguar-multi-{}-{n}-{}.{ext}",
    std::process::id(),
    stem,
  ));
  std::fs::copy(&src, &dest).expect("copy fixture");
  dest
}

#[test]
fn mp3_round_trips_multiple_artists() {
  // The ID3v2.4 multi-value path: two TPE1 values joined by `\0` on disk.
  let path = copy_fixture("silence.mp3");
  let artists = vec!["Daft Punk".to_string(), "Pharrell Williams".to_string()];
  write_values_to_path(&path, ItemKey::TrackArtist, &artists).unwrap();
  assert_eq!(
    read_values_from_path(&path, ItemKey::TrackArtist).unwrap(),
    artists,
  );
}

#[test]
fn mp3_round_trips_multiple_genres() {
  let path = copy_fixture("silence.mp3");
  let genres = vec!["Electronic".to_string(), "House".to_string()];
  write_values_to_path(&path, ItemKey::Genre, &genres).unwrap();
  assert_eq!(
    read_values_from_path(&path, ItemKey::Genre).unwrap(),
    genres,
  );
}

#[test]
fn slash_in_artist_name_is_preserved_as_one_value() {
  // Unlike genres, artist values must not be split on `/` (e.g. AC/DC).
  let path = copy_fixture("silence.mp3");
  let artists = vec!["AC/DC".to_string()];
  write_values_to_path(&path, ItemKey::TrackArtist, &artists).unwrap();
  assert_eq!(
    read_values_from_path(&path, ItemKey::TrackArtist).unwrap(),
    artists,
  );
}

#[test]
fn empty_values_are_dropped_on_save() {
  let path = copy_fixture("silence.mp3");
  write_values_to_path(
    &path,
    ItemKey::TrackArtist,
    &["keep".to_string(), String::new(), "also".to_string()],
  )
  .unwrap();
  assert_eq!(
    read_values_from_path(&path, ItemKey::TrackArtist).unwrap(),
    vec!["keep".to_string(), "also".to_string()],
  );
}

#[test]
fn flac_round_trips_multiple_artists() {
  let path = copy_fixture("silence.flac");
  let artists = vec!["A".to_string(), "B".to_string(), "C".to_string()];
  write_values_to_path(&path, ItemKey::TrackArtist, &artists).unwrap();
  assert_eq!(
    read_values_from_path(&path, ItemKey::TrackArtist).unwrap(),
    artists,
  );
}
