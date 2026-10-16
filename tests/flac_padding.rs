use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use lofty::config::WriteOptions;
use lofty::file::TaggedFileExt;
use lofty::ogg::tag::VorbisComments;
use lofty::ogg::OggPictureStorage;
use lofty::picture::{MimeType, Picture, PictureInformation, PictureType};
use lofty::prelude::{AudioFile, ItemKey};
use lofty::tag::{Tag, TagType};
use taguar::tag_io::{remove_tag, save_tag};

const PADDING: u32 = WriteOptions::DEFAULT_PREFERRED_PADDING;

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Copies the FLAC fixture (STREAMINFO, VORBIS_COMMENT, 8 KiB PADDING) to a
/// unique temp path, optionally prefixed with `prefix`.
fn temp_flac(prefix: &[u8]) -> PathBuf {
  let src = Path::new(env!("CARGO_MANIFEST_DIR"))
    .join("tests")
    .join("fixtures")
    .join("silence.flac");
  let mut bytes = prefix.to_vec();
  bytes.extend(std::fs::read(src).expect("read fixture"));

  let n = COUNTER.fetch_add(1, Ordering::Relaxed);
  let dest = std::env::temp_dir()
    .join(format!("taguar-padding-{}-{n}.flac", std::process::id()));
  std::fs::write(&dest, bytes).expect("write fixture");
  dest
}

/// The metadata blocks of the FLAC file at `path` as (type, content length),
/// and the offset at which its audio frames start.
fn blocks(path: &Path) -> (Vec<(u8, usize)>, usize) {
  let bytes = std::fs::read(path).unwrap();
  let mut pos = bytes.windows(4).position(|w| w == b"fLaC").unwrap() + 4;
  let mut blocks = Vec::new();
  loop {
    let header = &bytes[pos..pos + 4];
    let size = u32::from_be_bytes([0, header[1], header[2], header[3]]);
    blocks.push((header[0] & 0x7F, size as usize));
    pos += 4 + size as usize;
    if header[0] & 0x80 != 0 {
      return (blocks, pos);
    }
  }
}

/// The audio frames of the FLAC file at `path`.
fn audio(path: &Path) -> Vec<u8> {
  let (_, start) = blocks(path);
  std::fs::read(path).unwrap()[start..].to_vec()
}

fn title(path: &Path) -> Option<String> {
  let tagged = lofty::read_from_path(path).unwrap();
  tagged
    .tag(TagType::VorbisComments)?
    .get_string(ItemKey::TrackTitle)
    .map(str::to_string)
}

fn comments(title: &str) -> VorbisComments {
  let mut vc = VorbisComments::default();
  vc.push("TITLE".to_string(), title.to_string());
  vc
}

fn cover(size: usize) -> Picture {
  Picture::unchecked(vec![0xAB; size])
    .pic_type(PictureType::CoverFront)
    .mime_type(MimeType::Png)
    .build()
}

#[test]
fn edits_keep_the_audio_in_place() {
  let path = temp_flac(&[]);
  let size = std::fs::metadata(&path).unwrap().len();
  let audio_before = audio(&path);
  let (_, audio_start) = blocks(&path);

  save_tag(&comments("A much longer title than before"), &path, PADDING)
    .unwrap();
  assert_eq!(
    title(&path).as_deref(),
    Some("A much longer title than before")
  );
  save_tag(&comments("Short"), &path, PADDING).unwrap();
  assert_eq!(title(&path).as_deref(), Some("Short"));

  let mut vc = comments("With cover");
  vc.insert_picture(cover(1000), Some(PictureInformation::default()))
    .unwrap();
  save_tag(&vc, &path, PADDING).unwrap();

  remove_tag(&VorbisComments::default(), &path, PADDING).unwrap();
  assert_eq!(title(&path), None);

  assert_eq!(std::fs::metadata(&path).unwrap().len(), size);
  assert_eq!(blocks(&path).1, audio_start);
  assert_eq!(audio(&path), audio_before);
  let tagged = lofty::read_from_path(&path).unwrap();
  assert!(tagged.properties().duration().as_millis() > 0);

  std::fs::remove_file(&path).unwrap();
}

#[test]
fn deleting_through_the_generic_tag_keeps_the_audio_in_place() {
  let path = temp_flac(&[]);
  save_tag(&comments("Title"), &path, PADDING).unwrap();
  let size = std::fs::metadata(&path).unwrap().len();

  remove_tag(&Tag::new(TagType::VorbisComments), &path, PADDING).unwrap();

  assert_eq!(title(&path), None);
  assert_eq!(std::fs::metadata(&path).unwrap().len(), size);

  std::fs::remove_file(&path).unwrap();
}

#[test]
fn metadata_outgrowing_the_padding_gets_fresh_padding() {
  let path = temp_flac(&[]);
  let audio_before = audio(&path);

  let mut vc = comments("Big cover");
  vc.insert_picture(cover(20_000), Some(PictureInformation::default()))
    .unwrap();
  save_tag(&vc, &path, PADDING).unwrap();

  let (blocks, _) = blocks(&path);
  assert_eq!(blocks.last(), Some(&(1, PADDING as usize)));
  assert_eq!(audio(&path), audio_before);
  assert_eq!(title(&path).as_deref(), Some("Big cover"));

  std::fs::remove_file(&path).unwrap();
}

#[test]
fn zero_padding_deletes_for_real() {
  let path = temp_flac(&[]);
  let audio_before = audio(&path);

  remove_tag(&VorbisComments::default(), &path, 0).unwrap();

  let (blocks, audio_start) = blocks(&path);
  // Only STREAMINFO is left.
  assert_eq!(blocks.iter().map(|(ty, _)| *ty).collect::<Vec<_>>(), [0]);
  assert_eq!(
    std::fs::metadata(&path).unwrap().len() as usize,
    audio_start + audio_before.len(),
  );
  assert_eq!(audio(&path), audio_before);

  std::fs::remove_file(&path).unwrap();
}

#[test]
fn leading_id3v2_tag_is_kept() {
  // ID3v2.4 header with a single TIT2 frame ("Old").
  let mut id3 = Vec::new();
  id3.extend_from_slice(b"ID3");
  id3.extend_from_slice(&[4, 0, 0, 0, 0, 0, 14]);
  id3.extend_from_slice(b"TIT2");
  id3.extend_from_slice(&[0, 0, 0, 4, 0, 0, 3]);
  id3.extend_from_slice(b"Old");

  let path = temp_flac(&id3);
  let size = std::fs::metadata(&path).unwrap().len();

  save_tag(&comments("New"), &path, PADDING).unwrap();

  assert_eq!(std::fs::metadata(&path).unwrap().len(), size);
  assert!(std::fs::read(&path).unwrap().starts_with(&id3));
  assert_eq!(title(&path).as_deref(), Some("New"));

  std::fs::remove_file(&path).unwrap();
}
