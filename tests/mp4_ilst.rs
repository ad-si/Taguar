use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use lofty::config::WriteOptions;
use lofty::file::TaggedFileExt;
use lofty::mp4::{Atom, AtomData, AtomIdent, Ilst};
use lofty::prelude::ItemKey;
use lofty::tag::{Tag, TagType};
use taguar::tag_io::{remove_tag, save_tag};

const PADDING: u32 = WriteOptions::DEFAULT_PREFERRED_PADDING;

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn temp_m4a() -> PathBuf {
  let src = Path::new(env!("CARGO_MANIFEST_DIR"))
    .join("tests")
    .join("fixtures")
    .join("silence.m4a");
  let n = COUNTER.fetch_add(1, Ordering::Relaxed);
  let dest = std::env::temp_dir()
    .join(format!("taguar-ilst-{}-{n}.m4a", std::process::id()));
  std::fs::copy(src, &dest).expect("copy fixture");
  dest
}

/// (start, len, kind) of the atoms between `start` and `end` of `bytes`.
fn children(
  bytes: &[u8],
  start: usize,
  end: usize,
) -> Vec<(usize, usize, String)> {
  let mut atoms = Vec::new();
  let mut pos = start;
  while pos + 8 <= end {
    let len =
      u32::from_be_bytes(bytes[pos..pos + 4].try_into().unwrap()) as usize;
    let kind = String::from_utf8_lossy(&bytes[pos + 4..pos + 8]).into_owned();
    atoms.push((pos, len, kind));
    pos += len;
  }
  atoms
}

fn child<'a>(
  atoms: &'a [(usize, usize, String)],
  kind: &str,
) -> Option<&'a (usize, usize, String)> {
  atoms.iter().find(|(_, _, k)| k == kind)
}

/// Kinds of the children of `moov.udta.meta` (an ISO full box), or `None`
/// without a `meta` atom.
fn meta_children(path: &Path) -> Option<Vec<String>> {
  let bytes = std::fs::read(path).unwrap();
  let top = children(&bytes, 0, bytes.len());
  let &(moov, moov_len, _) = child(&top, "moov")?;
  let moov_children = children(&bytes, moov + 8, moov + moov_len);
  let &(udta, udta_len, _) = child(&moov_children, "udta")?;
  let udta_children = children(&bytes, udta + 8, udta + udta_len);
  let &(meta, meta_len, _) = child(&udta_children, "meta")?;
  Some(
    children(&bytes, meta + 12, meta + meta_len)
      .into_iter()
      .map(|(_, _, kind)| kind)
      .collect(),
  )
}

/// Renames the first atom of `kind` inside `meta` to `new_kind`.
fn rename_meta_child(path: &Path, kind: &str, new_kind: &[u8; 4]) {
  let mut bytes = std::fs::read(path).unwrap();
  let pos = bytes
    .windows(4)
    .position(|w| w == kind.as_bytes())
    .unwrap_or_else(|| panic!("no {kind} atom"));
  bytes[pos..pos + 4].copy_from_slice(new_kind);
  std::fs::write(path, bytes).unwrap();
}

fn titled(title: &str) -> Ilst {
  let mut ilst = Ilst::new();
  ilst.insert(Atom::new(
    AtomIdent::Fourcc(*b"\xA9nam"),
    AtomData::UTF8(title.to_string()),
  ));
  ilst
}

fn title(path: &Path) -> Option<String> {
  lofty::read_from_path(path)
    .unwrap()
    .tag(TagType::Mp4Ilst)?
    .get_string(ItemKey::TrackTitle)
    .map(str::to_string)
}

fn size(path: &Path) -> u64 {
  std::fs::metadata(path).unwrap().len()
}

#[test]
fn removing_and_rewriting_keeps_the_hdlr() {
  let path = temp_m4a();
  save_tag(&titled("First"), &path, PADDING).unwrap();
  let len = size(&path);

  remove_tag(&Ilst::new(), &path, PADDING).unwrap();
  assert_eq!(title(&path), None);
  assert_eq!(meta_children(&path).unwrap()[..3], ["hdlr", "ilst", "free"]);
  assert_eq!(size(&path), len);

  save_tag(&titled("Second"), &path, PADDING).unwrap();
  assert_eq!(title(&path).as_deref(), Some("Second"));
  assert_eq!(meta_children(&path).unwrap()[..2], ["hdlr", "ilst"]);
  assert_eq!(size(&path), len);

  std::fs::remove_file(&path).unwrap();
}

