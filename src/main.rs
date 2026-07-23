#![windows_subsystem = "windows"]

use iced::widget::{
  button, checkbox, column, container, image, mouse_area, opaque, row,
  scrollable, slider, stack, text, text_editor, text_input, Column, Row, Space,
};
use iced::{
  event, keyboard, mouse, Alignment, Background, Border, Color, Element, Event,
  Font, Length, Padding, Point, Subscription, Task, Theme,
};
use lofty::config::{ParseOptions, WriteOptions};
use lofty::file::{FileType, TaggedFileExt};
use lofty::flac::FlacFile;
use lofty::id3::v2::{ExtendedUrlFrame, Frame, Id3v2Tag};
use lofty::ogg::{
  OggPictureStorage, OpusFile, SpeexFile, VorbisComments, VorbisFile,
};
use lofty::picture::{Picture, PictureType};
use lofty::prelude::{Accessor, AudioFile, ItemKey, TagExt};
use lofty::tag::items::Timestamp;
use lofty::tag::{ItemValue, Tag, TagItem, TagType};
use lofty::TextEncoding;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, OnceLock};
use std::thread;
use std::time::Duration;
use taguar::{
  apply_descriptions, apply_values, read_descriptions, read_values,
};
use walkdir::WalkDir;

const AUDIO_EXTENSIONS: &[&str] = &[
  "mp3", "flac", "m4a", "m4b", "mp4", "ogg", "opus", "oga", "wav", "aiff",
  "aif", "aifc", "wv", "ape",
];

const ORANGE: Color = Color::from_rgb(0.96, 0.52, 0.15);
const ORANGE_DARK: Color = Color::from_rgb(0.85, 0.45, 0.12);
const ROW_HOVER: Color = Color::from_rgb(0.93, 0.93, 0.94);
const ROW_ALT: Color = Color::from_rgb(0.97, 0.97, 0.98);
const BORDER: Color = Color::from_rgb(0.82, 0.82, 0.84);
const PANEL_BG: Color = Color::from_rgb(0.98, 0.98, 0.99);
const HEADER_BG: Color = Color::from_rgb(0.94, 0.94, 0.96);
const MUTED: Color = Color::from_rgb(0.40, 0.40, 0.44);
const MODAL_SCRIM: Color = Color::from_rgba(0.0, 0.0, 0.0, 0.45);

/// Width of the column-picker dropdown panel.
const COLUMN_MENU_WIDTH: f32 = 180.0;

const FONT_REGULAR_BYTES: &[u8] =
  include_bytes!("../fonts/FiraSans-Regular.ttf");
const FONT_BOLD_BYTES: &[u8] = include_bytes!("../fonts/FiraSans-Bold.ttf");

const APP_FONT: Font = Font::with_name("Fira Sans");
const BOLD: Font = Font {
  family: iced::font::Family::Name("Fira Sans"),
  weight: iced::font::Weight::Bold,
  stretch: iced::font::Stretch::Normal,
  style: iced::font::Style::Normal,
};

pub fn main() -> iced::Result {
  let args: Vec<String> = std::env::args().skip(1).collect();
  if args.iter().any(|a| a == "-h" || a == "--help") {
    println!("Usage: taguar [DIRECTORY | FILE...]");
    std::process::exit(0);
  }
  let source = parse_source(&args);

  iced::application(
    move || {
      let state = Taguar::default();
      let task = match source.clone() {
        Some(Source::Directory(dir)) => {
          Task::done(Message::DirectoryChosen(Some(dir)))
        }
        Some(Source::Files(files)) => Task::done(Message::FilesChosen(files)),
        None => Task::none(),
      };
      (state, task)
    },
    Taguar::update,
    Taguar::view,
  )
  .subscription(Taguar::subscription)
  .title(|state: &Taguar| match &state.source {
    Some(Source::Directory(dir)) => {
      format!("Taguar — {}", dir.to_string_lossy())
    }
    Some(Source::Files(files)) => match files.as_slice() {
      [file] => format!(
        "Taguar — {}",
        file
          .file_name()
          .map(|n| n.to_string_lossy().into_owned())
          .unwrap_or_else(|| file.to_string_lossy().into_owned())
      ),
      _ => format!("Taguar — {} files", files.len()),
    },
    None => "Taguar".to_string(),
  })
  .theme(Theme::Light)
  .window_size((1200.0, 760.0))
  .font(FONT_REGULAR_BYTES)
  .font(FONT_BOLD_BYTES)
  .default_font(APP_FONT)
  .run()
}

/// What the current file listing was loaded from: either a directory that is
/// scanned recursively, or an explicit list of files passed on the command
/// line.
#[derive(Clone)]
enum Source {
  Directory(PathBuf),
  Files(Vec<PathBuf>),
}

/// Interprets the command-line arguments as either a single directory to scan
/// or an explicit list of files. A lone directory argument keeps the original
/// recursive-scan behavior; otherwise every argument is collected into a flat
/// file list, with any directory argument expanded to the audio files it
/// contains. Exits with an error for paths that don't exist.
fn parse_source(args: &[String]) -> Option<Source> {
  if args.is_empty() {
    return None;
  }
  if let [only] = args {
    let path = PathBuf::from(only);
    if path.is_dir() {
      return Some(Source::Directory(path.canonicalize().unwrap_or(path)));
    }
  }
  let mut files = Vec::new();
  let mut seen = std::collections::HashSet::new();
  for arg in args {
    let path = PathBuf::from(arg);
    if path.is_dir() {
      let dir = path.canonicalize().unwrap_or(path);
      for file in scan_audio_paths(&dir) {
        if seen.insert(file.clone()) {
          files.push(file);
        }
      }
    }
    else if path.is_file() {
      let file = path.canonicalize().unwrap_or(path);
      if seen.insert(file.clone()) {
        files.push(file);
      }
    }
    else {
      eprintln!("No such file or directory: {}", path.display());
      std::process::exit(2);
    }
  }
  Some(Source::Files(files))
}

/// A column that can be shown in the file listing. The variant order is the
/// canonical left-to-right display order; visibility is controlled per column
/// via [`Settings::visible_columns`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TableColumn {
  FilePath,
  Artist,
  Title,
  ReleaseDate,
  Genre,
  Description,
  Comment,
  Composer,
  Arranger,
  Album,
  Duration,
  Size,
}

impl TableColumn {
  /// All columns in canonical display order.
  const ALL: [TableColumn; 12] = [
    Self::FilePath,
    Self::Artist,
    Self::Title,
    Self::ReleaseDate,
    Self::Genre,
    Self::Description,
    Self::Comment,
    Self::Composer,
    Self::Arranger,
    Self::Album,
    Self::Duration,
    Self::Size,
  ];

  fn label(self) -> &'static str {
    match self {
      Self::FilePath => "File Path",
      Self::Artist => "Artist",
      Self::Title => "Title",
      Self::Album => "Album",
      Self::Genre => "Genre",
      Self::ReleaseDate => "Release Date",
      Self::Composer => "Composer",
      Self::Arranger => "Arranger",
      Self::Comment => "Comment",
      Self::Description => "Description",
      Self::Duration => "Duration",
      Self::Size => "Size",
    }
  }

  /// Proportional width used in both the header and the body rows.
  fn weight(self) -> u16 {
    match self {
      Self::FilePath => 8,
      Self::Artist => 4,
      Self::Title => 5,
      Self::Album => 5,
      Self::Genre => 3,
      Self::ReleaseDate => 3,
      Self::Composer => 4,
      Self::Arranger => 4,
      Self::Comment => 6,
      Self::Description => 6,
      Self::Duration => 2,
      Self::Size => 2,
    }
  }

  /// The multi-valued fields for this column, or `None` for single-valued
  /// columns. Drives whether a table cell renders pills (>= 2 entries) or
  /// plain text.
  fn multi_values(self, info: &FileInfo) -> Option<&[String]> {
    match self {
      Self::Artist => Some(&info.artist),
      Self::Genre => Some(&info.genre),
      Self::Composer => Some(&info.composer),
      Self::Arranger => Some(&info.arranger),
      _ => None,
    }
  }

  fn cell_text(self, info: &FileInfo) -> String {
    match self {
      Self::FilePath => info.filename.clone(),
      Self::Artist => info.artist.join(", "),
      Self::Title => info.title.clone(),
      Self::Album => info.album.clone(),
      Self::Genre => info.genre.join(", "),
      Self::ReleaseDate => info.release_date.clone(),
      Self::Composer => info.composer.join(", "),
      Self::Arranger => info.arranger.join(", "),
      Self::Comment => info.comment.clone(),
      Self::Description => info.description.clone(),
      Self::Duration => format_duration(info.duration_secs),
      Self::Size => format_size(info.size_bytes),
    }
  }
}

/// Persisted user settings. Loaded from [`settings_path`] on startup and
/// written back whenever the user changes something (e.g. toggles a column).
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
struct Settings {
  /// Columns to show in the listing, in canonical order. Stored as a list so
  /// the on-disk file is self-describing; rendering always follows
  /// [`TableColumn::ALL`] order regardless of the order here.
  visible_columns: Vec<TableColumn>,
}

impl Default for Settings {
  fn default() -> Self {
    Self {
      visible_columns: vec![
        TableColumn::FilePath,
        TableColumn::Artist,
        TableColumn::Title,
        TableColumn::Comment,
      ],
    }
  }
}

impl Settings {
  fn load() -> Self {
    let Some(path) = settings_path() else {
      return Self::default();
    };
    let mut settings: Settings = match std::fs::read_to_string(&path) {
      Ok(contents) => serde_yaml::from_str(&contents).unwrap_or_default(),
      Err(_) => Self::default(),
    };
    // Guard against a hand-edited file that hides every column.
    if settings.visible_columns.is_empty() {
      settings.visible_columns = Self::default().visible_columns;
    }
    settings
  }

  fn save(&self) {
    let Some(path) = settings_path() else {
      return;
    };
    if let Some(parent) = path.parent() {
      let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(yaml) = serde_yaml::to_string(self) {
      let _ = std::fs::write(&path, yaml);
    }
  }

  fn is_visible(&self, column: TableColumn) -> bool {
    self.visible_columns.contains(&column)
  }

  /// Adds the column (in canonical order) if hidden, removes it if shown.
  /// Refuses to remove the last visible column — at least one must remain.
  fn toggle_column(&mut self, column: TableColumn) {
    if let Some(pos) = self.visible_columns.iter().position(|c| *c == column) {
      if self.visible_columns.len() > 1 {
        self.visible_columns.remove(pos);
      }
    }
    else {
      self.visible_columns.push(column);
    }
  }
}

/// Location of the YAML config file: `$XDG_CONFIG_HOME/taguar/config.yaml`,
/// falling back to `%APPDATA%\taguar` on Windows and `~/.config/taguar`
/// elsewhere.
fn settings_path() -> Option<PathBuf> {
  let dir = if let Some(xdg) =
    std::env::var_os("XDG_CONFIG_HOME").filter(|s| !s.is_empty())
  {
    PathBuf::from(xdg)
  }
  else if cfg!(target_os = "windows") {
    PathBuf::from(std::env::var_os("APPDATA")?)
  }
  else {
    PathBuf::from(std::env::var_os("HOME")?).join(".config")
  };
  Some(dir.join("taguar").join("config.yaml"))
}

struct Taguar {
  source: Option<Source>,
  files: Vec<FileInfo>,
  selected_idx: Option<usize>,
  form: TagForm,
  saved_form: TagForm,
  lyrics_content: text_editor::Content,
  comment_content: text_editor::Content,
  description_contents: Vec<text_editor::Content>,
  id3v1: Option<Id3v1Display>,
  cover: Option<CoverInfo>,
  primary_tag_label: String,
  status: Option<String>,
  loading: bool,
  playing_path: Option<PathBuf>,
  is_paused: bool,
  /// Playback position of the active track in seconds, polled while playing.
  playback_pos_secs: f64,
  /// Seek-bar value while the user is dragging it; the actual seek is only
  /// sent to the playback thread on release.
  seek_drag_secs: Option<f64>,
  metadata_dump: Option<MetadataDump>,
  /// Open right-click dropdown within the metadata modal.
  copy_menu: Option<CopyMenu>,
  /// Open right-click dropdown for a song row in the listing.
  song_menu: Option<SongMenu>,
  /// Last known cursor position in window coordinates — captured from the
  /// event subscription while the modal is open, so we can pin the dropdown
  /// to where the user right-clicked.
  last_cursor: Option<Point>,
  /// Transient feedback shown in the modal header after a copy.
  copy_feedback: Option<String>,
  /// In-progress text for each pill field (Artist, Album Artist, Genre,
  /// Composer, Arranger) — the value typed but not yet committed to a pill.
  artist_draft: String,
  album_artist_draft: String,
  genre_draft: String,
  composer_draft: String,
  arranger_draft: String,
  /// Set when a draft edit just emptied a pill input, so the deferred
  /// backspace handler doesn't mistake "deleted the last character" for
  /// "backspace on an already-empty input" and remove a pill.
  pill_backspace_suppressed: bool,
  /// Set when the user tried to switch songs with unsaved edits in the form.
  /// Cleared once the changes are saved or the user reloads the current
  /// song (clicking the same row re-reads from disk).
  nav_warning: bool,
  cover_modal_open: bool,
  /// Persisted user settings (e.g. which listing columns are visible).
  settings: Settings,
  /// Anchor point of the open column-picker dropdown (window coordinates),
  /// or `None` when it's closed.
  column_menu: Option<Point>,
}

impl Default for Taguar {
  fn default() -> Self {
    Self {
      source: None,
      files: Vec::new(),
      selected_idx: None,
      form: TagForm::default(),
      saved_form: TagForm::default(),
      lyrics_content: text_editor::Content::new(),
      comment_content: text_editor::Content::new(),
      description_contents: Vec::new(),
      id3v1: None,
      cover: None,
      primary_tag_label: String::new(),
      status: None,
      loading: false,
      playing_path: None,
      is_paused: false,
      playback_pos_secs: 0.0,
      seek_drag_secs: None,
      metadata_dump: None,
      copy_menu: None,
      song_menu: None,
      last_cursor: None,
      copy_feedback: None,
      artist_draft: String::new(),
      album_artist_draft: String::new(),
      genre_draft: String::new(),
      composer_draft: String::new(),
      arranger_draft: String::new(),
      pill_backspace_suppressed: false,
      nav_warning: false,
      cover_modal_open: false,
      settings: Settings::load(),
      column_menu: None,
    }
  }
}

#[derive(Clone)]
struct CopyMenu {
  /// Anchor position (window coordinates) where the dropdown should appear.
  at: Point,
  key: String,
  value: String,
}

#[derive(Clone)]
struct SongMenu {
  /// Anchor position (window coordinates) where the dropdown should appear.
  at: Point,
  /// Filesystem path of the right-clicked song.
  path: PathBuf,
}

/// Snapshot of the currently selected file's metadata, shown in the
/// "All Metadata" modal as a heading plus (key, value) rows per section.
#[derive(Clone)]
struct MetadataDump {
  sections: Vec<MetadataSection>,
}

#[derive(Clone)]
struct MetadataSection {
  heading: String,
  rows: Vec<(String, String)>,
}

#[derive(Clone, Debug, Default)]
struct FileInfo {
  path: PathBuf,
  filename: String,
  title: String,
  // Multi-valued ID3v2.4 fields kept as separate entries so the table can
  // render them as pills.
  artist: Vec<String>,
  album: String,
  release_date: String,
  genre: Vec<String>,
  comment: String,
  description: String,
  composer: Vec<String>,
  arranger: Vec<String>,
  duration_secs: u64,
  size_bytes: u64,
}

#[derive(Default, Clone, PartialEq)]
struct TagForm {
  title: String,
  // Multi-valued ID3v2.4 text fields, one entry per value (rendered as pills).
  artist: Vec<String>,
  album: String,
  album_artist: Vec<String>,
  date: String,
  // Some(_) only when the file's TDRC and TDRL differ; a second input then
  // appears in the form so both values can be edited independently.
  release_date: Option<String>,
  // Custom TXXX:DATE_ADDED frame (ID3v2 only).
  date_added: String,
  track: String,
  track_total: String,
  disc: String,
  disc_total: String,
  genre: Vec<String>,
  audio_source: String,
  descriptions: Vec<String>,
  comment: String,
  composer: Vec<String>,
  arranger: Vec<String>,
  lyrics: String,
  compilation: bool,
}

impl TagForm {
  /// Returns a copy with surrounding whitespace stripped from every text
  /// field, so stray spaces a user typed aren't persisted to the file.
  /// `lyrics` keeps leading whitespace (only the end is trimmed) to preserve
  /// intentional indentation.
  fn trimmed(&self) -> TagForm {
    TagForm {
      title: self.title.trim().to_string(),
      artist: trim_values(&self.artist),
      album: self.album.trim().to_string(),
      album_artist: trim_values(&self.album_artist),
      date: self.date.trim().to_string(),
      release_date: self.release_date.as_ref().map(|d| d.trim().to_string()),
      date_added: self.date_added.trim().to_string(),
      track: self.track.trim().to_string(),
      track_total: self.track_total.trim().to_string(),
      disc: self.disc.trim().to_string(),
      disc_total: self.disc_total.trim().to_string(),
      genre: trim_values(&self.genre),
      audio_source: self.audio_source.trim().to_string(),
      descriptions: self
        .descriptions
        .iter()
        .map(|d| d.trim().to_string())
        .collect(),
      comment: self.comment.trim().to_string(),
      composer: trim_values(&self.composer),
      arranger: trim_values(&self.arranger),
      lyrics: self.lyrics.trim_end().to_string(),
      compilation: self.compilation,
    }
  }
}

/// Trims each value and drops the ones that end up empty, so blanked pills
/// aren't persisted.
fn trim_values(values: &[String]) -> Vec<String> {
  values
    .iter()
    .map(|v| v.trim().to_string())
    .filter(|v| !v.is_empty())
    .collect()
}

/// True when every entry is blank (or the list is empty) — used to decide
/// whether an ID3v1 value may be copied into the ID3v2 counterpart.
fn pills_blank(values: &[String]) -> bool {
  values.iter().all(|v| v.trim().is_empty())
}

/// A multi-valued text field edited as a row of pills in the sidebar form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PillField {
  Artist,
  AlbumArtist,
  Genre,
  Composer,
  Arranger,
}

impl PillField {
  /// The widget id of this field's text input — also used to map keyboard
  /// focus back to the field (see [`pill_field_for_id`]).
  fn input_id(self) -> &'static str {
    match self {
      PillField::Artist => "artist",
      PillField::AlbumArtist => "field-album-artist",
      PillField::Genre => "field-genre",
      PillField::Composer => "field-composer",
      PillField::Arranger => "field-arranger",
    }
  }
}

