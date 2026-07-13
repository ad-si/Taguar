use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use lofty::file::TaggedFileExt;
use lofty::prelude::ItemKey;
use lofty::tag::TagType;
use taguar::{editable_tag, read_values_from_path, write_values_to_path};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Copies the FLAC fixture into a unique temp path, prepending a minimal
/// ID3v2.4 tag (one TIT2 frame). Some tools write ID3v2 to FLAC files even
/// though the format's native tag is Vorbis Comments.
fn flac_with_id3v2() -> PathBuf {
  let src = Path::new(env!("CARGO_MANIFEST_DIR"))
    .join("tests")
    .join("fixtures")
    .join("silence.flac");
  let flac_bytes = std::fs::read(src).expect("read fixture");

  // TIT2 frame: id + syncsafe size + flags + (utf8 encoding byte + "Old").
  let mut frame = Vec::new();
  frame.extend_from_slice(b"TIT2");
  frame.extend_from_slice(&[0, 0, 0, 4]);
  frame.extend_from_slice(&[0, 0]);
  frame.extend_from_slice(&[3]);
  frame.extend_from_slice(b"Old");

  // ID3v2.4 header: magic + version + flags + syncsafe tag size.
  let mut bytes = Vec::new();
  bytes.extend_from_slice(b"ID3");
  bytes.extend_from_slice(&[4, 0, 0]);
  bytes.extend_from_slice(&[0, 0, 0, frame.len() as u8]);
  bytes.extend_from_slice(&frame);
  bytes.extend_from_slice(&flac_bytes);

  let n = COUNTER.fetch_add(1, Ordering::Relaxed);
  let dest = std::env::temp_dir()
    .join(format!("taguar-id3flac-{}-{n}.flac", std::process::id(),));
  std::fs::write(&dest, bytes).expect("write fixture");
  dest
}

#[test]
fn flac_with_id3v2_edits_target_vorbis_comments() {
  let path = flac_with_id3v2();

  let tagged = lofty::read_from_path(&path).unwrap();
  assert!(
    tagged.tag(TagType::Id3v2).is_some(),
    "fixture should carry an ID3v2 tag",
  );
  assert_eq!(
    editable_tag(&tagged).map(|t| t.tag_type()),
    Some(TagType::VorbisComments),
  );
}

#[test]
fn flac_with_id3v2_round_trips_values() {
  // Writing ID3v2 to FLAC is unsupported by lofty, so saving must go to the
  // Vorbis Comments tag even when an ID3v2 tag is present.
  let path = flac_with_id3v2();
  let artists = vec!["New Artist".to_string()];
  write_values_to_path(&path, ItemKey::TrackArtist, &artists).unwrap();
  assert_eq!(
    read_values_from_path(&path, ItemKey::TrackArtist).unwrap(),
    artists,
  );
}
