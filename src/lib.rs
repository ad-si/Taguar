use lofty::config::WriteOptions;
use lofty::file::{TaggedFile, TaggedFileExt};
use lofty::prelude::{ItemKey, TagExt};
use lofty::tag::{ItemValue, Tag, TagItem, TagType};
use std::path::Path;

/// Returns the tag edits should go to: the tag matching the file's primary
/// tag type when present, else the first non-ID3v1 tag. Preferring the
/// primary type matters for e.g. FLAC files carrying a stray ID3v2 tag:
/// lofty cannot write ID3v2 to FLAC, so edits must target Vorbis Comments.
pub fn editable_tag(tagged: &TaggedFile) -> Option<&Tag> {
  tagged.primary_tag().or_else(|| {
    tagged
      .tags()
      .iter()
      .find(|t| t.tag_type() != TagType::Id3v1)
  })
}

/// Returns every `Description` value present on the tag, in source order,
/// or a single empty string when none exist so the UI always shows one
/// editor. Empty entries are dropped on save (see [`apply_descriptions`]).
pub fn read_descriptions(tag: &Tag) -> Vec<String> {
  let mut descriptions: Vec<String> = tag
    .get_strings(ItemKey::Description)
    .map(|s| s.to_string())
    .collect();
  if descriptions.is_empty() {
    descriptions.push(String::new());
  }
  descriptions
}

/// Replaces all `Description` items on the tag with the given values,
/// skipping empty ones (so emptied editors are removed on save). Uses
/// `push_unchecked` so values reach lofty's per-format conversion even when
/// the main map doesn't include `Description`; callers should verify the
/// write afterwards rather than relying on a silent insert failure.
pub fn apply_descriptions(tag: &mut Tag, descriptions: &[String]) {
  tag.remove_key(ItemKey::Description);
  for desc in descriptions {
    if !desc.is_empty() {
      tag.push_unchecked(TagItem::new(
        ItemKey::Description,
        ItemValue::Text(desc.clone()),
      ));
    }
  }
}

/// Returns every value present for `key` on the tag, in source order. For
/// ID3v2.4 lofty splits a multi-value text frame (null-separated) into one
/// entry per value, so this yields each artist, genre, etc. separately.
/// Returns an empty `Vec` when the key is absent.
pub fn read_values(tag: &Tag, key: ItemKey) -> Vec<String> {
  tag.get_strings(key).map(|s| s.to_string()).collect()
}

/// Replaces all items for `key` with `values`, skipping empty ones. Each
/// value becomes its own `TagItem`; on save lofty joins same-key items into a
/// single ID3v2.4 frame separated by `\0` (and writes them as the format's
/// native multi-value representation for FLAC / OGG / M4A). Uses
/// `push_unchecked` so values reach lofty's per-format conversion; callers
/// should verify the write afterwards.
pub fn apply_values(tag: &mut Tag, key: ItemKey, values: &[String]) {
  tag.remove_key(key);
  for value in values {
    if !value.is_empty() {
      tag.push_unchecked(TagItem::new(key, ItemValue::Text(value.clone())));
    }
  }
}

/// Reads descriptions directly from an audio file at `path`. Skips ID3v1
/// since it has no Description concept; otherwise uses the first non-v1 tag
/// (or the primary tag).
pub fn read_descriptions_from_path(path: &Path) -> Result<Vec<String>, String> {
  let tagged = lofty::read_from_path(path).map_err(|e| e.to_string())?;
  Ok(
    editable_tag(&tagged)
      .map(read_descriptions)
      .unwrap_or_default(),
  )
}

/// Writes `descriptions` to the file's primary tag (excluding ID3v1),
/// replacing any existing Description items. Empty entries are dropped.
pub fn write_descriptions_to_path(
  path: &Path,
  descriptions: &[String],
) -> Result<(), String> {
  let tagged = lofty::read_from_path(path).map_err(|e| e.to_string())?;
  let mut tag = match editable_tag(&tagged).cloned() {
    Some(t) => t,
    None => Tag::new(tagged.primary_tag_type()),
  };
  apply_descriptions(&mut tag, descriptions);
  tag
    .save_to_path(path, WriteOptions::default())
    .map_err(|e| e.to_string())
}

/// Reads every value for `key` from the file's primary tag (excluding ID3v1).
/// For ID3v2.4 this returns each null-separated value of the text frame
/// separately.
pub fn read_values_from_path(
  path: &Path,
  key: ItemKey,
) -> Result<Vec<String>, String> {
  let tagged = lofty::read_from_path(path).map_err(|e| e.to_string())?;
  Ok(
    editable_tag(&tagged)
      .map(|t| read_values(t, key))
      .unwrap_or_default(),
  )
}