/// Separators the "Split" button breaks a single value on when converting it
/// into multiple entries.
const PILL_SEPARATORS: [char; 3] = [',', ';', '/'];

/// Splits `text` into individual values on the single most common
/// [`PILL_SEPARATORS`] separator, trimming each and dropping blanks. Splitting
/// on only one separator keeps compound names intact, e.g. `AC/DC,John,Marc`
/// splits on the two commas into `AC/DC`, `John`, `Marc` rather than also
/// breaking `AC/DC` apart. Ties are broken by [`PILL_SEPARATORS`] order. Only
/// invoked when the user explicitly converts a single value into multiple —
/// values are never split automatically on read.
fn split_into_values(text: &str) -> Vec<String> {
  // Pick the separator with the most occurrences, breaking ties toward the
  // earlier `PILL_SEPARATORS` entry. Falls back to the first separator when
  // none are present, which leaves `text` as a single value.
  // `max_by_key` yields the last maximum, so iterate in reverse to make the
  // earlier `PILL_SEPARATORS` entry win a tie.
  let separator = PILL_SEPARATORS
    .iter()
    .rev()
    .copied()
    .max_by_key(|&sep| text.matches(sep).count())
    .filter(|&sep| text.contains(sep))
    .unwrap_or(PILL_SEPARATORS[0]);

  text
    .split(separator)
    .map(|s| s.trim().to_string())
    .filter(|s| !s.is_empty())
    .collect()
}

/// Whether `text` contains a separator the "Split" button could act on.
fn has_pill_separator(text: &str) -> bool {
  text.contains(PILL_SEPARATORS.as_slice())
}

/// Maps a focused widget id back to its pill field, if any.
fn pill_field_for_id(id: &iced::widget::Id) -> Option<PillField> {
  [
    PillField::Artist,
    PillField::AlbumArtist,
    PillField::Genre,
    PillField::Composer,
    PillField::Arranger,
  ]
  .into_iter()
  .find(|f| *id == iced::widget::Id::new(f.input_id()))
}

/// Identifies which ID3v1 field a "copy to ID3v2" button targets.
#[derive(Debug, Clone, Copy)]
enum Id3v1Field {
  Title,
  Artist,
  Album,
  Year,
  Track,
  Genre,
  Comment,
}

#[derive(Clone)]
struct Id3v1Display {
  title: String,
  artist: String,
  album: String,
  year: String,
  comment: String,
  track: String,
  genre: String,
}

#[derive(Clone)]
struct CoverInfo {
  handle: image::Handle,
  width: u32,
  height: u32,
  size_bytes: usize,
  mime: String,
  pic_type: String,
}

#[derive(Debug, Clone)]
enum Message {
  SelectDirectory,
  DirectoryChosen(Option<PathBuf>),
  FilesChosen(Vec<PathBuf>),
  Reload,
  FilesLoaded(Vec<FileInfo>),
  FileSelected(usize),
  TitleChanged(String),
  TitleFromFilename,
  AlbumChanged(String),
  DateChanged(String),
  ReleaseDateChanged(String),
  DateAddedChanged(String),
  TrackChanged(String),
  DiscChanged(String),
  AudioSourceChanged(String),
  AudioSourceOpenUrl(String),
  CommentAction(text_editor::Action),
  DescriptionAction(usize, text_editor::Action),
  /// Edited a multi-valued field that's currently stored as a single value
  /// (or empty) — kept as a plain string, not split into pills.
  SingleFieldChanged(PillField, String),
  /// Convert a single value containing separators into multiple pill entries.
  PillSplit(PillField),
  /// Typed text changed in a pill field's input; commits any comma-separated
  /// complete segments and keeps the remainder as the draft.
  PillDraftChanged(PillField, String),
  /// Enter pressed in a pill field — commits the current draft as a pill.
  PillSubmit(PillField),
  /// The pill's `×` button was clicked — removes that value.
  PillRemove(PillField, usize),
  /// Backspace pressed; if the focused pill field's draft is empty, drops its
  /// last pill. Resolved against the focused widget id.
  PillBackspace,
  PillBackspaceResolve(iced::widget::Id),
  LyricsAction(text_editor::Action),
  CompilationToggled(bool),
  PlayPauseToggle,
  /// Stop playback of the current track and reset to the start.
  PlaybackStop,
  /// Periodic poll of the playback position while a track is playing.
  PlaybackTick,
  /// The seek bar is being dragged — only previews the position.
  SeekChanged(f64),
  /// The seek bar was released — jumps playback to the chosen position.
  SeekReleased,
  Save,
  Saved(Result<(), String>),
  /// Discard any unsaved edits in the form, reverting to the last saved
  /// snapshot.
  Reset,
  CoverReplace,
  CoverReplaceChosen(Option<PathBuf>),
  CoverDelete,
  CoverExport,
  CoverExportChosen(Option<PathBuf>),
  CoverExported(Result<PathBuf, String>),
  ShowCoverModal,
  HideCoverModal,
  Id3v1Delete,
  Id3v1Deleted(Result<(), String>),
  /// Copies an ID3v1 field's value into the corresponding (empty) ID3v2 form
  /// field.
  Id3v1Copy(Id3v1Field),
  /// Copies every ID3v1 field whose ID3v2 counterpart is still empty.
  Id3v1CopyAll,
  CommentOpenUrl(String),
  DescriptionOpenUrl(String),
  ShowAllMetadata,
  HideAllMetadata,
  /// Right-clicked on a metadata row — opens the copy dropdown pinned to
  /// the last known cursor position.
  OpenCopyMenu {
    key: String,
    value: String,
  },
  /// Closes any open copy menu without copying.
  CloseCopyMenu,
  /// Copies `text` to the system clipboard and closes the menu.
  CopyToClipboard(String),
  /// Right-clicked on a song row — opens the row dropdown pinned to the
  /// last known cursor position.
  OpenSongMenu(usize),
  /// Closes any open song-row menu without acting.
  CloseSongMenu,
  /// Copies the song's filepath to the clipboard and closes the menu.
  SongCopyPath(PathBuf),
  /// Reveals the song in the OS file manager and closes the menu.
  SongRevealInFinder(PathBuf),
  /// Tracks the cursor position while the metadata modal is visible so
  /// [`Message::OpenCopyMenu`] knows where to place the dropdown.
  CursorMoved(Point),
  /// Move selection to the previous/next file in the list.
  SelectPrevious,
  SelectNext,
  /// Focus the artist text input in the sidebar.
  FocusArtist,
  /// Move focus to the next/previous form field.
  FocusNextField,
  FocusPreviousField,
  /// Emacs-style line navigation in the focused input/editor. `select`
  /// extends the selection (Shift held) instead of just moving the caret.
  CursorLineStart {
    select: bool,
  },
  CursorLineEnd {
    select: bool,
  },
  ApplyCursorMotion {
    to_end: bool,
    select: bool,
    id: iced::widget::Id,
  },
  /// Emacs-style forward delete (Ctrl+D) in the focused editor. iced's
  /// text_editor rejects Ctrl+D (its Delete binding requires no accompanying
  /// text, but Ctrl+D carries U+0004), so the multi-line fields need this;
  /// single-line text_input already forward-deletes on Ctrl+D natively.
  DeleteForward,
  ApplyForwardDelete {
    id: iced::widget::Id,
  },
  /// Clears all Album-section fields (album, album artist, track, disc,
  /// compilation flag) so the user can blank them in one click.
  AlbumClear,
  /// Toggle the column-picker dropdown in the listing header.
  ToggleColumnMenu,
  /// Close the column-picker dropdown without acting.
  CloseColumnMenu,
  /// Show / hide a listing column; the change is persisted to the settings
  /// file immediately.
  ToggleColumn(TableColumn),
}

/// Describes a change to the embedded cover picture to apply during a save.
#[derive(Clone)]
enum PictureChange {
  None,
  Replace(PathBuf),
  Delete,
}

impl Taguar {
  /// Clears the current listing and per-file editing state in preparation for
  /// loading a fresh set of files (directory scan, explicit file list, or
  /// reload).
  fn reset_for_load(&mut self) {
    playback_send(PlaybackCmd::Stop);
    self.playing_path = None;
    self.is_paused = false;
    self.files.clear();
    self.selected_idx = None;
    self.nav_warning = false;
    self.form = TagForm::default();
    self.saved_form = TagForm::default();
    self.clear_pill_drafts();
    self.lyrics_content = text_editor::Content::new();
    self.comment_content = text_editor::Content::new();
    self.description_contents = Vec::new();
    self.id3v1 = None;
    self.cover = None;
    self.primary_tag_label.clear();
    self.loading = true;
  }

