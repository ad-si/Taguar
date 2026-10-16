//! Tag writing that avoids rewriting the audio data where possible.
//!
//! lofty keeps an existing FLAC PADDING block as is
//! (<https://github.com/Serial-ATA/lofty-rs/issues/445>), so any change in
//! the size of the Vorbis comments or pictures shifts the whole audio stream
//! behind them. For FLAC, these helpers let lofty encode the new metadata
//! blocks into an in-memory copy of the header only and then resize the
//! padding the way `metaflac` does: the size difference is absorbed by the
//! padding if it fits, otherwise the header grows by the preferred padding of
//! the `padding` argument (`0` meaning none, i.e. the file shrinks for
//! real when metadata is removed).

use lofty::config::WriteOptions;
use lofty::error::FileEncodingError;
use lofty::io::FileLike;
use lofty::prelude::TagExt;
use lofty::tag::TagType;
use std::fs::{File, OpenOptions};
use std::io::{Cursor, Read, Seek, SeekFrom};
use std::path::Path;

const FLAC_MARKER: &[u8; 4] = b"fLaC";
const BLOCK_HEADER_SIZE: usize = 4;
const BLOCK_TYPE_PADDING: u8 = 1;
const MAX_BLOCK_CONTENT_SIZE: usize = 0xFF_FFFF;

/// Writes `tag` to `path`.
pub fn save_tag<T: TagExt>(
  tag: &T,
  path: &Path,
  padding: u32,
) -> Result<(), String> {
  let write_options = WriteOptions::new().preferred_padding(padding);
  match tag.tag_type() {
    TagType::VorbisComments => {
      let written = write_flac_metadata(path, padding, |header, opts| {
        tag.save_to(header, opts)
      })?;
      if written {
        return Ok(());
      }
    }
    _ => {}
  }
  tag
    .save_to_path(path, write_options)
    .map_err(|e| e.to_string())
}

/// Removes the tag of `tag`'s type from `path`.
pub fn remove_tag<T: TagExt>(
  tag: &T,
  path: &Path,
  padding: u32,
) -> Result<(), String> {
  let write_options = WriteOptions::new().preferred_padding(padding);
  match tag.tag_type() {
    TagType::VorbisComments => {
      let written = write_flac_metadata(path, padding, |header, opts| {
        tag.remove_from(header, opts)
      })?;
      if written {
        return Ok(());
      }
    }
    _ => {}
  }
  // lofty 0.25's `remove_from_path` opens the file read-only, so every
  // removal through it fails
  // (https://github.com/Serial-ATA/lofty-rs/issues/750).
  let mut file = OpenOptions::new()
    .read(true)
    .write(true)
    .open(path)
    .map_err(|e| e.to_string())?;
  tag
    .remove_from(&mut file, write_options)
    .map_err(|e| e.to_string())
}

/// Lets `write` update the metadata blocks of the FLAC file at `path` and
/// stores the result, keeping the audio data in place whenever the new
/// blocks fit into the old header. Returns `Ok(false)` without touching the
/// file if `path` isn't a FLAC file.
fn write_flac_metadata(
  path: &Path,
  padding: u32,
  write: impl FnOnce(
    &mut Cursor<Vec<u8>>,
    WriteOptions,
  ) -> Result<(), FileEncodingError>,
) -> Result<bool, String> {
  let mut file = OpenOptions::new()
    .read(true)
    .write(true)
    .open(path)
    .map_err(|e| e.to_string())?;
  let Some((old_header, blocks_start)) =
    read_flac_header(&mut file).map_err(|e| e.to_string())?
  else {
    return Ok(false);
  };

  // Stand-in for the audio data: lofty 0.25 mis-truncates when the replaced
  // metadata reaches the end of the file (it truncates to the size
  // difference instead of the new length,
  // https://github.com/Serial-ATA/lofty-rs/issues/731).
  let mut header = old_header.clone();
  header.push(0);

  // The padding is recomputed below, so lofty mustn't add any of its own.
  let mut header = Cursor::new(header);
  write(&mut header, WriteOptions::new().preferred_padding(0))
    .map_err(|e| e.to_string())?;
  let mut header = header.into_inner();
  header.pop();

  // lofty leaves a leading ID3v2 tag alone, so the blocks still start at the
  // same offset.
  let blocks = flac_blocks(&header, blocks_start)
    .ok_or("lofty wrote an invalid FLAC header")?;
  let mut new_header = header[..blocks_start].to_vec();
  for &(_, block) in blocks.iter().filter(|(ty, _)| *ty != BLOCK_TYPE_PADDING) {
    new_header.extend_from_slice(block);
  }

  if padding > 0 {
    let free = old_header.len().checked_sub(new_header.len());
    let padding_size = match free {
      Some(0) => None,
      Some(free)
        if (BLOCK_HEADER_SIZE..=BLOCK_HEADER_SIZE + MAX_BLOCK_CONTENT_SIZE)
          .contains(&free) =>
      {
        Some(free - BLOCK_HEADER_SIZE)
      }
      _ => Some((padding as usize).min(MAX_BLOCK_CONTENT_SIZE)),
    };
    if let Some(size) = padding_size {
      new_header.push(BLOCK_TYPE_PADDING);
      new_header.extend_from_slice(&(size as u32).to_be_bytes()[1..]);
      new_header.resize(new_header.len() + size, 0);
    }
  }
  mark_last_block(&mut new_header, blocks_start);

  file
    .splice(0..old_header.len() as u64, &new_header)
    .map_err(|e| e.to_string())?;
  Ok(true)
}