#[test]
fn saving_an_empty_generic_tag_keeps_the_hdlr() {
  let path = temp_m4a();
  save_tag(&titled("Title"), &path, PADDING).unwrap();

  save_tag(&Tag::new(TagType::Mp4Ilst), &path, PADDING).unwrap();

  assert_eq!(title(&path), None);
  assert_eq!(meta_children(&path).unwrap()[..2], ["hdlr", "ilst"]);

  std::fs::remove_file(&path).unwrap();
}

#[test]
fn repairs_a_meta_lofty_stripped_the_ilst_from() {
  let path = temp_m4a();
  save_tag(&titled("First"), &path, PADDING).unwrap();
  let mut file = std::fs::OpenOptions::new()
    .read(true)
    .write(true)
    .open(&path)
    .unwrap();
  TagType::Mp4Ilst
    .remove_from(&mut file, WriteOptions::default())
    .unwrap();
  drop(file);
  assert!(!meta_children(&path).unwrap().contains(&"ilst".to_string()));

  save_tag(&titled("Second"), &path, PADDING).unwrap();

  assert_eq!(title(&path).as_deref(), Some("Second"));
  assert_eq!(meta_children(&path).unwrap()[..2], ["hdlr", "ilst"]);

  std::fs::remove_file(&path).unwrap();
}

#[test]
fn carves_the_ilst_out_of_existing_padding() {
  let path = temp_m4a();
  save_tag(&titled("First"), &path, PADDING).unwrap();
  remove_tag(&Ilst::new(), &path, PADDING).unwrap();
  rename_meta_child(&path, "ilst", b"free");
  assert_eq!(meta_children(&path).unwrap()[..2], ["hdlr", "free"]);
  let len = size(&path);

  save_tag(&titled("Second"), &path, PADDING).unwrap();

  assert_eq!(title(&path).as_deref(), Some("Second"));
  assert_eq!(meta_children(&path).unwrap()[..2], ["hdlr", "ilst"]);
  assert_eq!(size(&path), len);

  std::fs::remove_file(&path).unwrap();
}

#[test]
fn refuses_to_overwrite_unknown_meta_atoms() {
  let path = temp_m4a();
  save_tag(&titled("First"), &path, PADDING).unwrap();
  let mut file = std::fs::OpenOptions::new()
    .read(true)
    .write(true)
    .open(&path)
    .unwrap();
  TagType::Mp4Ilst
    .remove_from(&mut file, WriteOptions::default())
    .unwrap();
  drop(file);
  // Leave `meta` without padding the `ilst` could be carved out of.
  rename_meta_child(&path, "hdlr", b"keys");
  while meta_children(&path).unwrap().contains(&"free".to_string()) {
    rename_meta_child(&path, "free", b"xtra");
  }
  let before = std::fs::read(&path).unwrap();

  assert!(save_tag(&titled("Second"), &path, PADDING).is_err());
  assert_eq!(std::fs::read(&path).unwrap(), before);

  std::fs::remove_file(&path).unwrap();
}

#[test]
fn replaces_a_meta_without_room_for_an_ilst() {
  let path = temp_m4a();
  save_tag(&titled("First"), &path, PADDING).unwrap();
  let mut file = std::fs::OpenOptions::new()
    .read(true)
    .write(true)
    .open(&path)
    .unwrap();
  TagType::Mp4Ilst
    .remove_from(&mut file, WriteOptions::default())
    .unwrap();
  drop(file);
  // Leave `meta` without padding the `ilst` could be carved out of.
  while meta_children(&path).unwrap().contains(&"free".to_string()) {
    rename_meta_child(&path, "free", b"hdlr");
  }

  save_tag(&titled("Second"), &path, PADDING).unwrap();

  assert_eq!(title(&path).as_deref(), Some("Second"));
  assert_eq!(meta_children(&path).unwrap()[..2], ["hdlr", "ilst"]);

  std::fs::remove_file(&path).unwrap();
}