  fn update(&mut self, message: Message) -> Task<Message> {
    match message {
      Message::SelectDirectory => Task::perform(
        async {
          rfd::AsyncFileDialog::new()
            .set_title("Select a directory of audio files")
            .pick_folder()
            .await
            .map(|handle| handle.path().to_path_buf())
        },
        Message::DirectoryChosen,
      ),
      Message::DirectoryChosen(Some(dir)) => {
        self.reset_for_load();
        self.source = Some(Source::Directory(dir.clone()));
        self.status = Some("Loading...".to_string());
        Task::perform(
          async move {
            tokio::task::spawn_blocking(move || scan_and_load(&dir))
              .await
              .unwrap_or_default()
          },
          Message::FilesLoaded,
        )
      }
      Message::DirectoryChosen(None) => Task::none(),
      Message::FilesChosen(paths) => {
        if paths.is_empty() {
          return Task::none();
        }
        self.reset_for_load();
        self.source = Some(Source::Files(paths.clone()));
        self.status = Some("Loading...".to_string());
        Task::perform(
          async move {
            tokio::task::spawn_blocking(move || load_files(&paths))
              .await
              .unwrap_or_default()
          },
          Message::FilesLoaded,
        )
      }
      Message::Reload => {
        let load: Box<dyn FnOnce() -> Vec<FileInfo> + Send> = match self
          .source
          .clone()
        {
          Some(Source::Directory(dir)) => Box::new(move || scan_and_load(&dir)),
          Some(Source::Files(paths)) => Box::new(move || load_files(&paths)),
          None => return Task::none(),
        };
        self.reset_for_load();
        self.status = Some("Reloading...".to_string());
        Task::perform(
          async move { tokio::task::spawn_blocking(load).await.unwrap_or_default() },
          Message::FilesLoaded,
        )
      }
      Message::FilesLoaded(files) => {
        self.files = files;
        self.loading = false;
        self.status = None;
        if self.selected_idx.is_none() && !self.files.is_empty() {
          self.update(Message::FileSelected(0))
        }
        else {
          Task::none()
        }
      }
      Message::FileSelected(idx) => {
        if self.selected_idx != Some(idx) && self.is_dirty() {
          self.nav_warning = true;
          return Task::none();
        }
        self.nav_warning = false;
        if let Some(info) = self.files.get(idx) {
          let (form, id3v1, label, cover) = load_full(&info.path);
          self.lyrics_content = text_editor::Content::with_text(&form.lyrics);
          self.comment_content = text_editor::Content::with_text(&form.comment);
          self.description_contents = form
            .descriptions
            .iter()
            .map(|d| text_editor::Content::with_text(d))
            .collect();
          self.form = form.clone();
          self.saved_form = form;
          self.clear_pill_drafts();
          self.id3v1 = id3v1;
          self.primary_tag_label = label;
          self.cover = cover;
          self.selected_idx = Some(idx);
          self.status = None;
        }
        Task::none()
      }
      Message::SelectPrevious => {
        let idx = match self.selected_idx {
          Some(i) if i > 0 => i - 1,
          None if !self.files.is_empty() => 0,
          _ => return Task::none(),
        };
        self.update(Message::FileSelected(idx))
      }
      Message::SelectNext => {
        let idx = match self.selected_idx {
          Some(i) if i + 1 < self.files.len() => i + 1,
          None if !self.files.is_empty() => 0,
          _ => return Task::none(),
        };
        self.update(Message::FileSelected(idx))
      }
      Message::FocusArtist => {
        if self.selected_idx.is_some() {
          iced::widget::operation::focus(iced::widget::Id::new("artist"))
        }
        else {
          Task::none()
        }
      }
      Message::FocusNextField => iced::widget::operation::focus_next(),
      Message::FocusPreviousField => iced::widget::operation::focus_previous(),
      Message::CursorLineStart { select } => iced::advanced::widget::operate(
        iced::advanced::widget::operation::focusable::find_focused(),
      )
      .map(move |id| Message::ApplyCursorMotion {
        to_end: false,
        select,
        id,
      }),
      Message::CursorLineEnd { select } => iced::advanced::widget::operate(
        iced::advanced::widget::operation::focusable::find_focused(),
      )
      .map(move |id| Message::ApplyCursorMotion {
        to_end: true,
        select,
        id,
      }),
      Message::ApplyCursorMotion { to_end, select, id } => {
        use iced::widget::text_editor::{Action, Motion};
        let motion = if to_end { Motion::End } else { Motion::Home };
        let action = if select {
          Action::Select(motion)
        }
        else {
          Action::Move(motion)
        };
        if id == iced::widget::Id::new("editor-lyrics") {
          self.lyrics_content.perform(action);
          return Task::none();
        }
        if id == iced::widget::Id::new("editor-comment") {
          self.comment_content.perform(action);
          return Task::none();
        }
        for (i, c) in self.description_contents.iter_mut().enumerate() {
          if id == iced::widget::Id::from(format!("editor-description-{i}")) {
            c.perform(action);
            return Task::none();
          }
        }
        // Single-line text_input: iced's operation API exposes select_range but
        // not the current caret, so a cursor-relative selection can't be
        // anchored here. Shift therefore falls back to a plain caret move for
        // these fields (the text_editor branches above handle real selection).
        let _ = select;
        if to_end {
          iced::widget::operation::move_cursor_to_end(id)
        }
        else {
          iced::widget::operation::move_cursor_to_front(id)
        }
      }
      Message::DeleteForward => iced::advanced::widget::operate(
        iced::advanced::widget::operation::focusable::find_focused(),
      )
      .map(|id| Message::ApplyForwardDelete { id }),
      Message::ApplyForwardDelete { id } => {
        use iced::widget::text_editor::{Action, Edit};
        let action = Action::Edit(Edit::Delete);
        if id == iced::widget::Id::new("editor-lyrics") {
          self.lyrics_content.perform(action);
          return Task::none();
        }
        if id == iced::widget::Id::new("editor-comment") {
          self.comment_content.perform(action);
          return Task::none();
        }
        for (i, c) in self.description_contents.iter_mut().enumerate() {
          if id == iced::widget::Id::from(format!("editor-description-{i}")) {
            c.perform(action);
            return Task::none();
          }
        }
        // Single-line text_input already handles Ctrl+D natively, so any
        // non-editor focus is a no-op here.
        Task::none()
      }
      Message::AlbumClear => {
        self.form.album.clear();
        self.form.album_artist.clear();
        self.album_artist_draft.clear();
        self.form.track.clear();
        self.form.track_total.clear();
        self.form.disc.clear();
        self.form.disc_total.clear();
        self.form.compilation = false;
        Task::none()
      }
      Message::Id3v1Copy(field) => {
        if let Some(v1) = &self.id3v1 {
          match field {
            Id3v1Field::Title => self.form.title = v1.title.clone(),
            Id3v1Field::Artist => self.form.artist = vec![v1.artist.clone()],
            Id3v1Field::Album => self.form.album = v1.album.clone(),
            Id3v1Field::Year => self.form.date = v1.year.clone(),
            Id3v1Field::Track => self.form.track = v1.track.clone(),
            Id3v1Field::Genre => self.form.genre = vec![v1.genre.clone()],
            Id3v1Field::Comment => {
              self.form.comment = v1.comment.clone();
              self.comment_content =
                text_editor::Content::with_text(&v1.comment);
            }
          }
        }
        Task::none()
      }
      Message::Id3v1CopyAll => {
        if let Some(v1) = self.id3v1.clone() {
          // Only fill empty ID3v2 fields so existing values are never
          // clobbered, matching the per-field copy buttons.
          if pills_blank(&self.form.artist) && !v1.artist.is_empty() {
            self.form.artist = vec![v1.artist];
          }
          if self.form.title.trim().is_empty() {
            self.form.title = v1.title;
          }
          if self.form.album.trim().is_empty() {
            self.form.album = v1.album;
          }
          if self.form.date.trim().is_empty() {
            self.form.date = v1.year;
          }
          if self.form.track.trim().is_empty() {
            self.form.track = v1.track;
          }
          if pills_blank(&self.form.genre) && !v1.genre.is_empty() {
            self.form.genre = vec![v1.genre];
          }
          if self.form.comment.trim().is_empty() {
            self.form.comment = v1.comment.clone();
            self.comment_content = text_editor::Content::with_text(&v1.comment);
          }
        }
        Task::none()
      }
      Message::TitleChanged(v) => {
        self.form.title = v;
        Task::none()
      }
      Message::TitleFromFilename => {
        if let Some(file) = self.selected_idx.and_then(|i| self.files.get(i)) {
          if let Some(title) = title_from_filename(&file.filename) {
            self.form.title = title;
          }
        }
        Task::none()
      }
      Message::AlbumChanged(v) => {
        self.form.album = v;
        Task::none()
      }
      Message::DateChanged(v) => {
        // Recording-date edit: diverging from the release date splits the
        // unified TDRC/TDRL pair; matching values re-merge into one.
        if self.form.release_date.is_none() {
          self.form.release_date = Some(self.form.date.clone());
        }
        self.form.date = v;
        if self.form.release_date.as_deref() == Some(self.form.date.as_str()) {
          self.form.release_date = None;
        }
        Task::none()
      }
      Message::ReleaseDateChanged(v) => {
        match &self.form.release_date {
          // Unified: edit the shared value so TDRC stays mirrored on save.
          None => self.form.date = v,
          Some(_) if v == self.form.date => self.form.release_date = None,
          Some(_) => self.form.release_date = Some(v),
        }
        Task::none()
      }
      Message::DateAddedChanged(v) => {
        self.form.date_added = v;
        Task::none()
      }
      Message::TrackChanged(v) => {
        self.form.track = v;
        Task::none()
      }
      Message::DiscChanged(v) => {
        self.form.disc = v;
        Task::none()
      }
      Message::SingleFieldChanged(field, value) => {
        // Stored as a single string (no auto-splitting); empty clears it.
        let pills = self.pill_values_mut(field);
        pills.clear();
        if !value.is_empty() {
          pills.push(value);
        }
        Task::none()
      }
      Message::PillSplit(field) => {
        let current =
          self.pill_values(field).first().cloned().unwrap_or_default();
        *self.pill_values_mut(field) = split_into_values(&current);
        Task::none()
      }
      Message::PillDraftChanged(field, value) => {
        self.pill_draft_changed(field, value);
        Task::none()
      }
      Message::PillSubmit(field) => {
        self.commit_pill_draft(field);
        Task::none()
      }
      Message::PillRemove(field, idx) => {
        let pills = self.pill_values_mut(field);
        if idx < pills.len() {
          pills.remove(idx);
        }
        Task::none()
      }
      Message::PillBackspace => iced::advanced::widget::operate(
        iced::advanced::widget::operation::focusable::find_focused(),
      )
      .map(Message::PillBackspaceResolve),
      Message::PillBackspaceResolve(id) => {
        // Runs after the keystroke's draft edit (it arrives via an `operate`
        // round-trip), so an empty draft here without suppression means the
        // input was already empty: drop the preceding pill.
        if let Some(field) = pill_field_for_id(&id) {
          // Only act in pills mode (>= 2 entries); a single value is edited as
          // a plain string and must not be cleared by Backspace.
          if self.pill_values(field).len() >= 2
            && self.pill_draft(field).is_empty()
            && !self.pill_backspace_suppressed
          {
            self.pill_values_mut(field).pop();
          }
        }
        self.pill_backspace_suppressed = false;
        Task::none()
      }
      Message::AudioSourceChanged(v) => {
        self.form.audio_source = v;
        Task::none()
      }
      Message::AudioSourceOpenUrl(url) => {
        open_url(&url);
        Task::none()
      }
      Message::CommentAction(action) => {
        let is_edit = action.is_edit();
        self.comment_content.perform(action);
        if is_edit {
          self.form.comment = self
            .comment_content
            .text()
            .trim_end_matches('\n')
            .to_string();
        }
        Task::none()
      }
      Message::DescriptionAction(idx, action) => {
        if let Some(content) = self.description_contents.get_mut(idx) {
          let is_edit = action.is_edit();
          content.perform(action);
          if is_edit {
            if let Some(s) = self.form.descriptions.get_mut(idx) {
              *s = content.text().trim_end_matches('\n').to_string();
            }
          }
        }
        Task::none()
      }
      Message::LyricsAction(action) => {
        let is_edit = action.is_edit();
        self.lyrics_content.perform(action);
        if is_edit {
          self.form.lyrics = self
            .lyrics_content
            .text()
            .trim_end_matches('\n')
            .to_string();
        }
        Task::none()
      }
      Message::CompilationToggled(v) => {
        self.form.compilation = v;
        Task::none()
      }
      Message::PlayPauseToggle => {
        if let Some(idx) = self.selected_idx {
          let path = self.files[idx].path.clone();
          if self.playing_path.as_ref() == Some(&path) {
            if self.is_paused {
              playback_send(PlaybackCmd::Resume);
              self.is_paused = false;
            }
            else {
              playback_send(PlaybackCmd::Pause);
              self.is_paused = true;
            }
          }
          else {
            // Clear any leftover state from a previous track before the
            // worker starts publishing the new one's position.
            PLAYBACK_FINISHED.store(false, Ordering::Relaxed);
            PLAYBACK_POS_MS.store(0, Ordering::Relaxed);
            playback_send(PlaybackCmd::Play(path.clone()));
            self.playing_path = Some(path);
            self.is_paused = false;
            self.playback_pos_secs = 0.0;
            self.seek_drag_secs = None;
          }
        }
        Task::none()
      }
      Message::PlaybackStop => {
        playback_send(PlaybackCmd::Stop);
        self.playing_path = None;
        self.is_paused = false;
        self.playback_pos_secs = 0.0;
        self.seek_drag_secs = None;
        Task::none()
      }
      Message::PlaybackTick => {
        if PLAYBACK_FINISHED.swap(false, Ordering::Relaxed) {
          self.playing_path = None;
          self.is_paused = false;
          self.playback_pos_secs = 0.0;
          self.seek_drag_secs = None;
        }
        else if self.seek_drag_secs.is_none() {
          self.playback_pos_secs =
            PLAYBACK_POS_MS.load(Ordering::Relaxed) as f64 / 1000.0;
        }
        Task::none()
      }
      Message::SeekChanged(secs) => {
        // The bar is a no-op placeholder while the selected file isn't the
        // active track.
        if self.selected_is_active() {
          self.seek_drag_secs = Some(secs);
        }
        Task::none()
      }
      Message::SeekReleased => {
        if let Some(secs) = self.seek_drag_secs.take() {
          self.playback_pos_secs = secs;
          playback_send(PlaybackCmd::Seek(Duration::from_secs_f64(secs)));
        }
        Task::none()
      }
      Message::Save => self.spawn_save(PictureChange::None, "Saving..."),
      Message::Saved(Ok(())) => {
        self.status = Some("Saved.".to_string());
        self.nav_warning = false;
        if let Some(idx) = self.selected_idx {
          let path = self.files[idx].path.clone();
          // Refresh editable form + cover.
          let (form, id3v1, label, cover) = load_full(&path);
          self.lyrics_content = text_editor::Content::with_text(&form.lyrics);
          self.comment_content = text_editor::Content::with_text(&form.comment);
          self.description_contents = form
            .descriptions
            .iter()
            .map(|d| text_editor::Content::with_text(d))
            .collect();
          self.form = form.clone();
          self.saved_form = form;
          self.clear_pill_drafts();
          self.id3v1 = id3v1;
          self.primary_tag_label = label;
          self.cover = cover;
          // Refresh the file's row in the table.
          if let Ok(mut info) = load_file_info(&path) {
            if let Some(Source::Directory(root)) = &self.source {
              info.filename = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .into_owned();
            }
            self.files[idx] = info;
          }
        }
        Task::none()
      }
      Message::Saved(Err(e)) => {
        self.status = Some(format!("Error: {e}"));
        Task::none()
      }
      Message::Reset => {
        self.form = self.saved_form.clone();
        self.clear_pill_drafts();
        self.lyrics_content =
          text_editor::Content::with_text(&self.saved_form.lyrics);
        self.comment_content =
          text_editor::Content::with_text(&self.saved_form.comment);
        self.description_contents = self
          .saved_form
          .descriptions
          .iter()
          .map(|d| text_editor::Content::with_text(d))
          .collect();
        self.nav_warning = false;
        self.status = None;
        Task::none()
      }
      Message::CoverReplace => Task::perform(
        async {
          rfd::AsyncFileDialog::new()
            .set_title("Choose cover image")
            .add_filter(
              "Image",
              &["png", "jpg", "jpeg", "gif", "bmp", "tiff", "tif"],
            )
            .pick_file()
            .await
            .map(|h| h.path().to_path_buf())
        },
        Message::CoverReplaceChosen,
      ),
      Message::CoverReplaceChosen(None) => Task::none(),
      Message::CoverReplaceChosen(Some(img_path)) => {
        let status = if self.cover.is_some() {
          "Updating cover..."
        }
        else {
          "Adding cover..."
        };
        self.spawn_save(PictureChange::Replace(img_path), status)
      }
      Message::CoverDelete => {
        self.spawn_save(PictureChange::Delete, "Deleting cover...")
      }
      Message::CoverExport => {
        let Some(idx) = self.selected_idx else {
          return Task::none();
        };
        let Some(cov) = &self.cover else {
          return Task::none();
        };
        let file_info = &self.files[idx];
        let stem = Path::new(&file_info.filename)
          .file_stem()
          .map(|s| s.to_string_lossy().to_string())
          .unwrap_or_else(|| "cover".to_string());
        let ext = mime_to_extension(&cov.mime);
        let default_name = format!("{stem}-cover.{ext}");
        let start_dir = file_info
          .path
          .parent()
          .map(|p| p.to_path_buf())
          .unwrap_or_else(|| PathBuf::from("."));
        Task::perform(
          async move {
            rfd::AsyncFileDialog::new()
              .set_title("Export cover image")
              .set_file_name(&default_name)
              .set_directory(&start_dir)
              .save_file()
              .await
              .map(|h| h.path().to_path_buf())
          },
          Message::CoverExportChosen,
        )
      }
      Message::CoverExportChosen(None) => Task::none(),
      Message::CoverExportChosen(Some(dest)) => {
        let Some(idx) = self.selected_idx else {
          return Task::none();
        };
        let src = self.files[idx].path.clone();
        self.status = Some("Exporting cover...".to_string());
        Task::perform(
          async move {
            tokio::task::spawn_blocking(move || export_cover(&src, &dest))
              .await
              .map_err(|e| e.to_string())
              .and_then(|r| r)
          },
          Message::CoverExported,
        )
      }
      Message::CoverExported(Ok(path)) => {
        self.status = Some(format!("Exported cover to {}", path.display()));
        Task::none()
      }
      Message::CoverExported(Err(e)) => {
        self.status = Some(format!("Error exporting cover: {e}"));
        Task::none()
      }
      Message::ShowCoverModal => {
        if self.cover.is_some() {
          self.cover_modal_open = true;
        }
        Task::none()
      }
      Message::HideCoverModal => {
        self.cover_modal_open = false;
        Task::none()
      }
      Message::Id3v1Delete => {
        let Some(idx) = self.selected_idx else {
          return Task::none();
        };
        let path = self.files[idx].path.clone();
        self.status = Some("Deleting ID3v1 tag...".to_string());
        Task::perform(
          async move {
            tokio::task::spawn_blocking(move || delete_id3v1_tag(&path))
              .await
              .map_err(|e| e.to_string())
              .and_then(|r| r)
          },
          Message::Id3v1Deleted,
        )
      }
      Message::Id3v1Deleted(Ok(())) => {
        self.status = Some("ID3v1 tag deleted.".to_string());
        self.id3v1 = None;
        // Refresh metadata dump if open.
        if let Some(idx) = self.selected_idx {
          if self.metadata_dump.is_some() {
            self.metadata_dump =
              Some(load_metadata_dump(&self.files[idx].path));
          }
        }
        Task::none()
      }
      Message::Id3v1Deleted(Err(e)) => {
        self.status = Some(format!("Error deleting ID3v1: {e}"));
        Task::none()
      }
      Message::CommentOpenUrl(url) => {
        open_url(&url);
        Task::none()
      }
      Message::DescriptionOpenUrl(url) => {
        open_url(&url);
        Task::none()
      }
      Message::ShowAllMetadata => {
        if let Some(idx) = self.selected_idx {
          if let Some(info) = self.files.get(idx) {
            self.metadata_dump = Some(load_metadata_dump(&info.path));
          }
        }
        Task::none()
      }
      Message::HideAllMetadata => {
        self.metadata_dump = None;
        self.copy_menu = None;
        self.copy_feedback = None;
        self.last_cursor = None;
        Task::none()
      }
      Message::OpenCopyMenu { key, value } => {
        // Anchor the dropdown to the latest known cursor position; fall back
        // to (0, 0) if we somehow haven't seen a move event yet.
        let at = self.last_cursor.unwrap_or(Point::ORIGIN);
        self.copy_menu = Some(CopyMenu { at, key, value });
        Task::none()
      }
      Message::CloseCopyMenu => {
        self.copy_menu = None;
        Task::none()
      }
      Message::CopyToClipboard(text) => {
        self.copy_menu = None;
        let preview = if text.chars().count() > 48 {
          let head: String = text.chars().take(48).collect();
          format!("Copied: {head}…")
        }
        else {
          format!("Copied: {text}")
        };
        self.copy_feedback = Some(preview);
        iced::clipboard::write(text)
      }
      Message::CursorMoved(position) => {
        self.last_cursor = Some(position);
        Task::none()
      }
      Message::OpenSongMenu(idx) => {
        // Anchor the dropdown to the latest known cursor position; fall back
        // to (0, 0) if we somehow haven't seen a move event yet.
        if let Some(info) = self.files.get(idx) {
          let at = self.last_cursor.unwrap_or(Point::ORIGIN);
          self.song_menu = Some(SongMenu {
            at,
            path: info.path.clone(),
          });
        }
        Task::none()
      }
      Message::CloseSongMenu => {
        self.song_menu = None;
        Task::none()
      }
      Message::SongCopyPath(path) => {
        self.song_menu = None;
        let text = path.to_string_lossy().into_owned();
        self.status = Some(format!("Copied path: {text}"));
        iced::clipboard::write(text)
      }
      Message::SongRevealInFinder(path) => {
        self.song_menu = None;
        reveal_in_file_manager(&path);
        Task::none()
      }
      Message::ToggleColumnMenu => {
        // Anchor the dropdown to the button via the latest cursor position so
        // it drops down right under the click.
        self.column_menu = if self.column_menu.is_some() {
          None
        }
        else {
          Some(self.last_cursor.unwrap_or(Point::ORIGIN))
        };
        Task::none()
      }
      Message::CloseColumnMenu => {
        self.column_menu = None;
        Task::none()
      }
      Message::ToggleColumn(column) => {
        self.settings.toggle_column(column);
        self.settings.save();
        Task::none()
      }
    }
  }

  /// Only subscribes to cursor events when the metadata modal is open — so
  /// the rest of the app isn't paying for per-pixel messages. Likewise the
  /// playback-position tick only runs while a track is actually playing.
  fn subscription(&self) -> Subscription<Message> {
    let keyboard_sub = event::listen_with(|event, status, _window| {
      let captured = matches!(status, event::Status::Captured);
      match event {
        Event::Keyboard(keyboard::Event::KeyPressed {
          key: keyboard::Key::Named(keyboard::key::Named::ArrowUp),
          ..
        }) if !captured => Some(Message::SelectPrevious),
        Event::Keyboard(keyboard::Event::KeyPressed {
          key: keyboard::Key::Named(keyboard::key::Named::ArrowDown),
          ..
        }) if !captured => Some(Message::SelectNext),
        Event::Keyboard(keyboard::Event::KeyPressed {
          key: keyboard::Key::Named(keyboard::key::Named::ArrowRight),
          ..
        }) if !captured => Some(Message::FocusArtist),
        Event::Keyboard(keyboard::Event::KeyPressed {
          key: keyboard::Key::Named(keyboard::key::Named::Tab),
          modifiers,
          ..
        }) => {
          if modifiers.shift() {
            Some(Message::FocusPreviousField)
          }
          else {
            Some(Message::FocusNextField)
          }
        }
        Event::Keyboard(keyboard::Event::KeyPressed {
          key: keyboard::Key::Named(keyboard::key::Named::Escape),
          ..
        }) => Some(Message::HideCoverModal),
        // Backspace in an empty pill input deletes the preceding pill. The
        // focused field (and whether its draft is empty) is resolved in the
        // handler; for every other widget this is a no-op.
        Event::Keyboard(keyboard::Event::KeyPressed {
          key: keyboard::Key::Named(keyboard::key::Named::Backspace),
          modifiers,
          ..
        }) if !modifiers.control() && !modifiers.alt() && !modifiers.logo() => {
          Some(Message::PillBackspace)
        }
        Event::Keyboard(keyboard::Event::KeyPressed {
          ref key,
          modifiers,
          ..
        }) if modifiers.control() && !modifiers.alt() && !modifiers.logo() => {
          let select = modifiers.shift();
          match key.as_ref() {
            keyboard::Key::Character(c) if c.eq_ignore_ascii_case("a") => {
              Some(Message::CursorLineStart { select })
            }
            keyboard::Key::Character(c) if c.eq_ignore_ascii_case("e") => {
              Some(Message::CursorLineEnd { select })
            }
            keyboard::Key::Character(c) if c.eq_ignore_ascii_case("d") => {
              Some(Message::DeleteForward)
            }
            _ => None,
          }
        }
        _ => None,
      }
    });

    let mut subs = vec![keyboard_sub];

    // Track the cursor whenever a right-click dropdown could be opened: the
    // metadata modal's copy menu, or the song listing's row menu (shown once
    // a directory is loaded).
    if self.metadata_dump.is_some() || self.source.is_some() {
      subs.push(event::listen_with(|event, _status, _window| match event {
        Event::Mouse(mouse::Event::CursorMoved { position }) => {
          Some(Message::CursorMoved(position))
        }
        _ => None,
      }));
    }

    // Keep the seek bar moving while a track plays.
    if self.playing_path.is_some() && !self.is_paused {
      subs.push(
        iced::time::every(Duration::from_millis(250))
          .map(|_| Message::PlaybackTick),
      );
    }

    Subscription::batch(subs)
  }

  /// Whether the selected file is the active (playing or paused) track.
  fn selected_is_active(&self) -> bool {
    match (
      &self.playing_path,
      self.selected_idx.and_then(|i| self.files.get(i)),
    ) {
      (Some(playing), Some(selected)) => *playing == selected.path,
      _ => false,
    }
  }

  /// Kicks off a background save that applies the current form plus an
  /// optional picture change, reporting completion via [`Message::Saved`].
  fn pill_values(&self, field: PillField) -> &Vec<String> {
    match field {
      PillField::Artist => &self.form.artist,
      PillField::AlbumArtist => &self.form.album_artist,
      PillField::Genre => &self.form.genre,
      PillField::Composer => &self.form.composer,
      PillField::Arranger => &self.form.arranger,
    }
  }

  fn pill_values_mut(&mut self, field: PillField) -> &mut Vec<String> {
    match field {
      PillField::Artist => &mut self.form.artist,
      PillField::AlbumArtist => &mut self.form.album_artist,
      PillField::Genre => &mut self.form.genre,
      PillField::Composer => &mut self.form.composer,
      PillField::Arranger => &mut self.form.arranger,
    }
  }

  fn pill_draft(&self, field: PillField) -> &str {
    match field {
      PillField::Artist => &self.artist_draft,
      PillField::AlbumArtist => &self.album_artist_draft,
      PillField::Genre => &self.genre_draft,
      PillField::Composer => &self.composer_draft,
      PillField::Arranger => &self.arranger_draft,
    }
  }

  fn pill_draft_mut(&mut self, field: PillField) -> &mut String {
    match field {
      PillField::Artist => &mut self.artist_draft,
      PillField::AlbumArtist => &mut self.album_artist_draft,
      PillField::Genre => &mut self.genre_draft,
      PillField::Composer => &mut self.composer_draft,
      PillField::Arranger => &mut self.arranger_draft,
    }
  }

  /// Pushes `text` as a single pill onto `field` (trimmed), if non-empty.
  /// Separators inside `text` are kept verbatim — only the explicit "Split"
  /// button breaks a value into multiple entries.
  fn commit_pill_text(&mut self, field: PillField, text: &str) {
    let trimmed = text.trim();
    if !trimmed.is_empty() {
      self.pill_values_mut(field).push(trimmed.to_string());
    }
  }