/// Writes `values` for `key` to the file's primary tag (excluding ID3v1),
/// replacing any existing items. Empty entries are dropped; for ID3v2.4 the
/// values are stored as a single null-separated text frame.
pub fn write_values_to_path(
  path: &Path,
  key: ItemKey,
  values: &[String],
) -> Result<(), String> {
  let tagged = lofty::read_from_path(path).map_err(|e| e.to_string())?;
  let mut tag = match editable_tag(&tagged).cloned() {
    Some(t) => t,
    None => Tag::new(tagged.primary_tag_type()),
  };
  apply_values(&mut tag, key, values);
  tag
    .save_to_path(path, WriteOptions::default())
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
  use super::*;

  fn make_tag(tag_type: TagType, descriptions: &[&str]) -> Tag {
    let mut tag = Tag::new(tag_type);
    for d in descriptions {
      tag.push(TagItem::new(
        ItemKey::Description,
        ItemValue::Text((*d).to_string()),
      ));
    }
    tag
  }

  #[test]
  fn read_descriptions_returns_placeholder_for_any_tag_type() {
    for ty in [
      TagType::VorbisComments,
      TagType::Mp4Ilst,
      TagType::Id3v2,
      TagType::Ape,
      TagType::RiffInfo,
    ] {
      assert_eq!(
        read_descriptions(&Tag::new(ty)),
        vec![String::new()],
        "tag type {ty:?}"
      );
    }
  }

  #[test]
  fn read_descriptions_preserves_vorbis_order() {
    let tag = make_tag(TagType::VorbisComments, &["first", "second", "third"]);
    assert_eq!(
      read_descriptions(&tag),
      vec![
        "first".to_string(),
        "second".to_string(),
        "third".to_string()
      ],
    );
  }

  #[test]
  fn apply_descriptions_round_trips_multiple_values() {
    let mut tag = Tag::new(TagType::VorbisComments);
    apply_descriptions(&mut tag, &["a".to_string(), "b".to_string()]);
    assert_eq!(
      read_descriptions(&tag),
      vec!["a".to_string(), "b".to_string()],
    );
  }

  #[test]
  fn apply_descriptions_drops_empty_entries() {
    let mut tag = make_tag(TagType::VorbisComments, &["keep", "drop", "also"]);
    apply_descriptions(
      &mut tag,
      &["keep".to_string(), String::new(), "also".to_string()],
    );
    assert_eq!(
      tag.get_strings(ItemKey::Description).collect::<Vec<_>>(),
      vec!["keep", "also"],
    );
  }

  #[test]
  fn apply_descriptions_with_all_empty_clears_field() {
    let mut tag = make_tag(TagType::VorbisComments, &["x", "y"]);
    apply_descriptions(&mut tag, &[String::new(), String::new()]);
    assert_eq!(tag.get_strings(ItemKey::Description).count(), 0,);
  }

  #[test]
  fn apply_descriptions_replaces_existing_values() {
    let mut tag = make_tag(TagType::VorbisComments, &["old1", "old2"]);
    apply_descriptions(&mut tag, &["new".to_string()]);
    assert_eq!(
      tag.get_strings(ItemKey::Description).collect::<Vec<_>>(),
      vec!["new"],
    );
  }

  #[test]
  fn read_values_returns_each_value_in_order() {
    let mut tag = Tag::new(TagType::Id3v2);
    for a in ["Daft Punk", "Pharrell Williams"] {
      tag.push(TagItem::new(
        ItemKey::TrackArtist,
        ItemValue::Text(a.to_string()),
      ));
    }
    assert_eq!(
      read_values(&tag, ItemKey::TrackArtist),
      vec!["Daft Punk".to_string(), "Pharrell Williams".to_string()],
    );
  }

  #[test]
  fn read_values_is_empty_when_key_absent() {
    let tag = Tag::new(TagType::Id3v2);
    assert!(read_values(&tag, ItemKey::Genre).is_empty());
  }

  #[test]
  fn apply_values_round_trips_multiple_and_drops_empties() {
    let mut tag = Tag::new(TagType::Id3v2);
    apply_values(
      &mut tag,
      ItemKey::Genre,
      &["Electronic".to_string(), String::new(), "House".to_string()],
    );
    assert_eq!(
      read_values(&tag, ItemKey::Genre),
      vec!["Electronic".to_string(), "House".to_string()],
    );
  }

  #[test]
  fn apply_values_replaces_existing_items() {
    let mut tag = Tag::new(TagType::Id3v2);
    apply_values(&mut tag, ItemKey::TrackArtist, &["old".to_string()]);
    apply_values(
      &mut tag,
      ItemKey::TrackArtist,
      &["a".to_string(), "b".to_string()],
    );
    assert_eq!(
      read_values(&tag, ItemKey::TrackArtist),
      vec!["a".to_string(), "b".to_string()],
    );
  }

  #[test]
  fn apply_values_with_all_empty_clears_key() {
    let mut tag = Tag::new(TagType::Id3v2);
    apply_values(&mut tag, ItemKey::Genre, &["Rock".to_string()]);
    apply_values(&mut tag, ItemKey::Genre, &[String::new()]);
    assert!(read_values(&tag, ItemKey::Genre).is_empty());
  }
}