/// Reads the bytes of `file` up to the end of its last FLAC metadata block,
/// including a leading ID3v2 tag, along with the offset of the first block.
/// `None` if it isn't a FLAC file.
fn read_flac_header(
  file: &mut File,
) -> std::io::Result<Option<(Vec<u8>, usize)>> {
  let mut pos = 0;
  let mut id3 = [0; 10];
  file.seek(SeekFrom::Start(0))?;
  if file.read_exact(&mut id3).is_ok() && id3.starts_with(b"ID3") {
    let size = id3[6..10]
      .iter()
      .fold(0, |acc, &b| (acc << 7) | u64::from(b & 0x7F));
    let footer = if id3[5] & 0x10 != 0 { 10 } else { 0 };
    pos = 10 + size + footer;
  }

  let mut marker = [0; 4];
  file.seek(SeekFrom::Start(pos))?;
  if file.read_exact(&mut marker).is_err() || &marker != FLAC_MARKER {
    return Ok(None);
  }
  pos += FLAC_MARKER.len() as u64;
  let blocks_start = pos as usize;

  loop {
    let mut block_header = [0; BLOCK_HEADER_SIZE];
    file.seek(SeekFrom::Start(pos))?;
    file.read_exact(&mut block_header)?;
    let size = u32::from_be_bytes([
      0,
      block_header[1],
      block_header[2],
      block_header[3],
    ]);
    pos += (BLOCK_HEADER_SIZE as u64) + u64::from(size);
    if block_header[0] & 0x80 != 0 {
      break;
    }
  }

  let mut header = vec![0; pos as usize];
  file.seek(SeekFrom::Start(0))?;
  file.read_exact(&mut header)?;
  Ok(Some((header, blocks_start)))
}

/// Splits the metadata blocks of a FLAC header, starting at `blocks_start`,
/// into each block's type and bytes (block header included).
fn flac_blocks(header: &[u8], blocks_start: usize) -> Option<Vec<(u8, &[u8])>> {
  let mut blocks = Vec::new();
  let mut pos = blocks_start;
  loop {
    let block_header = header.get(pos..pos + BLOCK_HEADER_SIZE)?;
    let size = u32::from_be_bytes([
      0,
      block_header[1],
      block_header[2],
      block_header[3],
    ]) as usize;
    let end = pos + BLOCK_HEADER_SIZE + size;
    blocks.push((block_header[0] & 0x7F, header.get(pos..end)?));
    pos = end;
    if block_header[0] & 0x80 != 0 {
      return Some(blocks);
    }
  }
}

/// Sets the "last block" flag on the final metadata block of `header` (whose
/// blocks begin at `blocks_start`) and clears it on all others.
fn mark_last_block(header: &mut [u8], blocks_start: usize) {
  let mut pos = blocks_start;
  while pos < header.len() {
    let size = u32::from_be_bytes([
      0,
      header[pos + 1],
      header[pos + 2],
      header[pos + 3],
    ]) as usize;
    let end = pos + BLOCK_HEADER_SIZE + size;
    if end >= header.len() {
      header[pos] |= 0x80;
    }
    else {
      header[pos] &= 0x7F;
    }
    pos = end;
  }
}