  /// Commits the current draft as one or more pills and clears the draft.
  fn commit_pill_draft(&mut self, field: PillField) {
    let text = std::mem::take(self.pill_draft_mut(field));
    self.commit_pill_text(field, &text);
  }

  /// Handles typed input: any complete comma-separated segment becomes a pill,
  /// and the trailing fragment stays in the draft for further typing.
  fn pill_draft_changed(&mut self, field: PillField, value: String) {
    if let Some(last_comma) = value.rfind(',') {
      let (committed, remainder) = value.split_at(last_comma);
      for segment in committed.split(',') {
        self.commit_pill_text(field, segment);
      }
      // `remainder` still starts with the comma; drop it.
      *self.pill_draft_mut(field) = remainder[1..].trim_start().to_string();
    }
    else {
      *self.pill_draft_mut(field) = value;
    }
    // If this edit left the input empty, the same keystroke must not also pop
    // a pill (the user was deleting a character, not the preceding pill).
    self.pill_backspace_suppressed = self.pill_draft(field).is_empty();
  }

  /// Folds every non-empty pill draft into its pill list. Called before a save
  /// so text typed but not yet committed isn't lost.
  fn commit_pill_drafts(&mut self) {
    for field in [
      PillField::Artist,
      PillField::AlbumArtist,
      PillField::Genre,
      PillField::Composer,
      PillField::Arranger,
    ] {
      self.commit_pill_draft(field);
    }
  }

  fn clear_pill_drafts(&mut self) {
    self.artist_draft.clear();
    self.album_artist_draft.clear();
    self.genre_draft.clear();
    self.composer_draft.clear();
    self.arranger_draft.clear();
    self.pill_backspace_suppressed = false;
  }

  fn has_pending_pill_drafts(&self) -> bool {
    [
      &self.artist_draft,
      &self.album_artist_draft,
      &self.genre_draft,
      &self.composer_draft,
      &self.arranger_draft,
    ]
    .iter()
    .any(|d| !d.trim().is_empty())
  }

  /// True when the form differs from the last saved snapshot, including any
  /// uncommitted pill draft.
  fn is_dirty(&self) -> bool {
    self.form != self.saved_form || self.has_pending_pill_drafts()
  }

  /// Renders a multi-valued field. With two or more stored entries it shows a
  /// wrapping row of pills plus an input (Enter or comma adds a pill); with a
  /// single value (or none) it shows a plain text input, plus a "Split" button
  /// when that value contains a separator that could break it into entries.
  fn pill_input_view(
    &self,
    label_text: &'static str,
    field: PillField,
  ) -> Element<'_, Message> {
    let label = text(label_text).size(11).color(MUTED);
    let pills = self.pill_values(field);
    if pills.len() >= 2 {
      let mut wrap = Row::new().spacing(4).align_y(Alignment::Center);
      for (idx, value) in pills.iter().enumerate() {
        wrap = wrap.push(pill_chip(value, field, idx));
      }
      wrap = wrap.push(
        text_input("", self.pill_draft(field))
          .id(iced::widget::Id::new(field.input_id()))
          .on_input(move |v| Message::PillDraftChanged(field, v))
          .on_submit(Message::PillSubmit(field))
          .size(12)
          .padding(4)
          .width(Length::Fixed(130.0)),
      );
      return column![label, wrap.wrap()].spacing(2).into();
    }

