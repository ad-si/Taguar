//! Keeps an `ilst` atom in `moov.udta.meta` at all times.
//!
//! When `meta` has no `ilst` child, lofty 0.25 writes the new `ilst` over the
//! first child of `meta` instead, which is the mandatory `hdlr` atom
//! (<https://github.com/Serial-ATA/lofty-rs/issues/751>). lofty's own tag
//! removal leaves exactly such a `meta` behind. So removing a tag here only
//! empties the `ilst` atom, and writing one first gives `meta` its `ilst`
//! back if it lost it.
//!
//! The space of the removed items stays in the file as a `free` atom, like
//! lofty already keeps any other `free` atoms around the `ilst`: shrinking
//! the file would need lofty's removal, which drops the `ilst` atom.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

struct Atom {
  start: u64,
  header_len: u64,
  len: u64,
  kind: [u8; 4],
}

impl Atom {
  fn data_start(&self) -> u64 {
    self.start + self.header_len
  }

  fn end(&self) -> u64 {
    self.start + self.len
  }
}

/// Atoms that only reserve space.
fn is_padding(atom: &Atom) -> bool {
  &atom.kind == b"free" || &atom.kind == b"skip"
}

/// Makes sure `meta` has an `ilst` child before lofty writes a non-empty
/// `ilst` to `path`.
pub(super) fn prepare_ilst_write(path: &Path) -> Result<(), String> {
  let mut file = open(path)?;
  let Some((meta, children)) = find_meta(&mut file)? else {
    // lofty creates `udta` and `meta` from scratch.
    return Ok(());
  };
  if children.iter().any(|a| &a.kind == b"ilst") {
    return Ok(());
  }

  // Carve an empty `ilst` out of existing padding, so lofty can write the
  // tag into the rest of it.
  let usable_padding = children.iter().find(|a| {
    is_padding(a) && (a.len == 8 || a.len >= 16) && a.len <= u64::from(u32::MAX)
  });
  if let Some(padding) = usable_padding {
    write_atom_header(&mut file, padding.start, 8, b"ilst")?;
    if padding.len > 8 {
      write_atom_header(
        &mut file,
        padding.start + 8,
        padding.len - 8,
        b"free",
      )?;
    }
    return Ok(());
  }

  // Without room for an `ilst`, turn the whole `meta` into padding and let
  // lofty create a new one. Only safe if `meta` holds nothing else of value.
  if children.iter().all(|a| &a.kind == b"hdlr" || is_padding(a)) {
    file
      .seek(SeekFrom::Start(meta.start + 4))
      .and_then(|_| file.write_all(b"free"))
      .map_err(|e| e.to_string())?;
    return Ok(());
  }

  Err(
    "the MP4 `meta` atom has no `ilst` atom and no room for one, \
     so writing a tag would destroy its other atoms"
      .to_string(),
  )
}

/// Removes all items from the `ilst` of `path`, leaving an empty `ilst`
/// followed by a `free` atom in its place.
pub(super) fn clear_ilst(path: &Path) -> Result<(), String> {
  let mut file = open(path)?;
  let Some(ilst) = find_meta(&mut file)?.and_then(|(_, children)| {
    children.into_iter().find(|a| &a.kind == b"ilst")
  }) else {
    return Ok(());
  };
  if ilst.len == 8 {
    return Ok(());
  }
  if ilst.len < 16 {
    return Err(format!("malformed MP4 `ilst` atom of {} bytes", ilst.len));
  }

  write_atom_header(&mut file, ilst.start, 8, b"ilst")?;
  let free_start = ilst.start + 8;
  let free_len = ilst.len - 8;
  let free_header_len =
    write_atom_header(&mut file, free_start, free_len, b"free")?;

  // Zero what's left of the old items, so the removed metadata is gone.
  let zeros = vec![0; 64 * 1024];
  let mut remaining = free_len - free_header_len;
  while remaining > 0 {
    let chunk = remaining.min(zeros.len() as u64) as usize;
    file.write_all(&zeros[..chunk]).map_err(|e| e.to_string())?;
    remaining -= chunk as u64;
  }
  Ok(())
}

fn open(path: &Path) -> Result<File, String> {
  OpenOptions::new()
    .read(true)
    .write(true)
    .open(path)
    .map_err(|e| e.to_string())
}

/// The `moov.udta.meta` atom lofty keeps the `ilst` in, and its children.
fn find_meta(file: &mut File) -> Result<Option<(Atom, Vec<Atom>)>, String> {
  let file_len = file.metadata().map_err(|e| e.to_string())?.len();
  let find = |file: &mut File, start, end, kind: &[u8; 4]| {
    read_children(file, start, end)
      .map(|atoms| atoms.into_iter().find(|a| &a.kind == kind))
      .map_err(|e| e.to_string())
  };

  let Some(moov) = find(file, 0, file_len, b"moov")? else {
    return Ok(None);
  };
  let Some(udta) = find(file, moov.data_start(), moov.end(), b"udta")? else {
    return Ok(None);
  };
  let Some(meta) = find(file, udta.data_start(), udta.end(), b"meta")? else {
    return Ok(None);
  };

  // ISO `meta` is a full box with version and flags before its children,
  // QuickTime's isn't.
  let mut head = [0; 8];
  file
    .seek(SeekFrom::Start(meta.data_start()))
    .and_then(|_| file.read_exact(&mut head))
    .map_err(|e| e.to_string())?;
  let children_start = if &head[4..] == b"hdlr" {
    meta.data_start()
  }
  else {
    meta.data_start() + 4
  };

  let children = read_children(file, children_start, meta.end())
    .map_err(|e| e.to_string())?;
  Ok(Some((meta, children)))
}

/// The atoms between `start` and `end`. Stops at the first malformed one.
fn read_children(
  file: &mut File,
  start: u64,
  end: u64,
) -> std::io::Result<Vec<Atom>> {
  let mut atoms = Vec::new();
  let mut pos = start;
  while pos + 8 <= end {
    let mut header = [0; 8];
    file.seek(SeekFrom::Start(pos))?;
    file.read_exact(&mut header)?;
    let size = u32::from_be_bytes([header[0], header[1], header[2], header[3]]);
    let (len, header_len) = match size {
      0 => (end - pos, 8),
      1 => {
        let mut size = [0; 8];
        file.read_exact(&mut size)?;
        (u64::from_be_bytes(size), 16)
      }
      size => (u64::from(size), 8),
    };
    if len < header_len || pos + len > end {
      break;
    }
    atoms.push(Atom {
      start: pos,
      header_len,
      len,
      kind: [header[4], header[5], header[6], header[7]],
    });
    pos += len;
  }
  Ok(atoms)
}

/// Writes the header of a `len` byte atom at `pos`, returning its length.
fn write_atom_header(
  file: &mut File,
  pos: u64,
  len: u64,
  kind: &[u8; 4],
) -> Result<u64, String> {
  let mut header = Vec::with_capacity(16);
  match u32::try_from(len) {
    Ok(len) => header.extend_from_slice(&len.to_be_bytes()),
    Err(_) => header.extend_from_slice(&1u32.to_be_bytes()),
  }
  header.extend_from_slice(kind);
  if len > u64::from(u32::MAX) {
    header.extend_from_slice(&len.to_be_bytes());
  }
  file
    .seek(SeekFrom::Start(pos))
    .and_then(|_| file.write_all(&header))
    .map_err(|e| e.to_string())?;
  Ok(header.len() as u64)
}