    let value = pills.first().map(String::as_str).unwrap_or("");
    let input = text_input("", value)
      .id(iced::widget::Id::new(field.input_id()))
      .on_input(move |v| Message::SingleFieldChanged(field, v))
      .size(12)
      .padding(4);
    // Always wrap the input in a row so appending the "Split" button doesn't
    // shift the input's position in the widget tree. iced reconciles widget
    // state by tree position; moving the input would rebuild it with fresh
    // state and drop keyboard focus mid-typing.
    let mut body = row![input].spacing(4).align_y(Alignment::Center);
    if has_pill_separator(value) {
      body = body.push(
        button(text("Split").size(11))
          .on_press(Message::PillSplit(field))
          .padding([4, 8]),
      );
    }
    column![label, body].spacing(2).into()
  }

  fn spawn_save(
    &mut self,
    pic_change: PictureChange,
    status: &str,
  ) -> Task<Message> {
    self.commit_pill_drafts();
    let Some(idx) = self.selected_idx else {
      return Task::none();
    };
    let path = self.files[idx].path.clone();
    let form = self.form.clone();
    self.status = Some(status.to_string());
    Task::perform(
      async move {
        tokio::task::spawn_blocking(move || save_tags(&path, &form, pic_change))
          .await
          .map_err(|e| e.to_string())
          .and_then(|r| r)
      },
      Message::Saved,
    )
  }

  fn view(&self) -> Element<'_, Message> {
    if self.source.is_none() {
      return container(
        button(text("Select Directory").size(16))
          .on_press(Message::SelectDirectory)
          .padding([10, 22]),
      )
      .center_x(Length::Fill)
      .center_y(Length::Fill)
      .into();
    }

    let header = self.header_view();
    let table = self.table_view();
    let sidebar = self.sidebar_view();
    let status = self.status_bar_view();

    let left: Element<Message> = column![
      header,
      container(table)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(panel_style),
      container(status)
        .padding([4, 10])
        .width(Length::Fill)
        .style(status_bar_style),
    ]
    .into();

    let body: Element<Message> = row![
      container(left)
        .width(Length::FillPortion(7))
        .height(Length::Fill),
      container(sidebar)
        .width(Length::FillPortion(3))
        .height(Length::Fill)
        .style(sidebar_style)
        .padding(Padding::new(10.0).right(0.0)),
    ]
    .height(Length::Fill)
    .into();

    let mut layered: Element<Message> = body;
    if let Some(dump) = &self.metadata_dump {
      layered = stack![layered, self.metadata_modal_view(dump)].into();
    }
    if self.cover_modal_open {
      if let Some(cov) = &self.cover {
        layered = stack![layered, self.cover_modal_view(cov)].into();
      }
    }
    // Always stack a song-menu layer (an empty `Space` when closed) so
    // opening / closing the menu doesn't change the widget tree and reset
    // the listing scrollable's position.
    let song_overlay: Element<Message> = match &self.song_menu {
      Some(menu) => self.song_menu_view(menu),
      None => Space::new().into(),
    };
    layered = stack![layered, song_overlay].into();

    if let Some(at) = self.column_menu {
      layered = stack![layered, self.column_menu_view(at)].into();
    }
    layered
  }

  /// Renders the floating right-click dropdown for a song row at `menu.at`,
  /// mirroring [`copy_menu_view`](Self::copy_menu_view): a transparent
  /// full-window scrim closes the menu on any outside click.
  fn song_menu_view(&self, menu: &SongMenu) -> Element<'_, Message> {
    let panel = container(
      column![
        button(text("Copy Filepath").size(12))
          .on_press(Message::SongCopyPath(menu.path.clone()))
          .padding([4, 12])
          .width(Length::Fill)
          .style(menu_item_style),
        button(text(REVEAL_LABEL).size(12))
          .on_press(Message::SongRevealInFinder(menu.path.clone()))
          .padding([4, 12])
          .width(Length::Fill)
          .style(menu_item_style),
      ]
      .spacing(2),
    )
    .padding(4)
    .width(Length::Fixed(180.0))
    .style(menu_panel_style);

    let dismiss = mouse_area(
      container(Space::new())
        .width(Length::Fill)
        .height(Length::Fill),
    )
    .on_press(Message::CloseSongMenu)
    .on_right_press(Message::CloseSongMenu);

    let x = menu.at.x.max(0.0);
    let y = menu.at.y.max(0.0);
    let positioned = column![
      Space::new().height(Length::Fixed(y)),
      row![Space::new().width(Length::Fixed(x)), opaque(panel)],
    ];

    stack![dismiss, positioned].into()
  }

  fn nav_warning_banner(&self) -> Element<'_, Message> {
    container(
      text(
        "Unsaved changes — save the current song before switching to another.",
      )
      .size(14)
      .font(BOLD)
      .color(Color::WHITE),
    )
    .padding([10, 16])
    .width(Length::Fill)
    .style(warning_banner_style)
    .into()
  }

  fn header_view(&self) -> Element<'_, Message> {
    container(
      row![
        button(text("Change Directory").size(12))
          .on_press(Message::SelectDirectory)
          .padding([4, 10]),
        button(text("Reload").size(12))
          .on_press(Message::Reload)
          .padding([4, 10]),
        // Push the column picker to the right edge of the toolbar.
        Space::new().width(Length::Fill),
        button(text("Columns").size(12))
          .on_press(Message::ToggleColumnMenu)
          .padding([4, 10]),
      ]
      .spacing(10)
      .align_y(Alignment::Center),
    )
    .padding([6, 10])
    .width(Length::Fill)
    .style(header_bar_style)
    .into()
  }

  fn table_view(&self) -> Element<'_, Message> {
    // Which columns to show is user-configurable (persisted in settings);
    // rendering always follows the canonical `TableColumn::ALL` order. The
    // proportional weights make the columns stretch to fill the width.
    let columns: Vec<TableColumn> = TableColumn::ALL
      .into_iter()
      .filter(|c| self.settings.is_visible(*c))
      .collect();

    let mut header_inner =
      iced::widget::Row::new().spacing(10).padding([6, 10]);
    for col in &columns {
      header_inner = header_inner.push(
        text(col.label())
          .size(12)
          .font(BOLD)
          .width(Length::FillPortion(col.weight()))
          .color(MUTED),
      );
    }
    let header_row = container(header_inner)
      .width(Length::Fill)
      .style(table_header_style);

    if self.loading {
      let body = container(text("Loading..."))
        .center_x(Length::Fill)
        .center_y(Length::Fill);
      return column![header_row, body].into();
    }

    if self.files.is_empty() {
      let body = container(text("No audio files found.").color(MUTED))
        .center_x(Length::Fill)
        .center_y(Length::Fill);
      return column![header_row, body].into();
    }

    let rows = self.files.iter().enumerate().map(|(idx, info)| {
      let selected = self.selected_idx == Some(idx);
      let alt = idx % 2 == 1;

      let mut cells = iced::widget::Row::new().spacing(10);
      for col in &columns {
        let cell: Element<Message> = match col.multi_values(info) {
          Some(values) => table_value_view(values),
          None => text(col.cell_text(info)).size(12).into(),
        };
        cells =
          cells.push(container(cell).width(Length::FillPortion(col.weight())));
      }

      let style: fn(&Theme, button::Status) -> button::Style = if selected {
        selected_row_style
      }
      else if alt {
        alt_row_style
      }
      else {
        plain_row_style
      };

      let row_button = button(cells)
        .on_press(Message::FileSelected(idx))
        .width(Length::Fill)
        .padding([4, 10])
        .style(style);

      mouse_area(row_button)
        .on_right_press(Message::OpenSongMenu(idx))
        .into()
    });

    let body = scrollable(Column::with_children(rows).spacing(0))
      .height(Length::Fill)
      .width(Length::Fill);

    column![header_row, body].into()
  }

  /// Floating dropdown listing every column with a checkbox for visibility.
  /// Anchored to `at` (the cursor position when the button was clicked) so it
  /// drops down just below the "Columns" button; a transparent full-window
  /// scrim closes it on any outside click.
  fn column_menu_view(&self, at: Point) -> Element<'_, Message> {
    let mut items = Column::new().spacing(2);
    for col in TableColumn::ALL {
      items = items.push(
        checkbox(self.settings.is_visible(col))
          .label(col.label())
          .on_toggle(move |_| Message::ToggleColumn(col))
          .size(14)
          .text_size(12),
      );
    }
    let panel = container(items)
      .padding(8)
      .width(Length::Fixed(COLUMN_MENU_WIDTH))
      .style(menu_panel_style);

    let dismiss = mouse_area(
      container(Space::new())
        .width(Length::Fill)
        .height(Length::Fill),
    )
    .on_press(Message::CloseColumnMenu)
    .on_right_press(Message::CloseColumnMenu);

    // Right-align the panel to the click so it stays on-screen (the button
    // sits near the right edge), and drop it just below the toolbar.
    let x = (at.x - COLUMN_MENU_WIDTH).max(0.0);
    let y = at.y + 14.0;
    let positioned = column![
      Space::new().height(Length::Fixed(y)),
      row![Space::new().width(Length::Fixed(x)), opaque(panel)],
    ];

    stack![dismiss, positioned].into()
  }

  fn sidebar_view(&self) -> Element<'_, Message> {
    let form = &self.form;

    let label = |s: &'static str| text(s).size(11).color(MUTED);

    let field = |lbl: &'static str,
                 val: &str,
                 msg: fn(String) -> Message|
     -> Element<Message> {
      column![
        label(lbl),
        text_input("", val)
          .id(iced::widget::Id::new(lbl))
          .on_input(msg)
          .size(12)
          .padding(4),
      ]
      .spacing(2)
      .into()
    };

    // Release Date is the master value: while TDRC and TDRL are unified
    // (release_date == None) it edits `form.date`, so both fields track it.
    // Editing Recording Date splits the pair; equal values re-merge them.
    let release_val = form.release_date.as_deref().unwrap_or(&form.date);
    let date_field = row![
      column![
        label("Release Date:"),
        text_input("YYYY[-MM[-DD]]", release_val)
          .id(iced::widget::Id::new("field-date"))
          .on_input(Message::ReleaseDateChanged)
          .size(12)
          .padding(4)
          .width(Length::Fixed(110.0)),
      ]
      .spacing(2),
      column![
        label("Recording Date:"),
        text_input("YYYY[-MM[-DD]]", &form.date)
          .id(iced::widget::Id::new("field-recording-date"))
          .on_input(Message::DateChanged)
          .size(12)
          .padding(4)
          .width(Length::Fixed(110.0)),
      ]
      .spacing(2),
    ]
    .spacing(12);

    let album_track_disc = row![
      column![
        label("Track:"),
        text_input("", &form.track)
          .id(iced::widget::Id::new("field-track"))
          .on_input(Message::TrackChanged)
          .size(12)
          .padding(4)
          .width(Length::Fixed(60.0)),
      ]
      .spacing(2),
      column![
        label("Disc Number:"),
        text_input("", &form.disc)
          .id(iced::widget::Id::new("field-disc"))
          .on_input(Message::DiscChanged)
          .size(12)
          .padding(4)
          .width(Length::Fixed(60.0)),
      ]
      .spacing(2),
      column![
        Space::new().height(14),
        checkbox(form.compilation)
          .label("Compilation")
          .on_toggle(Message::CompilationToggled)
          .size(13)
          .text_size(12),
      ]
      .spacing(2),
    ]
    .spacing(12)
    .align_y(Alignment::End);

    let album_has_data = !form.album.is_empty()
      || !form.album_artist.is_empty()
      || !form.track.is_empty()
      || !form.track_total.is_empty()
      || !form.disc.is_empty()
      || !form.disc_total.is_empty()
      || form.compilation;
    let mut album_delete_btn = button(text("Delete").size(11))
      .padding([2, 8])
      .style(button::danger);
    if album_has_data {
      album_delete_btn = album_delete_btn.on_press(Message::AlbumClear);
    }
    let album_header = row![
      text("Album").size(12).font(BOLD),
      Space::new().width(Length::Fill),
      album_delete_btn,
    ]
    .align_y(Alignment::Center);
    let album_fieldset = container(
      column![
        album_header,
        field("Album:", &form.album, Message::AlbumChanged),
        self.pill_input_view("Album Artist:", PillField::AlbumArtist),
        album_track_disc,
      ]
      .spacing(6),
    )
    .padding(8)
    .style(fieldset_style);

    let save_btn = button(text("Save").size(12))
      .padding([4, 14])
      .style(primary_button_style);
    let has_unsaved = self.is_dirty();
    let save_btn = if has_unsaved {
      save_btn.on_press(Message::Save)
    }
    else {
      save_btn
    };
    let reset_btn = button(text("Reset").size(12))
      .padding([4, 8])
      .style(text_button_style);
    let reset_btn = if has_unsaved {
      reset_btn.on_press(Message::Reset)
    }
    else {
      reset_btn
    };
    let save_row = row![
      save_btn,
      reset_btn,
      text(self.status.as_deref().unwrap_or(""))
        .size(11)
        .color(MUTED),
    ]
    .spacing(10)
    .align_y(Alignment::Center);

    // Play / pause button for currently selected file.
    let selected_path = self
      .selected_idx
      .and_then(|i| self.files.get(i).map(|f| f.path.clone()));
    let is_this_playing = match (&self.playing_path, &selected_path) {
      (Some(p), Some(s)) => p == s && !self.is_paused,
      _ => false,
    };
    let play_glyph = if is_this_playing {
      "\u{23F8}"
    }
    else {
      "\u{25B6}"
    };
    let play_label = if is_this_playing { "Pause" } else { "Play" };
    let mut play_btn = button(
      row![text(play_glyph).size(14), text(play_label).size(12),]
        .spacing(6)
        .align_y(Alignment::Center),
    )
    .padding([4, 12])
    .style(primary_button_style);
    if selected_path.is_some() {
      play_btn = play_btn.on_press(Message::PlayPauseToggle);
    }

    // Stop button — shown only while the selected file is the active track
    // (playing or paused), sitting next to the play/pause button.
    let is_active = match (&self.playing_path, &selected_path) {
      (Some(p), Some(s)) => p == s,
      _ => false,
    };
    let mut playback_controls =
      row![play_btn].spacing(8).align_y(Alignment::Center);
    if is_active {
      playback_controls = playback_controls.push(
        button(
          row![text("\u{23F9}").size(14), text("Stop").size(12)]
            .spacing(6)
            .align_y(Alignment::Center),
        )
        .padding([4, 12])
        .style(primary_button_style)
        .on_press(Message::PlaybackStop),
      );
    }

    let selected_file = self.selected_idx.and_then(|i| self.files.get(i));
    let file_info_text = selected_file
      .map(|f| {
        let ext = f
          .path
          .extension()
          .and_then(|e| e.to_str())
          .map(|s| s.to_uppercase())
          .unwrap_or_default();
        format!(
          "{} | {} | {}",
          ext,
          format_size(f.size_bytes),
          format_duration(f.duration_secs),
        )
      })
      .unwrap_or_default();

    let mut content = Column::new().spacing(6).push(
      row![
        playback_controls,
        text(file_info_text).size(11).color(MUTED),
      ]
      .spacing(12)
      .align_y(Alignment::Center)
      .padding([0, 0]),
    );

    // Seek bar — always present so starting playback doesn't shift the
    // layout below. It stays at 0:00 and ignores drags until the selected
    // file is the active track.
    let total_secs = selected_file.map(|f| f.duration_secs).unwrap_or(0);
    let pos_secs = if is_active {
      self
        .seek_drag_secs
        .unwrap_or(self.playback_pos_secs)
        .min(total_secs as f64)
    }
    else {
      0.0
    };
    content = content.push(
      row![
        text(format_duration(pos_secs as u64)).size(11).color(MUTED),
        slider(
          // Avoid a degenerate range when no file is selected or the
          // duration is unknown.
          0.0..=total_secs.max(1) as f64,
          pos_secs,
          Message::SeekChanged,
        )
        .step(1.0)
        .on_release(Message::SeekReleased)
        .height(14.0)
        .style(seek_slider_style),
        text(format_duration(total_secs)).size(11).color(MUTED),
      ]
      .spacing(8)
      .align_y(Alignment::Center),
    );

    content = content
      .push(self.pill_input_view("Artist:", PillField::Artist))
      .push({
        let input = text_input("", &form.title)
          .id(iced::widget::Id::new("Title:"))
          .on_input(Message::TitleChanged)
          .size(12)
          .padding(4);
        let suggestion = (form.title.is_empty())
          .then(|| selected_file.and_then(|f| title_from_filename(&f.filename)))
          .flatten();
        // Always wrap the input in a row so appending the "From Filepath"
        // button doesn't shift the input's position in the widget tree. iced
        // reconciles widget state by tree position; moving the input would
        // rebuild it with fresh state and drop keyboard focus mid-typing.
        let mut input_row = row![input].spacing(4).align_y(Alignment::Center);
        if suggestion.is_some() {
          input_row = input_row.push(
            button(text("From Filepath").size(11))
              .on_press(Message::TitleFromFilename)
              .padding([4, 8]),
          );
        }
        column![label("Title:"), input_row].spacing(2)
      })
      .push(date_field)
      .push(self.pill_input_view("Genre:", PillField::Genre));
    let supports_extras =
      matches!(self.primary_tag_label.as_str(), "ID3v2" | "Vorbis Comments");
    if supports_extras {
      content = content.push(field(
        "Date Added:",
        &form.date_added,
        Message::DateAddedChanged,
      ));
    }
    let audio_source_label = supports_extras.then_some("Audio Source:");
    let audio_source_field: Option<Element<Message>> =
      audio_source_label.map(|lbl| {
        let input = text_input("", &form.audio_source)
          .id(iced::widget::Id::new("field-audio-source"))
          .on_input(Message::AudioSourceChanged)
          .size(12)
          .padding(4);
        let row_el: Element<Message> =
          if let Some(url) = first_url(&form.audio_source) {
            row![
              input,
              button(text("\u{1F310}").size(12))
                .on_press(Message::AudioSourceOpenUrl(url))
                .padding([4, 8]),
            ]
            .spacing(4)
            .align_y(Alignment::Center)
            .into()
          }
          else {
            input.into()
          };
        column![label(lbl), row_el].spacing(2).into()
      });
    let description_fields: Vec<Element<Message>> = self
      .description_contents
      .iter()
      .enumerate()
      .map(|(idx, c)| {
        let editor = text_editor(c)
          .id(iced::widget::Id::from(format!("editor-description-{idx}")))
          .on_action(move |a| Message::DescriptionAction(idx, a))
          .size(12)
          .padding(4);
        let desc_text = self
          .form
          .descriptions
          .get(idx)
          .map(String::as_str)
          .unwrap_or("");
        let editor_row: Element<Message> =
          if let Some(url) = only_url(desc_text) {
            row![
              editor,
              button(text("\u{1F310}").size(12))
                .on_press(Message::DescriptionOpenUrl(url))
                .padding([4, 8]),
            ]
            .spacing(4)
            .align_y(Alignment::Center)
            .into()
          }
          else {
            editor.into()
          };
        column![label("Description:"), editor_row].spacing(2).into()
      })
      .collect();
    let comment_editor = text_editor(&self.comment_content)
      .id(iced::widget::Id::new("editor-comment"))
      .on_action(Message::CommentAction)
      .size(12)
      .padding(4);
    let comment_row: Element<Message> =
      if let Some(url) = first_url(&form.comment) {
        row![
          comment_editor,
          button(text("\u{1F310}").size(12))
            .on_press(Message::CommentOpenUrl(url))
            .padding([4, 8]),
        ]
        .spacing(4)
        .align_y(Alignment::Center)
        .into()
      }
      else {
        comment_editor.into()
      };
    let comment_field = column![label("Comment:"), comment_row].spacing(2);
    let lyrics_editor = text_editor(&self.lyrics_content)
      .id(iced::widget::Id::new("editor-lyrics"))
      .on_action(Message::LyricsAction)
      .size(12)
      .padding(4);
    let lyrics_field = column![label("Lyrics:"), lyrics_editor].spacing(2);
    if let Some(field) = audio_source_field {
      content = content.push(field);
    }
    for desc_field in description_fields {
      content = content.push(desc_field);
    }
    content = content
      .push(comment_field)
      .push(self.pill_input_view("Composer:", PillField::Composer))
      .push(self.pill_input_view("Arranger:", PillField::Arranger))
      .push(lyrics_field)
      .push(Space::new().height(14))
      .push(album_fieldset)
      .push(Space::new().height(6))
      .push(save_row);

    if self.nav_warning {
      content = content.push(self.nav_warning_banner());
    }

    if !self.primary_tag_label.is_empty() {
      content = content.push(
        text(format!("Editing: {}", self.primary_tag_label))
          .size(10)
          .color(MUTED),
      );
    }

    // Cover
    if let Some(cov) = &self.cover {
      let dims = if cov.width > 0 && cov.height > 0 {
        format!("{}x{}, ", cov.width, cov.height)
      }
      else {
        String::new()
      };
      let cover_image: Element<Message> = mouse_area(
        container(
          image(cov.handle.clone())
            .width(Length::Fixed(240.0))
            .height(Length::Fixed(240.0)),
        )
        .style(cover_frame_style)
        .padding(1),
      )
      .on_press(Message::ShowCoverModal)
      .into();
      let cover_details = column![
        text(format!(
          "{}{} KB, {}, {}",
          dims,
          cov.size_bytes / 1024,
          cov.mime,
          cov.pic_type,
        ))
        .size(10)
        .color(MUTED),
        row![
          button(text("Replace").size(12))
            .on_press(Message::CoverReplace)
            .padding([4, 10]),
          button(text("Export").size(12))
            .on_press(Message::CoverExport)
            .padding([4, 10]),
          button(text("Delete").size(12))
            .on_press(Message::CoverDelete)
            .padding([4, 10]),
        ]
        .spacing(6),
      ]
      .spacing(6);
      let cover_fieldset = container(
        column![
          text("Cover").size(12).font(BOLD),
          row![cover_image, cover_details].spacing(8).wrap(),
        ]
        .spacing(6),
      )
      .width(Length::Fill)
      .padding(8)
      .style(fieldset_style);
      content = content.push(Space::new().height(8));
      content = content.push(cover_fieldset);
    }
    else if self.selected_idx.is_some() {
      let cover_fieldset = container(
        column![
          text("Cover").size(12).font(BOLD),
          container(
            container(
              button(text("Add Cover").size(12))
                .on_press(Message::CoverReplace)
                .padding([6, 14]),
            )
            .center_x(Length::Fixed(240.0))
            .center_y(Length::Fixed(240.0)),
          )
          .style(cover_frame_style)
          .padding(1),
        ]
        .spacing(6),
      )
      .width(Length::Fill)
      .padding(8)
      .style(fieldset_style);
      content = content.push(Space::new().height(8));
      content = content.push(cover_fieldset);
    }

    // ID3v1 read-only
    if let Some(v1) = &self.id3v1 {
      content = content.push(Space::new().height(10));
      content = content.push(text("ID3v1 (read-only)").size(11).color(MUTED));
      let form = &self.form;
      let v1_row = |lbl: &'static str,
                    val: &str,
                    v2_empty: bool,
                    field: Id3v1Field|
       -> Element<Message> {
        let mut r = row![
          text(lbl).size(10).color(MUTED).width(Length::Fixed(56.0)),
          text(val.to_string()).size(10),
        ]
        .spacing(4);
        // Offer to copy into ID3v2 only when v1 has a value and the matching
        // v2 field is empty (so we never silently overwrite existing data).
        if !val.is_empty() && v2_empty {
          r = r.push(Space::new().width(Length::Fixed(12.0)));
          r = r.push(
            button(text("Copy →").size(9))
              .on_press(Message::Id3v1Copy(field))
              .padding([1, 6])
              .style(primary_button_style),
          );
        }
        r.into()
      };
      // Every field defined by the ID3v1 standard, in display order. Only the
      // ones that actually carry a value are rendered below.
      let v1_fields: [(&'static str, &str, bool, Id3v1Field); 7] = [
        (
          "Artist",
          &v1.artist,
          pills_blank(&form.artist),
          Id3v1Field::Artist,
        ),
        (
          "Title",
          &v1.title,
          form.title.trim().is_empty(),
          Id3v1Field::Title,
        ),
        (
          "Album",
          &v1.album,
          form.album.trim().is_empty(),
          Id3v1Field::Album,
        ),
        (
          "Year",
          &v1.year,
          form.date.trim().is_empty(),
          Id3v1Field::Year,
        ),
        (
          "Track",
          &v1.track,
          form.track.trim().is_empty(),
          Id3v1Field::Track,
        ),
        (
          "Genre",
          &v1.genre,
          pills_blank(&form.genre),
          Id3v1Field::Genre,
        ),
        (
          "Comment",
          &v1.comment,
          form.comment.trim().is_empty(),
          Id3v1Field::Comment,
        ),
      ];
      // Whether any field can still be copied into an empty ID3v2 counterpart;
      // drives visibility of the "Copy All" shortcut.
      let any_copyable = v1_fields
        .iter()
        .any(|(_, val, v2_empty, _)| !val.is_empty() && *v2_empty);
      for (lbl, val, v2_empty, field) in v1_fields {
        if !val.is_empty() {
          content = content.push(v1_row(lbl, val, v2_empty, field));
        }
      }
      let mut v1_actions = row![].spacing(6);
      if any_copyable {
        v1_actions = v1_actions.push(
          button(text("Copy All →").size(10))
            .on_press(Message::Id3v1CopyAll)
            .padding([2, 8])
            .style(primary_button_style),
        );
      }
      v1_actions = v1_actions.push(
        button(text("Delete ID3v1").size(10))
          .on_press(Message::Id3v1Delete)
          .padding([2, 8])
          .style(button::danger),
      );
      content = content.push(Space::new().height(4));
      content = content.push(v1_actions);
    }

    if self.selected_idx.is_some() {
      content = content.push(Space::new().height(12));
      content = content.push(
        row![
          Space::new().width(Length::Fill),
          button(text("Show All Metadata").size(11))
            .on_press(Message::ShowAllMetadata)
            .padding([4, 12]),
        ]
        .align_y(Alignment::Center),
      );
    }

    scrollable(content.padding(Padding::new(2.0).right(10.0)))
      .height(Length::Fill)
      .into()
  }

  fn status_bar_view(&self) -> Element<'_, Message> {
    let (total_dur, total_size) =
      self.files.iter().fold((0u64, 0u64), |(d, s), f| {
        (d + f.duration_secs, s + f.size_bytes)
      });

    let selected = if let Some(idx) = self.selected_idx {
      if let Some(f) = self.files.get(idx) {
        format!(
          "1 ({} | {})",
          format_duration(f.duration_secs),
          format_size(f.size_bytes)
        )
      }
      else {
        String::new()
      }
    }
    else {
      String::new()
    };

    let total = format!(
      "{} ({} | {})",
      self.files.len(),
      format_duration(total_dur),
      format_size(total_size)
    );

    row![
      text(selected)
        .size(11)
        .color(MUTED)
        .width(Length::Fixed(220.0)),
      text(total).size(11).color(MUTED),
    ]
    .spacing(20)
    .into()
  }

  fn cover_modal_view<'a>(
    &'a self,
    cov: &'a CoverInfo,
  ) -> Element<'a, Message> {
    let (w, h) = if cov.width > 0 && cov.height > 0 {
      let max_w = 900.0_f32;
      let max_h = 650.0_f32;
      let nw = cov.width as f32;
      let nh = cov.height as f32;
      let scale = (max_w / nw).min(max_h / nh);
      (nw * scale, nh * scale)
    }
    else {
      (600.0, 600.0)
    };

    let panel = container(
      image(cov.handle.clone())
        .width(Length::Fixed(w))
        .height(Length::Fixed(h)),
    )
    .padding(8)
    .style(modal_panel_style);

    let scrim = mouse_area(
      container(Space::new())
        .width(Length::Fill)
        .height(Length::Fill)
        .style(modal_scrim_style),
    )
    .on_press(Message::HideCoverModal);

    let centered = container(opaque(panel))
      .center_x(Length::Fill)
      .center_y(Length::Fill);

    stack![scrim, centered].into()
  }

  fn metadata_modal_view<'a>(
    &'a self,
    dump: &'a MetadataDump,
  ) -> Element<'a, Message> {
    let feedback: Element<Message> = match &self.copy_feedback {
      Some(msg) => text(msg.clone()).size(11).color(ORANGE).into(),
      None => Space::new().into(),
    };
    let header_row = row![
      text("All Metadata").size(15).font(BOLD),
      feedback,
      Space::new().width(Length::Fill),
      button(text("Close").size(12))
        .on_press(Message::HideAllMetadata)
        .padding([4, 12]),
    ]
    .align_y(Alignment::Center)
    .spacing(10);

    let mut body = Column::new().spacing(12);
    for section in &dump.sections {
      let mut section_col = Column::new()
        .push(text(section.heading.clone()).size(12).font(BOLD))
        .push(Space::new().height(4))
        .spacing(2);

      if section.rows.is_empty() {
        section_col = section_col.push(text("(empty)").size(11).color(MUTED));
      }
      for (key, value) in &section.rows {
        section_col = section_col.push(self.metadata_row_view(key, value));
      }

      body = body.push(section_col);
    }

    let panel = container(
      column![
        header_row,
        Space::new().height(8),
        text("Tip: right-click any value to copy it.")
          .size(10)
          .color(MUTED),
        Space::new().height(6),
        scrollable(body).spacing(8).height(Length::Fill),
      ]
      .spacing(0),
    )
    .padding(16)
    .width(Length::Fixed(720.0))
    .max_height(600.0)
    .style(modal_panel_style);

    // Dim background that closes the modal when left-clicked.
    let scrim = mouse_area(
      container(Space::new())
        .width(Length::Fill)
        .height(Length::Fill)
        .style(modal_scrim_style),
    )
    .on_press(Message::HideAllMetadata);

    let centered = container(opaque(panel))
      .center_x(Length::Fill)
      .center_y(Length::Fill);

    // Always return a 3-child stack so opening / closing the copy menu
    // doesn't shuffle the widget tree and reset the scrollable's state.
    let menu_overlay: Element<Message> = match &self.copy_menu {
      Some(menu) => self.copy_menu_view(menu),
      None => Space::new().into(),
    };

    stack![scrim, centered, menu_overlay].into()
  }

  fn metadata_row_view<'a>(
    &self,
    key: &'a str,
    value: &'a str,
  ) -> Element<'a, Message> {
    let label = text(key.to_string())
      .size(11)
      .color(MUTED)
      .font(BOLD)
      .width(Length::Fixed(180.0));

    // Let the value widget fill the remaining width so long values wrap
    // inside the panel instead of pushing the row past the scrollable's
    // reserved scrollbar area.
    let value_area =
      mouse_area(text(value.to_string()).size(11).width(Length::Fill))
        .on_right_press(Message::OpenCopyMenu {
          key: key.to_string(),
          value: value.to_string(),
        });

    row![label, value_area]
      .width(Length::Fill)
      .spacing(10)
      .align_y(Alignment::Start)
      .into()
  }

  /// Renders the floating right-click dropdown at `menu.at`. The whole
  /// window-sized area beneath the menu is a transparent scrim that closes
  /// the menu on any click, so the menu feels like a real popup.
  fn copy_menu_view(&self, menu: &CopyMenu) -> Element<'_, Message> {
    let value_msg = Message::CopyToClipboard(menu.value.clone());
    let pair_msg =
      Message::CopyToClipboard(format!("{}: {}", menu.key, menu.value));

    let panel = container(
      column![
        button(text("Copy Value").size(12))
          .on_press(value_msg)
          .padding([4, 12])
          .width(Length::Fill)
          .style(menu_item_style),
        button(text("Copy Key: Value").size(12))
          .on_press(pair_msg)
          .padding([4, 12])
          .width(Length::Fill)
          .style(menu_item_style),
      ]
      .spacing(2),
    )
    .padding(4)
    .width(Length::Fixed(180.0))
    .style(menu_panel_style);

    // Transparent full-window scrim that swallows any outside click and
    // closes the menu.
    let dismiss = mouse_area(
      container(Space::new())
        .width(Length::Fill)
        .height(Length::Fill),
    )
    .on_press(Message::CloseCopyMenu)
    .on_right_press(Message::CloseCopyMenu);

    // Pin `panel` to `menu.at` using empty space offsets.
    let x = menu.at.x.max(0.0);
    let y = menu.at.y.max(0.0);
    let positioned = column![
      Space::new().height(Length::Fixed(y)),
      row![Space::new().width(Length::Fixed(x)), opaque(panel)],
    ];

    stack![dismiss, positioned].into()
  }
}

// ───── Styles ─────────────────────────────────────────────────────────────

fn panel_style(_theme: &Theme) -> container::Style {
  container::Style {
    background: Some(Background::Color(Color::WHITE)),
    border: Border {
      color: BORDER,
      width: 1.0,
      radius: 4.0.into(),
    },
    ..container::Style::default()
  }
}

fn sidebar_style(_theme: &Theme) -> container::Style {
  container::Style {
    background: Some(Background::Color(PANEL_BG)),
    border: Border {
      color: BORDER,
      width: 1.0,
      radius: 4.0.into(),
    },
    ..container::Style::default()
  }
}

fn header_bar_style(_theme: &Theme) -> container::Style {
  container::Style {
    background: Some(Background::Color(HEADER_BG)),
    border: Border {
      color: BORDER,
      width: 0.0,
      radius: 0.0.into(),
    },
    ..container::Style::default()
  }
}

fn warning_banner_style(_theme: &Theme) -> container::Style {
  container::Style {
    background: Some(Background::Color(Color::from_rgb(0.85, 0.2, 0.2))),
    text_color: Some(Color::WHITE),
    border: Border {
      color: Color::from_rgb(0.6, 0.1, 0.1),
      width: 0.0,
      radius: 0.0.into(),
    },
    ..container::Style::default()
  }
}

fn status_bar_style(_theme: &Theme) -> container::Style {
  container::Style {
    background: Some(Background::Color(HEADER_BG)),
    border: Border {
      color: BORDER,
      width: 1.0,
      radius: 0.0.into(),
    },
    ..container::Style::default()
  }
}

fn table_header_style(_theme: &Theme) -> container::Style {
  container::Style {
    background: Some(Background::Color(HEADER_BG)),
    border: Border {
      color: BORDER,
      width: 0.0,
      radius: 0.0.into(),
    },
    ..container::Style::default()
  }
}

fn cover_frame_style(_theme: &Theme) -> container::Style {
  container::Style {
    background: Some(Background::Color(Color::WHITE)),
    border: Border {
      color: BORDER,
      width: 1.0,
      radius: 2.0.into(),
    },
    ..container::Style::default()
  }
}

fn fieldset_style(_theme: &Theme) -> container::Style {
  container::Style {
    background: Some(Background::Color(Color::WHITE)),
    border: Border {
      color: BORDER,
      width: 1.0,
      radius: 4.0.into(),
    },
    ..container::Style::default()
  }
}

/// A single committed value rendered as a rounded chip with an `×` button.
fn pill_chip(
  value: &str,
  field: PillField,
  idx: usize,
) -> Element<'static, Message> {
  container(
    row![
      text(value.to_string()).size(11),
      button(text("\u{00D7}").size(12))
        .on_press(Message::PillRemove(field, idx))
        .padding([0, 4])
        .style(pill_remove_button_style),
    ]
    .spacing(2)
    .align_y(Alignment::Center),
  )
  .padding([1, 7])
  .style(pill_style)
  .into()
}

/// A read-only chip (no remove button) used to show a single value in a
/// table cell. The text color is pinned dark so it stays readable on the
/// light chip background even when the row is selected (which otherwise turns
/// inherited text white).
fn read_only_chip(value: &str) -> Element<'static, Message> {
  container(text(value.to_string()).size(11).color(Color::BLACK))
    .padding([1, 7])
    .style(pill_style)
    .into()
}

/// Renders a table cell for a multi-valued field: a wrapping row of read-only
/// chips when there are two or more entries, otherwise plain text.
fn table_value_view(values: &[String]) -> Element<'_, Message> {
  if values.len() >= 2 {
    let mut wrap = Row::new().spacing(4).align_y(Alignment::Center);
    for value in values {
      wrap = wrap.push(read_only_chip(value));
    }
    wrap.wrap().into()
  }
  else {
    text(values.first().cloned().unwrap_or_default())
      .size(12)
      .into()
  }
}

fn pill_style(_theme: &Theme) -> container::Style {
  container::Style {
    background: Some(Background::Color(ROW_ALT)),
    border: Border {
      color: BORDER,
      width: 1.0,
      radius: 8.0.into(),
    },
    ..container::Style::default()
  }
}

fn pill_remove_button_style(
  _theme: &Theme,
  status: button::Status,
) -> button::Style {
  let text_color = match status {
    button::Status::Hovered | button::Status::Pressed => {
      Color::from_rgb(0.85, 0.2, 0.2)
    }
    _ => MUTED,
  };
  button::Style {
    background: None,
    text_color,
    border: Border::default(),
    ..button::Style::default()
  }
}

fn selected_row_style(_theme: &Theme, status: button::Status) -> button::Style {
  let bg = match status {
    button::Status::Hovered | button::Status::Pressed => ORANGE_DARK,
    _ => ORANGE,
  };
  button::Style {
    background: Some(Background::Color(bg)),
    text_color: Color::WHITE,
    border: Border::default(),
    ..button::Style::default()
  }
}

fn plain_row_style(_theme: &Theme, status: button::Status) -> button::Style {
  let bg = match status {
    button::Status::Hovered => ROW_HOVER,
    _ => Color::WHITE,
  };
  button::Style {
    background: Some(Background::Color(bg)),
    text_color: Color::BLACK,
    border: Border::default(),
    ..button::Style::default()
  }
}

fn alt_row_style(_theme: &Theme, status: button::Status) -> button::Style {
  let bg = match status {
    button::Status::Hovered => ROW_HOVER,
    _ => ROW_ALT,
  };
  button::Style {
    background: Some(Background::Color(bg)),
    text_color: Color::BLACK,
    border: Border::default(),
    ..button::Style::default()
  }
}

fn modal_scrim_style(_theme: &Theme) -> container::Style {
  container::Style {
    background: Some(Background::Color(MODAL_SCRIM)),
    ..container::Style::default()
  }
}

fn modal_panel_style(_theme: &Theme) -> container::Style {
  container::Style {
    background: Some(Background::Color(Color::WHITE)),
    border: Border {
      color: BORDER,
      width: 1.0,
      radius: 6.0.into(),
    },
    ..container::Style::default()
  }
}

fn menu_panel_style(_theme: &Theme) -> container::Style {
  container::Style {
    background: Some(Background::Color(Color::WHITE)),
    border: Border {
      color: BORDER,
      width: 1.0,
      radius: 4.0.into(),
    },
    ..container::Style::default()
  }
}

fn menu_item_style(_theme: &Theme, status: button::Status) -> button::Style {
  let (bg, fg) = match status {
    button::Status::Hovered | button::Status::Pressed => {
      (Some(Background::Color(ROW_HOVER)), Color::BLACK)
    }
    _ => (None, Color::BLACK),
  };
  button::Style {
    background: bg,
    text_color: fg,
    border: Border {
      color: Color::TRANSPARENT,
      width: 0.0,
      radius: 2.0.into(),
    },
    ..button::Style::default()
  }
}

fn primary_button_style(
  _theme: &Theme,
  status: button::Status,
) -> button::Style {
  let bg = match status {
    button::Status::Disabled => Color::from_rgb(0.78, 0.78, 0.80),
    button::Status::Hovered | button::Status::Pressed => ORANGE_DARK,
    _ => ORANGE,
  };
  button::Style {
    background: Some(Background::Color(bg)),
    text_color: Color::WHITE,
    border: Border {
      color: Color::TRANSPARENT,
      width: 0.0,
      radius: 4.0.into(),
    },
    ..button::Style::default()
  }
}

fn seek_slider_style(_theme: &Theme, status: slider::Status) -> slider::Style {
  let handle_color = match status {
    slider::Status::Hovered | slider::Status::Dragged => ORANGE_DARK,
    _ => ORANGE,
  };
  slider::Style {
    rail: slider::Rail {
      backgrounds: (Background::Color(ORANGE), Background::Color(BORDER)),
      width: 4.0,
      border: Border {
        color: Color::TRANSPARENT,
        width: 0.0,
        radius: 2.0.into(),
      },
    },
    handle: slider::Handle {
      shape: slider::HandleShape::Circle { radius: 6.0 },
      background: Background::Color(handle_color),
      border_width: 0.0,
      border_color: Color::TRANSPARENT,
    },
  }
}

fn text_button_style(_theme: &Theme, status: button::Status) -> button::Style {
  let text_color = match status {
    button::Status::Disabled => Color::from_rgb(0.70, 0.70, 0.72),
    button::Status::Hovered | button::Status::Pressed => ORANGE_DARK,
    _ => MUTED,
  };
  button::Style {
    background: None,
    text_color,
    border: Border {
      color: Color::TRANSPARENT,
      width: 0.0,
      radius: 4.0.into(),
    },
    ..button::Style::default()
  }
}

// ───── IO / Tag utilities ─────────────────────────────────────────────────

/// Returns the first `http://` or `https://` URL found in `s`, trimmed of
/// common trailing punctuation, or `None` if no URL is present.
fn first_url(s: &str) -> Option<String> {
  let http = s.find("http://");
  let https = s.find("https://");
  let start = match (http, https) {
    (Some(a), Some(b)) => a.min(b),
    (Some(a), None) => a,
    (None, Some(b)) => b,
    (None, None) => return None,
  };
  let rest = &s[start..];
  let end = rest.find(|c: char| c.is_whitespace()).unwrap_or(rest.len());
  let mut url = rest[..end].to_string();
  while let Some(last) = url.chars().last() {
    if matches!(
      last,
      '.' | ',' | ')' | ']' | '}' | '!' | '?' | ';' | ':' | '>' | '"' | '\''
    ) {
      url.pop();
    }
    else {
      break;
    }
  }
  // Require something beyond the scheme.
  let scheme_len = if url.starts_with("https://") { 8 } else { 7 };
  if url.len() > scheme_len {
    Some(url)
  }
  else {
    None
  }
}

/// Returns the URL when `s` (after trimming) is nothing but a single URL.
fn only_url(s: &str) -> Option<String> {
  let trimmed = s.trim();
  let url = first_url(trimmed)?;
  (url == trimmed).then_some(url)
}

/// Derives a title suggestion from a file's path (relative to the loaded
/// directory) by dropping the extension, turning path separators and
/// `_`, `-`, `.` into spaces, and title-casing each word. Returns `None`
/// when nothing usable remains.
fn title_from_filename(filename: &str) -> Option<String> {
  let path = Path::new(filename);
  // Drop only the final extension, keeping any relative directory parts.
  let stem = match (path.parent(), path.file_stem()) {
    (Some(parent), Some(file_stem)) if !parent.as_os_str().is_empty() => {
      parent.join(file_stem)
    }
    (_, Some(file_stem)) => PathBuf::from(file_stem),
    _ => path.to_path_buf(),
  };
  let title = stem
    .to_string_lossy()
    .split(|c: char| {
      matches!(c, '_' | '-' | '.' | '/' | '\\') || c.is_whitespace()
    })
    .filter(|w| !w.is_empty())
    .map(|word| {
      let mut chars = word.chars();
      match chars.next() {
        Some(first) => {
          first.to_uppercase().collect::<String>()
            + &chars.as_str().to_lowercase()
        }
        None => String::new(),
      }
    })
    .collect::<Vec<_>>()
    .join(" ");
  (!title.is_empty()).then_some(title)
}

/// Label for the "reveal in file manager" menu item, named after the host
/// platform's default file explorer.
#[cfg(target_os = "macos")]
const REVEAL_LABEL: &str = "Reveal in Finder";
#[cfg(target_os = "windows")]
const REVEAL_LABEL: &str = "Show in Explorer";
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
const REVEAL_LABEL: &str = "Show in File Manager";

/// Reveals `path` in the host platform's file manager, selecting the file
/// where supported.
fn reveal_in_file_manager(path: &std::path::Path) {
  #[cfg(target_os = "macos")]
  let _ = std::process::Command::new("open")
    .arg("-R")
    .arg(path)
    .spawn();
  #[cfg(target_os = "windows")]
  let _ = std::process::Command::new("explorer")
    .arg(format!("/select,{}", path.display()))
    .spawn();
  // Linux/BSD: no portable "select the file" verb, so open its parent folder.
  #[cfg(not(any(target_os = "macos", target_os = "windows")))]
  {
    let target = path.parent().unwrap_or(path);
    let _ = std::process::Command::new("xdg-open").arg(target).spawn();
  }
}

/// Opens `url` in the user's default browser on the host platform.
fn open_url(url: &str) {
  #[cfg(target_os = "macos")]
  let _ = std::process::Command::new("open").arg(url).spawn();
  #[cfg(target_os = "linux")]
  let _ = std::process::Command::new("xdg-open").arg(url).spawn();
  #[cfg(target_os = "windows")]
  let _ = std::process::Command::new("cmd")
    .args(["/C", "start", "", url])
    .spawn();
}

fn format_duration(secs: u64) -> String {
  let h = secs / 3600;
  let m = (secs % 3600) / 60;
  let s = secs % 60;
  if h > 0 {
    format!("{}:{:02}:{:02}", h, m, s)
  }
  else {
    format!("{}:{:02}", m, s)
  }
}

fn format_size(bytes: u64) -> String {
  let mb = bytes as f64 / 1_048_576.0;
  if mb >= 1024.0 {
    format!("{:.2} GB", mb / 1024.0)
  }
  else {
    format!("{:.1} MB", mb)
  }
}

fn scan_audio_paths(dir: &Path) -> Vec<PathBuf> {
  let mut files: Vec<PathBuf> = WalkDir::new(dir)
    .follow_links(false)
    .into_iter()
    .filter_entry(|e| {
      !e.file_name()
        .to_str()
        .map(|s| s.starts_with('.'))
        .unwrap_or(false)
    })
    .filter_map(|e| e.ok())
    .filter(|e| e.file_type().is_file())
    .map(|e| e.into_path())
    .filter(|p| {
      p.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| AUDIO_EXTENSIONS.contains(&ext.to_lowercase().as_str()))
        .unwrap_or(false)
    })
    .collect();
  files.sort();
  files
}

fn scan_and_load(dir: &Path) -> Vec<FileInfo> {
  scan_audio_paths(dir)
    .into_iter()
    .map(|p| {
      let mut info = load_file_info(&p).unwrap_or_else(|_| fallback_info(&p));
      info.filename = p
        .strip_prefix(dir)
        .unwrap_or(&p)
        .to_string_lossy()
        .into_owned();
      info
    })
    .collect()
}

/// Loads [`FileInfo`] for an explicit list of files (as passed on the command
/// line), preserving the given order. Missing paths are skipped so a reload
/// after a file was moved or deleted doesn't fail; the row's `filename` is the
/// bare file name.
fn load_files(paths: &[PathBuf]) -> Vec<FileInfo> {
  paths
    .iter()
    .filter(|p| p.is_file())
    .map(|p| load_file_info(p).unwrap_or_else(|_| fallback_info(p)))
    .collect()
}

fn fallback_info(path: &Path) -> FileInfo {
  let filename = path
    .file_name()
    .map(|n| n.to_string_lossy().into_owned())
    .unwrap_or_default();
  let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
  FileInfo {
    path: path.to_path_buf(),
    filename,
    size_bytes: size,
    ..Default::default()
  }
}

fn load_file_info(path: &Path) -> Result<FileInfo, String> {
  let filename = path
    .file_name()
    .map(|n| n.to_string_lossy().into_owned())
    .unwrap_or_default();
  let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);

  let tagged_file = lofty::read_from_path(path).map_err(|e| e.to_string())?;
  let props = tagged_file.properties();
  let duration = props.duration().as_secs();

  let mut info = FileInfo {
    path: path.to_path_buf(),
    filename,
    duration_secs: duration,
    size_bytes: size,
    ..Default::default()
  };

  if let Some(t) = editable_tag(&tagged_file) {
    info.title = t.title().map(|s| s.to_string()).unwrap_or_default();
    // Join multi-valued fields with ", " for the (single-line) table columns.
    info.artist = read_values(t, ItemKey::TrackArtist);
    info.album = t.album().map(|s| s.to_string()).unwrap_or_default();
    info.comment = t.comment().map(|s| s.to_string()).unwrap_or_default();
    info.genre = read_values(t, ItemKey::Genre);
    info.composer = read_values(t, ItemKey::Composer);
    info.arranger = read_values(t, ItemKey::Arranger);
    // Prefer the explicit release date (TDRL); fall back to the recording
    // date (TDRC) so single-date files still populate the column.
    info.release_date = t
      .get_string(ItemKey::ReleaseDate)
      .map(|s| s.to_string())
      .or_else(|| t.date().map(|d| d.to_string()))
      .unwrap_or_default();
    // Join the (possibly multiple) non-empty descriptions for the column.
    info.description = read_descriptions(t)
      .into_iter()
      .filter(|d| !d.is_empty())
      .collect::<Vec<_>>()
      .join("; ");
  }

  Ok(info)
}

fn editable_tag(tagged_file: &lofty::file::TaggedFile) -> Option<&Tag> {
  taguar::editable_tag(tagged_file)
}

fn load_full(
  path: &Path,
) -> (TagForm, Option<Id3v1Display>, String, Option<CoverInfo>) {
  let mut form = TagForm::default();
  let mut id3v1_display = None;
  let mut label = String::new();
  let mut cover = None;

  let tagged_file = match lofty::read_from_path(path) {
    Ok(f) => f,
    Err(_) => return (form, None, label, None),
  };

  if let Some(tag) = tagged_file.tag(TagType::Id3v1) {
    id3v1_display = Some(Id3v1Display {
      title: tag.title().map(|s| s.to_string()).unwrap_or_default(),
      artist: tag.artist().map(|s| s.to_string()).unwrap_or_default(),
      album: tag.album().map(|s| s.to_string()).unwrap_or_default(),
      year: tag.date().map(|d| d.year.to_string()).unwrap_or_default(),
      comment: tag.comment().map(|s| s.to_string()).unwrap_or_default(),
      track: tag.track().map(|t| t.to_string()).unwrap_or_default(),
      genre: tag.genre().map(|s| s.to_string()).unwrap_or_default(),
    });
  }

  if let Some(tag) = editable_tag(&tagged_file) {
    label = tag_type_label(tag.tag_type()).to_string();
    form.title = tag.title().map(|s| s.to_string()).unwrap_or_default();
    form.artist = read_values(tag, ItemKey::TrackArtist);
    form.album = tag.album().map(|s| s.to_string()).unwrap_or_default();
    form.album_artist = read_values(tag, ItemKey::AlbumArtist);
    let tdrc = tag.date();
    let tdrl = tag
      .get_string(ItemKey::ReleaseDate)
      .and_then(|s| s.parse::<Timestamp>().ok());
    match (tdrc, tdrl) {
      (Some(rec), Some(rel)) if rec != rel => {
        form.date = rec.to_string();
        form.release_date = Some(rel.to_string());
      }
      (Some(ts), _) | (_, Some(ts)) => {
        form.date = ts.to_string();
        form.release_date = None;
      }
      (None, None) => {
        form.date = String::new();
        form.release_date = None;
      }
    }
    form.track = tag.track().map(|t| t.to_string()).unwrap_or_default();
    form.track_total =
      tag.track_total().map(|t| t.to_string()).unwrap_or_default();
    form.disc = tag.disk().map(|d| d.to_string()).unwrap_or_default();
    form.disc_total =
      tag.disk_total().map(|d| d.to_string()).unwrap_or_default();
    form.genre = read_values(tag, ItemKey::Genre);
    form.comment = tag.comment().map(|s| s.to_string()).unwrap_or_default();
    form.descriptions = read_descriptions(tag);
    form.composer = read_values(tag, ItemKey::Composer);
    form.arranger = read_values(tag, ItemKey::Arranger);
    form.lyrics = tag
      .get_string(ItemKey::Lyrics)
      .or_else(|| tag.get_string(ItemKey::UnsyncLyrics))
      .map(|s| s.to_string())
      .unwrap_or_default();
    form.compilation = tag
      .get_string(ItemKey::FlagCompilation)
      .map(|v| matches!(v.trim(), "1" | "true" | "yes"))
      .unwrap_or(false);
    form.date_added =
      read_date_added(path, tagged_file.file_type(), tag.tag_type())
        .unwrap_or_default();
    form.audio_source =
      read_audio_source(path, tagged_file.file_type(), tag.tag_type())
        .unwrap_or_default();
    // ID3v2 has no native Description frame; we store descriptions as a
    // TXXX:Description user-text frame, joined by `\0` for multi-value.
    if tag.tag_type() == TagType::Id3v2 {
      let descs = read_id3v2_descriptions(path);
      if !descs.is_empty() {
        form.descriptions = descs;
      }
    }

    // Pick cover: prefer CoverFront, else first picture.
    if let Some(pic) = tag
      .pictures()
      .iter()
      .find(|p| p.pic_type() == PictureType::CoverFront)
      .or_else(|| tag.pictures().first())
    {
      let data = pic.data().to_vec();
      let size_bytes = data.len();
      let mime = pic
        .mime_type()
        .map(|m| m.as_str().to_string())
        .unwrap_or_else(|| "image".to_string());
      let pic_type_label = pic_type_label(pic.pic_type());
      let (w, h) = probe_image_dims(&data);
      cover = Some(CoverInfo {
        handle: image::Handle::from_bytes(data),
        width: w,
        height: h,
        size_bytes,
        mime,
        pic_type: pic_type_label,
      });
    }
  }
  else {
    label = tag_type_label(tagged_file.primary_tag_type()).to_string();
  }

  (form, id3v1_display, label, cover)
}

/// Collects every piece of metadata we can extract from `path` into one
/// [`MetadataSection`] per logical group (file path, audio properties, each
/// tag) for the "All Metadata" modal.
fn load_metadata_dump(path: &Path) -> MetadataDump {
  let mut sections = Vec::new();

  sections.push(MetadataSection {
    heading: "File".to_string(),
    rows: vec![("Path".to_string(), path.display().to_string())],
  });

  let tagged_file = match lofty::read_from_path(path) {
    Ok(f) => f,
    Err(e) => {
      sections.push(MetadataSection {
        heading: "Error".to_string(),
        rows: vec![("Message".to_string(), e.to_string())],
      });
      return MetadataDump { sections };
    }
  };

  // Audio properties
  let p = tagged_file.properties();
  let mut props: Vec<(String, String)> = Vec::new();
  props.push((
    "File Type".to_string(),
    format!("{:?}", tagged_file.file_type()),
  ));
  props.push((
    "Duration".to_string(),
    format_duration(p.duration().as_secs()),
  ));
  if let Some(br) = p.overall_bitrate() {
    props.push(("Overall Bitrate".to_string(), format!("{br} kbps")));
  }
  if let Some(br) = p.audio_bitrate() {
    props.push(("Audio Bitrate".to_string(), format!("{br} kbps")));
  }
  if let Some(sr) = p.sample_rate() {
    props.push(("Sample Rate".to_string(), format!("{sr} Hz")));
  }
  if let Some(bd) = p.bit_depth() {
    props.push(("Bit Depth".to_string(), format!("{bd} bit")));
  }
  if let Some(ch) = p.channels() {
    props.push(("Channels".to_string(), ch.to_string()));
  }
  if let Ok(meta) = std::fs::metadata(path) {
    props.push(("File Size".to_string(), format_size(meta.len())));
  }
  sections.push(MetadataSection {
    heading: "Audio Properties".to_string(),
    rows: props,
  });

  // One section per tag on the file (ID3v2, ID3v1, Vorbis, MP4 ilst, …).
  for tag in tagged_file.tags() {
    let heading = format!(
      "{} ({} items)",
      tag_type_label(tag.tag_type()),
      tag.item_count()
    );
    let mut rows: Vec<(String, String)> = tag
      .items()
      .map(|item| {
        let key = format!("{:?}", item.key());
        let value = match item.value() {
          ItemValue::Text(t) => t.clone(),
          ItemValue::Locator(t) => format!("[locator] {t}"),
          ItemValue::Binary(b) => format!("[binary] {} bytes", b.len()),
        };
        (key, value)
      })
      .collect();
    for (i, pic) in tag.pictures().iter().enumerate() {
      let mime = pic
        .mime_type()
        .map(|m| m.as_str().to_string())
        .unwrap_or_else(|| "unknown".to_string());
      let (w, h) = probe_image_dims(pic.data());
      let dims = if w > 0 && h > 0 {
        format!("{w}x{h}, ")
      }
      else {
        String::new()
      };
      rows.push((
        format!("Picture #{}", i + 1),
        format!(
          "{} — {}{} KB, {}",
          pic_type_label(pic.pic_type()),
          dims,
          pic.data().len() / 1024,
          mime
        ),
      ));
    }
    sections.push(MetadataSection { heading, rows });
  }

  MetadataDump { sections }
}

fn probe_image_dims(data: &[u8]) -> (u32, u32) {
  use std::io::Cursor;
  let cursor = Cursor::new(data);
  if let Ok(reader) = ::image::ImageReader::new(cursor).with_guessed_format() {
    if let Ok(dims) = reader.into_dimensions() {
      return dims;
    }
  }
  (0, 0)
}

fn tag_type_label(tag_type: TagType) -> &'static str {
  match tag_type {
    TagType::Id3v1 => "ID3v1",
    TagType::Id3v2 => "ID3v2",
    TagType::Ape => "APE",
    TagType::VorbisComments => "Vorbis Comments",
    TagType::Mp4Ilst => "MP4 iTunes (ilst)",
    TagType::RiffInfo => "RIFF INFO",
    TagType::AiffText => "AIFF Text",
    _ => "Tag",
  }
}

fn mime_to_extension(mime: &str) -> &'static str {
  match mime.to_ascii_lowercase().as_str() {
    "image/jpeg" | "image/jpg" => "jpg",
    "image/png" => "png",
    "image/gif" => "gif",
    "image/bmp" => "bmp",
    "image/tiff" => "tiff",
    "image/webp" => "webp",
    _ => "img",
  }
}

fn export_cover(src: &Path, dest: &Path) -> Result<PathBuf, String> {
  let tagged_file = lofty::read_from_path(src).map_err(|e| e.to_string())?;
  let tag = editable_tag(&tagged_file)
    .ok_or_else(|| "No editable tag found".to_string())?;
  let pic = tag
    .pictures()
    .iter()
    .find(|p| p.pic_type() == PictureType::CoverFront)
    .or_else(|| tag.pictures().first())
    .ok_or_else(|| "No embedded cover image".to_string())?;
  std::fs::write(dest, pic.data()).map_err(|e| e.to_string())?;
  Ok(dest.to_path_buf())
}

fn pic_type_label(t: PictureType) -> String {
  match t {
    PictureType::CoverFront => "Front Cover".to_string(),
    PictureType::CoverBack => "Back Cover".to_string(),
    PictureType::Icon => "Icon".to_string(),
    PictureType::OtherIcon => "Other Icon".to_string(),
    PictureType::Leaflet => "Leaflet".to_string(),
    PictureType::Media => "Media".to_string(),
    PictureType::LeadArtist => "Lead Artist".to_string(),
    PictureType::Artist => "Artist".to_string(),
    PictureType::Conductor => "Conductor".to_string(),
    PictureType::Band => "Band".to_string(),
    PictureType::Composer => "Composer".to_string(),
    PictureType::Lyricist => "Lyricist".to_string(),
    PictureType::RecordingLocation => "Recording Location".to_string(),
    PictureType::DuringRecording => "During Recording".to_string(),
    PictureType::DuringPerformance => "During Performance".to_string(),
    PictureType::ScreenCapture => "Screen Capture".to_string(),
    PictureType::BrightFish => "Bright Fish".to_string(),
    PictureType::Illustration => "Illustration".to_string(),
    PictureType::BandLogo => "Band Logo".to_string(),
    PictureType::PublisherLogo => "Publisher Logo".to_string(),
    PictureType::Other => "Other".to_string(),
    _ => "Picture".to_string(),
  }
}

fn save_tags(
  path: &Path,
  form: &TagForm,
  pic_change: PictureChange,
) -> Result<(), String> {
  // Normalize away surrounding whitespace before writing (and before the
  // round-trip check in `verify_saved`, which receives this same `form`).
  let form = &form.trimmed();

  let tagged_file =
    lofty::read_from_path(path).map_err(|e| format!("read file: {e}"))?;

  let mut tag = match editable_tag(&tagged_file).cloned() {
    Some(t) => t,
    None => Tag::new(tagged_file.primary_tag_type()),
  };

  apply_picture_change(&mut tag, pic_change)?;

  set_or_remove_string(&mut tag, ItemKey::TrackTitle, &form.title, |t, v| {
    t.set_title(v)
  });
  set_or_remove_string(&mut tag, ItemKey::AlbumTitle, &form.album, |t, v| {
    t.set_album(v)
  });
  set_or_remove_string(&mut tag, ItemKey::Comment, &form.comment, |t, v| {
    t.set_comment(v)
  });

  // Multi-valued ID3v2.4 fields: each value becomes its own item, joined into
  // one null-separated frame on save (and the native multi-value form for
  // other tag types).
  apply_values(&mut tag, ItemKey::TrackArtist, &form.artist);
  apply_values(&mut tag, ItemKey::Genre, &form.genre);
  apply_values(&mut tag, ItemKey::AlbumArtist, &form.album_artist);
  apply_values(&mut tag, ItemKey::Composer, &form.composer);
  // Arranger relies on `apply_values`' `push_unchecked` so values reach
  // lofty's per-format conversion (e.g. ID3v2's TIPL routing); a post-save
  // round-trip check then surfaces any value the target format can't
  // represent.
  apply_values(&mut tag, ItemKey::Arranger, &form.arranger);

  apply_descriptions(&mut tag, &form.descriptions);

  let lyrics = form.lyrics.clone();
  tag.remove_key(ItemKey::Lyrics);
  tag.remove_key(ItemKey::UnsyncLyrics);
  if !lyrics.is_empty() {
    // Use Lyrics for formats that support it; UnsyncLyrics for ID3v2.
    let key = if tag.tag_type() == TagType::Id3v2 {
      ItemKey::UnsyncLyrics
    }
    else {
      ItemKey::Lyrics
    };
    tag.insert_unchecked(TagItem::new(key, ItemValue::Text(lyrics)));
  }

  if form.compilation {
    tag.insert_unchecked(TagItem::new(
      ItemKey::FlagCompilation,
      ItemValue::Text("1".to_string()),
    ));
  }
  else {
    tag.remove_key(ItemKey::FlagCompilation);
  }

  set_or_remove_dates(&mut tag, &form.date, form.release_date.as_deref())?;
  set_or_remove_u32(
    &mut tag,
    &form.track,
    "track",
    |t| t.remove_track(),
    |t, v| t.set_track(v),
  )?;
  set_or_remove_u32(
    &mut tag,
    &form.track_total,
    "track total",
    |t| t.remove_track_total(),
    |t, v| t.set_track_total(v),
  )?;
  set_or_remove_u32(
    &mut tag,
    &form.disc,
    "disc",
    |t| t.remove_disk(),
    |t, v| t.set_disk(v),
  )?;
  set_or_remove_u32(
    &mut tag,
    &form.disc_total,
    "disc total",
    |t| t.remove_disk_total(),
    |t, v| t.set_disk_total(v),
  )?;

  let file_type = tagged_file.file_type();
  let tag_type = tag.tag_type();
  let date_added = form.date_added.clone();
  let audio_source = form.audio_source.clone();

  if tag_type == TagType::Id3v2 {
    let mut id3v2 = Id3v2Tag::from(tag);
    if date_added.is_empty() {
      id3v2.remove_user_text("DATE_ADDED");
    }
    else {
      id3v2.insert_user_text("DATE_ADDED".to_string(), date_added);
    }
    set_id3v2_user_url(&mut id3v2, "AUDIO_SOURCE", &audio_source);
    set_id3v2_descriptions(&mut id3v2, &form.descriptions);
    id3v2
      .save_to_path(path, WriteOptions::default())
      .map_err(|e| format!("write tags: {e}"))?;
  }
  else {
    tag
      .save_to_path(path, WriteOptions::default())
      .map_err(|e| format!("write tags: {e}"))?;
    if tag_type == TagType::VorbisComments {
      write_vorbis_extras(path, file_type, &date_added, &audio_source)?;
    }
  }

  verify_saved(path, form, tag_type)?;

  Ok(())
}

/// True when a non-empty expected value didn't round-trip to `actual`.
fn value_missing(expected: &str, actual: Option<&str>) -> bool {
  !expected.is_empty() && actual.unwrap_or("") != expected
}

/// True when the non-empty expected values didn't round-trip to `actual` (in
/// the same order). An empty expectation never counts as missing.
fn values_missing(expected: &[String], actual: &[String]) -> bool {
  let expected: Vec<&str> = expected
    .iter()
    .map(String::as_str)
    .filter(|s| !s.is_empty())
    .collect();
  if expected.is_empty() {
    return false;
  }
  actual.iter().map(String::as_str).collect::<Vec<_>>() != expected
}

/// Re-reads the file after save and confirms every non-empty form field
/// round-tripped. Surfaces a clear error rather than silently losing data
/// when the target format can't represent a value (e.g. AIFF Text / RIFF
/// INFO lacking a mapping for some `ItemKey`).
fn verify_saved(
  path: &Path,
  form: &TagForm,
  tag_type: TagType,
) -> Result<(), String> {
  let tagged =
    lofty::read_from_path(path).map_err(|e| format!("verify: {e}"))?;
  let tag = tagged
    .tags()
    .iter()
    .find(|t| t.tag_type() == tag_type)
    .ok_or_else(|| "Saved tag not found on disk".to_string())?;

  let mut missing: Vec<&str> = Vec::new();
  if value_missing(&form.title, tag.title().as_deref()) {
    missing.push("Title");
  }
  if values_missing(&form.artist, &read_values(tag, ItemKey::TrackArtist)) {
    missing.push("Artist");
  }
  if value_missing(&form.album, tag.album().as_deref()) {
    missing.push("Album");
  }
  if values_missing(&form.album_artist, &read_values(tag, ItemKey::AlbumArtist))
  {
    missing.push("Album Artist");
  }
  if values_missing(&form.genre, &read_values(tag, ItemKey::Genre)) {
    missing.push("Genre");
  }
  if values_missing(&form.composer, &read_values(tag, ItemKey::Composer)) {
    missing.push("Composer");
  }
  if values_missing(&form.arranger, &read_values(tag, ItemKey::Arranger)) {
    missing.push("Arranger");
  }
  if value_missing(&form.comment, tag.comment().as_deref()) {
    missing.push("Comment");
  }
  let lyrics_actual = tag
    .get_string(ItemKey::Lyrics)
    .or_else(|| tag.get_string(ItemKey::UnsyncLyrics));
  if value_missing(form.lyrics.trim_end(), lyrics_actual) {
    missing.push("Lyrics");
  }
  if form.compilation && tag.get_string(ItemKey::FlagCompilation).is_none() {
    missing.push("Compilation");
  }
  let expected_descriptions: Vec<String> = form
    .descriptions
    .iter()
    .filter(|d| !d.is_empty())
    .cloned()
    .collect();
  if !expected_descriptions.is_empty() {
    let actual: Vec<String> = if tag_type == TagType::Id3v2 {
      read_id3v2_descriptions(path)
    }
    else {
      tag
        .get_strings(ItemKey::Description)
        .map(str::to_string)
        .collect()
    };
    if actual != expected_descriptions {
      missing.push("Description");
    }
  }

  if missing.is_empty() {
    Ok(())
  }
  else {
    Err(format!(
      "{} doesn't support: {}",
      tag_type_label(tag_type),
      missing.join(", ")
    ))
  }
}

/// Reads `TXXX:Description` from an ID3v2 file. Multi-value descriptions are
/// stored joined by `\0` (the ID3v2.4 multi-value separator); this returns
/// them split back into individual entries, or an empty Vec if the frame is
/// absent.
fn read_id3v2_descriptions(path: &Path) -> Vec<String> {
  let Ok(tagged) = lofty::read_from_path(path) else {
    return Vec::new();
  };
  let Some(tag) = tagged
    .tags()
    .iter()
    .find(|t| t.tag_type() == TagType::Id3v2)
    .cloned()
  else {
    return Vec::new();
  };
  match Id3v2Tag::from(tag).get_user_text("Description") {
    Some(s) if !s.is_empty() => s.split('\0').map(str::to_string).collect(),
    _ => Vec::new(),
  }
}

/// Writes `descriptions` to ID3v2 as a single `TXXX:Description` frame, with
/// multiple entries joined by `\0`. Empty entries are dropped, and removing
/// all of them deletes the frame.
fn set_id3v2_descriptions(tag: &mut Id3v2Tag, descriptions: &[String]) {
  let non_empty: Vec<&str> = descriptions
    .iter()
    .map(String::as_str)
    .filter(|d| !d.is_empty())
    .collect();
  if non_empty.is_empty() {
    tag.remove_user_text("Description");
  }
  else {
    tag.insert_user_text("Description".to_string(), non_empty.join("\0"));
  }
}

/// Replaces or removes a `WXXX` frame keyed by `description`. Empty `value`
/// removes any matching frame; otherwise inserts/replaces it.
fn set_id3v2_user_url(tag: &mut Id3v2Tag, description: &str, value: &str) {
  tag.retain(|frame| {
    !matches!(
      frame,
      Frame::UserUrl(ExtendedUrlFrame { description: d, .. })
        if d == description
    )
  });
  if !value.is_empty() {
    tag.insert(Frame::UserUrl(ExtendedUrlFrame::new(
      TextEncoding::UTF8,
      description.to_string(),
      value.to_string(),
    )));
  }
}

/// Reads a custom `DATE_ADDED` value from formats that carry a user-visible
/// concept of one: ID3v2 uses a `TXXX:DATE_ADDED` frame; Vorbis Comments
/// (Opus/Vorbis/FLAC/Speex) use a plain `DATE_ADDED` key.
fn read_date_added(
  path: &Path,
  file_type: FileType,
  tag_type: TagType,
) -> Option<String> {
  match tag_type {
    TagType::Id3v2 => {
      let tagged_file = lofty::read_from_path(path).ok()?;
      let tag = tagged_file
        .tags()
        .iter()
        .find(|t| t.tag_type() == TagType::Id3v2)?
        .clone();
      Id3v2Tag::from(tag)
        .get_user_text("DATE_ADDED")
        .map(str::to_string)
    }
    TagType::VorbisComments => {
      let vc = read_vorbis_comments(path, file_type)?;
      vc.get("DATE_ADDED").map(str::to_string)
    }
    _ => None,
  }
}

/// Reads the audio-source URL: ID3v2 uses a `WXXX:AUDIO_SOURCE` user-defined
/// URL frame; Vorbis Comments use a plain `AUDIO_SOURCE` key.
fn read_audio_source(
  path: &Path,
  file_type: FileType,
  tag_type: TagType,
) -> Option<String> {
  match tag_type {
    TagType::Id3v2 => {
      let tagged_file = lofty::read_from_path(path).ok()?;
      let tag = tagged_file
        .tags()
        .iter()
        .find(|t| t.tag_type() == TagType::Id3v2)?
        .clone();
      (&Id3v2Tag::from(tag))
        .into_iter()
        .find_map(|frame| match frame {
          Frame::UserUrl(ExtendedUrlFrame {
            description,
            content,
            ..
          }) if description == "AUDIO_SOURCE" => Some(content.to_string()),
          _ => None,
        })
    }
    TagType::VorbisComments => {
      let vc = read_vorbis_comments(path, file_type)?;
      vc.get("AUDIO_SOURCE").map(str::to_string)
    }
    _ => None,
  }
}

fn read_vorbis_comments(
  path: &Path,
  file_type: FileType,
) -> Option<VorbisComments> {
  let file = File::open(path).ok()?;
  let mut reader = BufReader::new(file);
  let options = ParseOptions::new();
  match file_type {
    FileType::Opus => OpusFile::read_from(&mut reader, options)
      .ok()
      .map(|f| f.vorbis_comments().clone()),
    FileType::Vorbis => VorbisFile::read_from(&mut reader, options)
      .ok()
      .map(|f| f.vorbis_comments().clone()),
    FileType::Flac => FlacFile::read_from(&mut reader, options).ok().map(|f| {
      // FLAC keeps its cover in native PICTURE metadata blocks, separate from
      // the Vorbis comment block that `vorbis_comments()` returns. Saving a
      // bare VorbisComments back to the file strips those PICTURE blocks, so
      // fold them into the returned tag to preserve the cover on write.
      let mut vc = f.vorbis_comments().cloned().unwrap_or_default();
      for (pic, info) in f.pictures() {
        let _ = vc.insert_picture(pic.clone(), Some(*info));
      }
      vc
    }),
    FileType::Speex => SpeexFile::read_from(&mut reader, options)
      .ok()
      .map(|f| f.vorbis_comments().clone()),
    _ => None,
  }
}

fn write_vorbis_extras(
  path: &Path,
  file_type: FileType,
  date_added: &str,
  audio_source: &str,
) -> Result<(), String> {
  let Some(mut vc) = read_vorbis_comments(path, file_type) else {
    return Ok(());
  };
  set_vorbis_field(&mut vc, "DATE_ADDED", date_added);
  set_vorbis_field(&mut vc, "AUDIO_SOURCE", audio_source);
  vc.save_to_path(path, WriteOptions::default())
    .map_err(|e| format!("write extras: {e}"))
}

fn set_vorbis_field(vc: &mut VorbisComments, key: &str, value: &str) {
  if value.is_empty() {
    let _ = vc.remove(key).count();
  }
  else {
    vc.insert(key.to_string(), value.to_string());
  }
}

fn delete_id3v1_tag(path: &Path) -> Result<(), String> {
  Tag::new(TagType::Id3v1)
    .remove_from_path(path)
    .map_err(|e| e.to_string())
}

fn apply_picture_change(
  tag: &mut Tag,
  change: PictureChange,
) -> Result<(), String> {
  // Index of the picture that the UI treats as "the cover": prefer
  // CoverFront, else the first picture if any.
  let cover_idx = tag
    .pictures()
    .iter()
    .position(|p| p.pic_type() == PictureType::CoverFront)
    .or_else(|| (!tag.pictures().is_empty()).then_some(0));

  match change {
    PictureChange::None => {}
    PictureChange::Replace(img_path) => {
      let image_file =
        File::open(&img_path).map_err(|e| format!("open image: {e}"))?;
      let mut reader = BufReader::new(image_file);
      let mut new_pic = Picture::from_reader(&mut reader)
        .map_err(|e| format!("read image: {e}"))?;

      // Preserve the pic_type of the picture being replaced when possible.
      let desired_type = cover_idx
        .and_then(|i| tag.pictures().get(i).map(|p| p.pic_type()))
        .unwrap_or(PictureType::CoverFront);
      new_pic.set_pic_type(desired_type);

      match cover_idx {
        Some(i) => tag.set_picture(i, new_pic),
        None => tag.push_picture(new_pic),
      }
    }
    PictureChange::Delete => {
      if let Some(i) = cover_idx {
        tag.remove_picture(i);
      }
    }
  }

  Ok(())
}

fn set_or_remove_string(
  tag: &mut Tag,
  key: ItemKey,
  value: &str,
  setter: impl FnOnce(&mut Tag, String),
) {
  if value.is_empty() {
    tag.remove_key(key);
  }
  else {
    setter(tag, value.to_string());
  }
}

fn set_or_remove_dates(
  tag: &mut Tag,
  date: &str,
  release_date: Option<&str>,
) -> Result<(), String> {
  // TDRC (RecordingDate): de-facto "release date" read by most players.
  let tdrc = parse_opt_date(date, "date")?;
  // TDRL (ReleaseDate): semantically correct per the spec. When in unified
  // mode (release_date == None), mirror TDRC so the two stay in sync.
  let tdrl = match release_date {
    Some(rd) => parse_opt_date(rd, "release date")?,
    None => tdrc,
  };

  match tdrc {
    Some(ts) => tag.set_date(ts),
    None => tag.remove_date(),
  }
  match tdrl {
    Some(ts) => {
      tag.insert_unchecked(TagItem::new(
        ItemKey::ReleaseDate,
        ItemValue::Text(ts.to_string()),
      ));
    }
    None => tag.remove_key(ItemKey::ReleaseDate),
  }

  Ok(())
}

/// Writes `value` to the tag under `key`, bypassing lofty's per-format map
/// check so values reach format-specific conversion (e.g. ID3v2 TIPL). Empty
/// values remove the key.
fn parse_opt_date(
  value: &str,
  label: &str,
) -> Result<Option<Timestamp>, String> {
  let trimmed = value.trim();
  if trimmed.is_empty() {
    Ok(None)
  }
  else {
    trimmed
      .parse::<Timestamp>()
      .map(Some)
      .map_err(|_| format!("Invalid {label}: '{value}'"))
  }
}

fn set_or_remove_u32(
  tag: &mut Tag,
  value: &str,
  field_name: &str,
  remover: impl FnOnce(&mut Tag),
  setter: impl FnOnce(&mut Tag, u32),
) -> Result<(), String> {
  if value.trim().is_empty() {
    remover(tag);
    Ok(())
  }
  else {
    match value.trim().parse::<u32>() {
      Ok(v) => {
        setter(tag, v);
        Ok(())
      }
      Err(_) => Err(format!("Invalid {field_name}: '{value}'")),
    }
  }
}

// ───── Playback ───────────────────────────────────────────────────────────

#[derive(Debug)]
enum PlaybackCmd {
  Play(PathBuf),
  Pause,
  Resume,
  Stop,
  Seek(Duration),
}

static PLAYBACK: OnceLock<mpsc::Sender<PlaybackCmd>> = OnceLock::new();

/// Current playback position in milliseconds, published by the worker and
/// polled by the UI's tick subscription.
static PLAYBACK_POS_MS: AtomicU64 = AtomicU64::new(0);
/// Set by the worker when the active track played to its end, so the UI can
/// reset the transport controls. Consumed (swapped to false) by the UI.
static PLAYBACK_FINISHED: AtomicBool = AtomicBool::new(false);

const PLAYBACK_THREAD_NAME: &str = "taguar-playback";

fn install_silent_panic_hook() {
  let prev = std::panic::take_hook();
  std::panic::set_hook(Box::new(move |info| {
    if thread::current().name() == Some(PLAYBACK_THREAD_NAME) {
      // Swallow panics from the playback thread — they're caught by
      // catch_unwind and surfaced via our own logging.
      return;
    }
    prev(info);
  }));
}

fn playback_send(cmd: PlaybackCmd) {
  let tx = PLAYBACK.get_or_init(|| {
    install_silent_panic_hook();
    let (tx, rx) = mpsc::channel::<PlaybackCmd>();
    thread::Builder::new()
      .name(PLAYBACK_THREAD_NAME.into())
      .spawn(move || playback_worker(rx))
      .expect("spawn playback thread");
    tx
  });
  let _ = tx.send(cmd);
}

fn playback_worker(rx: mpsc::Receiver<PlaybackCmd>) {
  let device_sink = match rodio::DeviceSinkBuilder::open_default_sink() {
    Ok(s) => s,
    Err(_) => return,
  };
  let mixer = device_sink.mixer();
  let mut player: Option<rodio::Player> = None;

  loop {
    // Wake up regularly (even without commands) to publish the position.
    let cmd = match rx.recv_timeout(Duration::from_millis(200)) {
      Ok(cmd) => Some(cmd),
      Err(mpsc::RecvTimeoutError::Timeout) => None,
      Err(mpsc::RecvTimeoutError::Disconnected) => break,
    };
    match cmd {
      None => {}
      Some(PlaybackCmd::Play(path)) => {
        if let Some(p) = player.take() {
          p.stop();
        }
        let new_player = rodio::Player::connect_new(mixer);
        let is_opus = path
          .extension()
          .and_then(|e| e.to_str())
          .map(|s| s.eq_ignore_ascii_case("opus"))
          .unwrap_or(false);

        let result = if is_opus {
          match OpusSource::open(&path) {
            Ok(src) => {
              new_player.append(src);
              Ok(())
            }
            Err(e) => Err(e),
          }
        }
        else {
          match File::open(&path) {
            Ok(file) => {
              // `try_from` (unlike `Decoder::new`) marks the source as
              // seekable and sets its byte length — without this, symphonia
              // can only "seek" forward by skipping ahead, and seeking
              // backwards fails.
              let decoder_result =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                  rodio::Decoder::try_from(file)
                }));
              match decoder_result {
                Ok(Ok(decoder)) => {
                  new_player.append(decoder);
                  Ok(())
                }
                Ok(Err(e)) => Err(e.to_string()),
                Err(_) => Err(format!(
                  "decoder panicked (unsupported / malformed): {}",
                  path.display()
                )),
              }
            }
            Err(e) => Err(e.to_string()),
          }
        };

        match result {
          Ok(()) => {
            new_player.play();
            player = Some(new_player);
          }
          Err(e) => eprintln!("play error: {e}"),
        }
      }
      Some(PlaybackCmd::Pause) => {
        if let Some(p) = &player {
          p.pause();
        }
      }
      Some(PlaybackCmd::Resume) => {
        if let Some(p) = &player {
          p.play();
        }
      }
      Some(PlaybackCmd::Stop) => {
        if let Some(p) = player.take() {
          p.stop();
        }
        PLAYBACK_POS_MS.store(0, Ordering::Relaxed);
      }
      Some(PlaybackCmd::Seek(pos)) => {
        if let Some(p) = &player {
          if let Err(e) = p.try_seek(pos) {
            eprintln!("seek error: {e}");
          }
        }
      }
    }

    // Publish the position (or completion) for the UI's tick handler.
    if player.as_ref().is_some_and(|p| p.empty()) {
      player = None;
      PLAYBACK_POS_MS.store(0, Ordering::Relaxed);
      PLAYBACK_FINISHED.store(true, Ordering::Relaxed);
    }
    else if let Some(p) = &player {
      PLAYBACK_POS_MS.store(p.get_pos().as_millis() as u64, Ordering::Relaxed);
    }
  }
}

// ───── Opus decoder (OGG container) ────────────────────────────────────────
// symphonia 0.5 has no working Opus decoder, so this bypasses rodio and
// streams Opus packets through libopus via the `opus` crate.

struct OpusSource {
  reader: ogg::PacketReader<BufReader<File>>,
  decoder: opus::Decoder,
  channels: rodio::ChannelCount,
  /// Pre-skip from the OpusHead — granule positions include these samples.
  pre_skip: u64,
  pre_skip_remaining: u64,
  samples: Vec<f32>,
  sample_pos: usize,
  finished: bool,
}

impl OpusSource {
  fn open(path: &Path) -> Result<Self, String> {
    let file = File::open(path).map_err(|e| format!("open: {e}"))?;
    let mut reader = ogg::PacketReader::new(BufReader::new(file));

    // OpusHead
    let head = reader
      .read_packet_expected()
      .map_err(|e| format!("read OpusHead: {e}"))?;
    if head.data.len() < 19 || &head.data[0..8] != b"OpusHead" {
      return Err("not an Ogg Opus stream (missing OpusHead)".into());
    }
    let channel_count = head.data[9];
    let pre_skip = u16::from_le_bytes([head.data[10], head.data[11]]) as u64;

    let (ch_enum, ch_num) = match channel_count {
      1 => (opus::Channels::Mono, std::num::NonZeroU16::new(1).unwrap()),
      2 => (
        opus::Channels::Stereo,
        std::num::NonZeroU16::new(2).unwrap(),
      ),
      n => return Err(format!("unsupported channel count: {n}")),
    };

    // OpusTags (comment header) — skip.
    reader
      .read_packet_expected()
      .map_err(|e| format!("read OpusTags: {e}"))?;

    let decoder = opus::Decoder::new(48_000, ch_enum)
      .map_err(|e| format!("opus init: {e}"))?;

    Ok(Self {
      reader,
      decoder,
      channels: ch_num,
      pre_skip,
      pre_skip_remaining: pre_skip,
      samples: Vec::new(),
      sample_pos: 0,
      finished: false,
    })
  }

  fn fill(&mut self) -> bool {
    // Up to 120 ms of audio at 48 kHz.
    const MAX_SAMPLES_PER_CHANNEL: usize = 5760;

    loop {
      if self.finished {
        return false;
      }
      let packet = match self.reader.read_packet() {
        Ok(Some(p)) => p,
        Ok(None) => {
          self.finished = true;
          return false;
        }
        Err(e) => {
          eprintln!("opus read error: {e}");
          self.finished = true;
          return false;
        }
      };
      if packet.data.is_empty() {
        continue;
      }

      let mut buf =
        vec![0.0f32; MAX_SAMPLES_PER_CHANNEL * self.channels.get() as usize];
      let decoded =
        match self.decoder.decode_float(&packet.data, &mut buf, false) {
          Ok(n) => n,
          Err(e) => {
            eprintln!("opus decode error: {e}");
            continue;
          }
        };
      buf.truncate(decoded * self.channels.get() as usize);

      // Discard pre-skip samples from the start of the stream.
      if self.pre_skip_remaining > 0 {
        let skip_frames = (self.pre_skip_remaining as usize).min(decoded);
        let skip_samples = skip_frames * self.channels.get() as usize;
        self.pre_skip_remaining -= skip_frames as u64;
        if skip_samples >= buf.len() {
          continue;
        }
        buf.drain(..skip_samples);
      }

      if buf.is_empty() {
        continue;
      }
      self.samples = buf;
      self.sample_pos = 0;
      return true;
    }
  }
}

impl Iterator for OpusSource {
  type Item = f32;

  fn next(&mut self) -> Option<f32> {
    if self.sample_pos >= self.samples.len() && !self.fill() {
      return None;
    }
    let s = self.samples[self.sample_pos];
    self.sample_pos += 1;
    Some(s)
  }
}

impl rodio::Source for OpusSource {
  fn current_span_len(&self) -> Option<usize> {
    None
  }
  fn channels(&self) -> rodio::ChannelCount {
    self.channels
  }
  fn sample_rate(&self) -> rodio::SampleRate {
    std::num::NonZeroU32::new(48_000).unwrap()
  }
  fn total_duration(&self) -> Option<std::time::Duration> {
    None
  }

  fn try_seek(
    &mut self,
    pos: Duration,
  ) -> Result<(), rodio::source::SeekError> {
    use rodio::source::SeekError;

    // Ogg granule positions for Opus count 48 kHz samples from the start of
    // the stream, including the pre-skip samples.
    let absgp = (pos.as_secs_f64() * 48_000.0) as u64 + self.pre_skip;
    let found = self
      .reader
      .seek_absgp(None, absgp)
      .map_err(|e| SeekError::Other(std::sync::Arc::new(e)))?;
    if !found {
      return Err(SeekError::NotSupported {
        underlying_source: "ogg-opus",
      });
    }
    let _ = self.decoder.reset_state();
    self.samples.clear();
    self.sample_pos = 0;
    // The seek already skipped past the start of the stream.
    self.pre_skip_remaining = 0;
    self.finished = false;
    Ok(())
  }
}

#[cfg(test)]
mod tests {
  use super::split_into_values;
  use super::title_from_filename;

  #[test]
  fn splits_only_on_the_most_common_separator() {
    assert_eq!(
      split_into_values("AC/DC,John,Marc"),
      vec!["AC/DC", "John", "Marc"],
    );
  }

  #[test]
  fn splits_on_single_present_separator() {
    assert_eq!(split_into_values("Rock/Pop"), vec!["Rock", "Pop"]);
    assert_eq!(split_into_values("a; b ;c"), vec!["a", "b", "c"]);
  }

  #[test]
  fn keeps_value_intact_without_separators() {
    assert_eq!(split_into_values("Solo"), vec!["Solo"]);
  }

  #[test]
  fn dominant_comma_keeps_slashed_name_together() {
    assert_eq!(
      split_into_values("AC/DC,Bob Marley & The Wailers,Slash"),
      vec!["AC/DC", "Bob Marley & The Wailers", "Slash"],
    );
  }

  #[test]
  fn tie_prefers_earlier_separator() {
    assert_eq!(split_into_values("a,b/c"), vec!["a", "b/c"]);
  }

  #[test]
  fn title_cases_separators_and_drops_extension() {
    assert_eq!(
      title_from_filename("some_song-name.mp3").as_deref(),
      Some("Some Song Name"),
    );
  }

  #[test]
  fn normalizes_existing_capitalization() {
    assert_eq!(
      title_from_filename("MY GREAT track.flac").as_deref(),
      Some("My Great Track"),
    );
  }

  #[test]
  fn includes_relative_directory_parts() {
    assert_eq!(
      title_from_filename("Pink Floyd/The Wall/01_another-brick.mp3")
        .as_deref(),
      Some("Pink Floyd The Wall 01 Another Brick"),
    );
  }

  #[test]
  fn returns_none_when_nothing_usable() {
    assert_eq!(title_from_filename("___.mp3"), None);
    assert_eq!(title_from_filename(""), None);
  }
}
