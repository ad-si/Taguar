#![windows_subsystem = "windows"]

use iced::widget::{
  button, checkbox, column, container, image, mouse_area, opaque, row,
  scrollable, stack, text, text_input, Column, Space,
};
use iced::{
  event, keyboard, mouse, Alignment, Background, Border, Color, Element, Event,
  Font, Length, Point, Subscription, Task, Theme,
};
use lofty::config::WriteOptions;
use lofty::file::TaggedFileExt;
use lofty::picture::{Picture, PictureType};
use lofty::prelude::{Accessor, AudioFile, ItemKey, TagExt};
use lofty::tag::items::Timestamp;
use lofty::tag::{ItemValue, Tag, TagType};
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, OnceLock};
use std::thread;
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
  let arg_dir = std::env::args().nth(1).map(|arg| {
    if arg == "-h" || arg == "--help" {
      println!("Usage: taguar [DIRECTORY]");
      std::process::exit(0);
    }
    let path = PathBuf::from(&arg);
    if !path.is_dir() {
      eprintln!("Not a directory: {}", path.display());
      std::process::exit(2);
    }
    path.canonicalize().unwrap_or(path)
  });

  iced::application(
    move || {
      let state = Taguar::default();
      let task = match arg_dir.clone() {
        Some(dir) => Task::done(Message::DirectoryChosen(Some(dir))),
        None => Task::none(),
      };
      (state, task)
    },
    Taguar::update,
    Taguar::view,
  )
  .subscription(Taguar::subscription)
  .title("Taguar")
  .theme(Theme::Light)
  .window_size((1200.0, 760.0))
  .font(FONT_REGULAR_BYTES)
  .font(FONT_BOLD_BYTES)
  .default_font(APP_FONT)
  .run()
}

#[derive(Default)]
struct Taguar {
  directory: Option<PathBuf>,
  files: Vec<FileInfo>,
  selected_idx: Option<usize>,
  form: TagForm,
  saved_form: TagForm,
  id3v1: Option<Id3v1Display>,
  cover: Option<CoverInfo>,
  primary_tag_label: String,
  status: Option<String>,
  loading: bool,
  playing_path: Option<PathBuf>,
  is_paused: bool,
  metadata_dump: Option<MetadataDump>,
  /// Open right-click dropdown within the metadata modal.
  copy_menu: Option<CopyMenu>,
  /// Last known cursor position in window coordinates — captured from the
  /// event subscription while the modal is open, so we can pin the dropdown
  /// to where the user right-clicked.
  last_cursor: Option<Point>,
  /// Transient feedback shown in the modal header after a copy.
  copy_feedback: Option<String>,
}

#[derive(Clone)]
struct CopyMenu {
  /// Anchor position (window coordinates) where the dropdown should appear.
  at: Point,
  key: String,
  value: String,
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
  artist: String,
  comment: String,
  duration_secs: u64,
  size_bytes: u64,
}

#[derive(Default, Clone, PartialEq)]
struct TagForm {
  title: String,
  artist: String,
  album: String,
  album_artist: String,
  date: String,
  // Some(_) only when the file's TDRC and TDRL differ; a second input then
  // appears in the form so both values can be edited independently.
  release_date: Option<String>,
  track: String,
  track_total: String,
  disc: String,
  disc_total: String,
  genre: String,
  comment: String,
  composer: String,
  compilation: bool,
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
  Reload,
  FilesLoaded(Vec<FileInfo>),
  FileSelected(usize),
  TitleChanged(String),
  ArtistChanged(String),
  AlbumChanged(String),
  AlbumArtistChanged(String),
  DateChanged(String),
  ReleaseDateChanged(String),
  TrackChanged(String),
  DiscChanged(String),
  GenreChanged(String),
  CommentChanged(String),
  ComposerChanged(String),
  CompilationToggled(bool),
  PlayPauseToggle,
  Save,
  Saved(Result<(), String>),
  CoverReplace,
  CoverReplaceChosen(Option<PathBuf>),
  CoverDelete,
  Id3v1Delete,
  Id3v1Deleted(Result<(), String>),
  CommentOpenUrl(String),
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
}

/// Describes a change to the embedded cover picture to apply during a save.
#[derive(Clone)]
enum PictureChange {
  None,
  Replace(PathBuf),
  Delete,
}

impl Taguar {
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
        playback_send(PlaybackCmd::Stop);
        self.playing_path = None;
        self.is_paused = false;
        self.directory = Some(dir.clone());
        self.files.clear();
        self.selected_idx = None;
        self.form = TagForm::default();
        self.saved_form = TagForm::default();
        self.id3v1 = None;
        self.cover = None;
        self.primary_tag_label.clear();
        self.loading = true;
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
      Message::Reload => {
        if let Some(dir) = self.directory.clone() {
          playback_send(PlaybackCmd::Stop);
          self.playing_path = None;
          self.is_paused = false;
          self.files.clear();
          self.selected_idx = None;
          self.form = TagForm::default();
          self.saved_form = TagForm::default();
          self.id3v1 = None;
          self.cover = None;
          self.primary_tag_label.clear();
          self.loading = true;
          self.status = Some("Reloading...".to_string());
          Task::perform(
            async move {
              tokio::task::spawn_blocking(move || scan_and_load(&dir))
                .await
                .unwrap_or_default()
            },
            Message::FilesLoaded,
          )
        }
        else {
          Task::none()
        }
      }
      Message::FilesLoaded(files) => {
        self.files = files;
        self.loading = false;
        self.status = None;
        Task::none()
      }
      Message::FileSelected(idx) => {
        if let Some(info) = self.files.get(idx) {
          let (form, id3v1, label, cover) = load_full(&info.path);
          self.form = form.clone();
          self.saved_form = form;
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
      Message::TitleChanged(v) => {
        self.form.title = v;
        Task::none()
      }
      Message::ArtistChanged(v) => {
        self.form.artist = v;
        Task::none()
      }
      Message::AlbumChanged(v) => {
        self.form.album = v;
        Task::none()
      }
      Message::AlbumArtistChanged(v) => {
        self.form.album_artist = v;
        Task::none()
      }
      Message::DateChanged(v) => {
        self.form.date = v;
        Task::none()
      }
      Message::ReleaseDateChanged(v) => {
        self.form.release_date = Some(v);
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
      Message::GenreChanged(v) => {
        self.form.genre = v;
        Task::none()
      }
      Message::CommentChanged(v) => {
        self.form.comment = v;
        Task::none()
      }
      Message::ComposerChanged(v) => {
        self.form.composer = v;
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
            playback_send(PlaybackCmd::Play(path.clone()));
            self.playing_path = Some(path);
            self.is_paused = false;
          }
        }
        Task::none()
      }
      Message::Save => self.spawn_save(PictureChange::None, "Saving..."),
      Message::Saved(Ok(())) => {
        self.status = Some("Saved.".to_string());
        if let Some(idx) = self.selected_idx {
          let path = self.files[idx].path.clone();
          // Refresh editable form + cover.
          let (form, id3v1, label, cover) = load_full(&path);
          self.form = form.clone();
          self.saved_form = form;
          self.id3v1 = id3v1;
          self.primary_tag_label = label;
          self.cover = cover;
          // Refresh the file's row in the table.
          if let Ok(mut info) = load_file_info(&path) {
            if let Some(root) = &self.directory {
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
    }
  }

  /// Only subscribes to cursor events when the metadata modal is open — so
  /// the rest of the app isn't paying for per-pixel messages.
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
        _ => None,
      }
    });

    if self.metadata_dump.is_some() {
      let cursor_sub =
        event::listen_with(|event, _status, _window| match event {
          Event::Mouse(mouse::Event::CursorMoved { position }) => {
            Some(Message::CursorMoved(position))
          }
          _ => None,
        });
      Subscription::batch([keyboard_sub, cursor_sub])
    }
    else {
      keyboard_sub
    }
  }

  /// Kicks off a background save that applies the current form plus an
  /// optional picture change, reporting completion via [`Message::Saved`].
  fn spawn_save(
    &mut self,
    pic_change: PictureChange,
    status: &str,
  ) -> Task<Message> {
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
    if self.directory.is_none() {
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

    let base: Element<Message> = column![
      header,
      row![
        container(table)
          .width(Length::FillPortion(7))
          .height(Length::Fill)
          .style(panel_style),
        container(sidebar)
          .width(Length::FillPortion(3))
          .height(Length::Fill)
          .style(sidebar_style)
          .padding(10),
      ]
      .height(Length::Fill),
      container(status)
        .padding([4, 10])
        .width(Length::Fill)
        .style(status_bar_style),
    ]
    .into();

    match &self.metadata_dump {
      Some(dump) => stack![base, self.metadata_modal_view(dump)].into(),
      None => base,
    }
  }

  fn header_view(&self) -> Element<'_, Message> {
    let dir = self
      .directory
      .as_ref()
      .map(|p| p.to_string_lossy().to_string())
      .unwrap_or_default();

    container(
      row![
        button(text("Change Directory").size(12))
          .on_press(Message::SelectDirectory)
          .padding([4, 10]),
        text(dir).size(12).font(BOLD).width(Length::Fill),
        button(text("Reload").size(12))
          .on_press(Message::Reload)
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
    // Columns: Filename | Artist | Title | Comment
    // Weights (proportional) — columns stretch to fill the available width.
    let weights: [u16; 4] = [8, 4, 5, 6];
    let headers = ["Filename", "Artist", "Title", "Comment"];

    let header_cells: Vec<Element<Message>> = headers
      .iter()
      .zip(weights.iter())
      .map(|(label, w)| {
        text(*label)
          .size(12)
          .font(BOLD)
          .width(Length::FillPortion(*w))
          .color(MUTED)
          .into()
      })
      .collect();
    let header_row = container(
      iced::widget::Row::with_children(header_cells)
        .spacing(10)
        .padding([6, 10]),
    )
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

      let cells = row![
        text(info.filename.clone())
          .size(12)
          .width(Length::FillPortion(weights[0])),
        text(info.artist.clone())
          .size(12)
          .width(Length::FillPortion(weights[1])),
        text(info.title.clone())
          .size(12)
          .width(Length::FillPortion(weights[2])),
        text(info.comment.clone())
          .size(12)
          .width(Length::FillPortion(weights[3])),
      ]
      .spacing(10);

      let style: fn(&Theme, button::Status) -> button::Style = if selected {
        selected_row_style
      }
      else if alt {
        alt_row_style
      }
      else {
        plain_row_style
      };

      button(cells)
        .on_press(Message::FileSelected(idx))
        .width(Length::Fill)
        .padding([4, 10])
        .style(style)
        .into()
    });

    let body = scrollable(Column::with_children(rows).spacing(0))
      .height(Length::Fill)
      .width(Length::Fill);

    column![header_row, body].into()
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
        text_input("", val).on_input(msg).size(12).padding(4),
      ]
      .spacing(2)
      .into()
    };

    let date_label = if form.release_date.is_some() {
      "Recording Date (TDRC):"
    }
    else {
      "Release Date:"
    };
    let year_genre = row![
      column![
        label(date_label),
        text_input("YYYY[-MM[-DD]]", &form.date)
          .on_input(Message::DateChanged)
          .size(12)
          .padding(4)
          .width(Length::Fixed(110.0)),
      ]
      .spacing(2),
      column![
        label("Genre:"),
        text_input("", &form.genre)
          .on_input(Message::GenreChanged)
          .size(12)
          .padding(4),
      ]
      .spacing(2),
    ]
    .spacing(6);

    let album_track_disc = row![
      column![
        label("Track:"),
        text_input("", &form.track)
          .on_input(Message::TrackChanged)
          .size(12)
          .padding(4)
          .width(Length::Fixed(60.0)),
      ]
      .spacing(2),
      column![
        label("Disc Number:"),
        text_input("", &form.disc)
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

    let album_fieldset = container(
      column![
        text("Album").size(12).font(BOLD),
        field("Album:", &form.album, Message::AlbumChanged),
        field(
          "Album Artist:",
          &form.album_artist,
          Message::AlbumArtistChanged,
        ),
        album_track_disc,
      ]
      .spacing(6),
    )
    .padding(8)
    .style(fieldset_style);

    let save_btn = button(text("Save").size(12))
      .padding([4, 14])
      .style(primary_button_style);
    let save_btn = if self.form != self.saved_form {
      save_btn.on_press(Message::Save)
    }
    else {
      save_btn
    };
    let save_row = row![
      save_btn,
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

    let mut content = Column::new()
      .spacing(6)
      .push(row![play_btn].padding([0, 0]))
      .push(
        column![
          label("Artist:"),
          text_input("", &form.artist)
            .id(iced::widget::Id::new("artist"))
            .on_input(Message::ArtistChanged)
            .size(12)
            .padding(4),
        ]
        .spacing(2),
      )
      .push(field("Title:", &form.title, Message::TitleChanged))
      .push(year_genre);
    if let Some(rd) = &form.release_date {
      content = content.push(field(
        "Release Date (TDRL):",
        rd,
        Message::ReleaseDateChanged,
      ));
    }
    let comment_input = text_input("", &form.comment)
      .on_input(Message::CommentChanged)
      .size(12)
      .padding(4);
    let comment_row: Element<Message> =
      if let Some(url) = first_url(&form.comment) {
        row![
          comment_input,
          button(text("\u{1F310}").size(12))
            .on_press(Message::CommentOpenUrl(url))
            .padding([4, 8]),
        ]
        .spacing(4)
        .align_y(Alignment::Center)
        .into()
      }
      else {
        comment_input.into()
      };
    let comment_field = column![label("Comment:"), comment_row].spacing(2);
    content = content
      .push(comment_field)
      .push(field("Composer:", &form.composer, Message::ComposerChanged))
      .push(Space::new().height(14))
      .push(album_fieldset)
      .push(Space::new().height(6))
      .push(save_row);

    if !self.primary_tag_label.is_empty() {
      content = content.push(
        text(format!("Editing: {}", self.primary_tag_label))
          .size(10)
          .color(MUTED),
      );
    }

    // Cover
    if let Some(cov) = &self.cover {
      content = content.push(Space::new().height(8));
      content = content.push(
        container(
          image(cov.handle.clone())
            .width(Length::Fixed(240.0))
            .height(Length::Fixed(240.0)),
        )
        .style(cover_frame_style)
        .padding(1),
      );
      let dims = if cov.width > 0 && cov.height > 0 {
        format!("{}x{}, ", cov.width, cov.height)
      }
      else {
        String::new()
      };
      content = content.push(
        text(format!(
          "{}{} KB, {}, {}",
          dims,
          cov.size_bytes / 1024,
          cov.mime,
          cov.pic_type,
        ))
        .size(10)
        .color(MUTED),
      );
      content = content.push(Space::new().height(4));
      content = content.push(
        row![
          button(text("Replace").size(12))
            .on_press(Message::CoverReplace)
            .padding([4, 10]),
          button(text("Delete").size(12))
            .on_press(Message::CoverDelete)
            .padding([4, 10]),
        ]
        .spacing(6),
      );
    }
    else if self.selected_idx.is_some() {
      // No cover: render a placeholder matching the real cover's 240x240
      // framing, with a centered "Add Cover" button.
      content = content.push(Space::new().height(8));
      content = content.push(
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
      );
    }

    // ID3v1 read-only
    if let Some(v1) = &self.id3v1 {
      content = content.push(Space::new().height(10));
      content = content.push(text("ID3v1 (read-only)").size(11).color(MUTED));
      let v1_row = |lbl: &'static str, val: &str| -> Element<Message> {
        row![
          text(lbl).size(10).color(MUTED).width(Length::Fixed(56.0)),
          text(val.to_string()).size(10),
        ]
        .spacing(4)
        .into()
      };
      content = content.push(v1_row("Title", &v1.title));
      content = content.push(v1_row("Artist", &v1.artist));
      content = content.push(v1_row("Album", &v1.album));
      content = content.push(v1_row("Year", &v1.year));
      content = content.push(v1_row("Track", &v1.track));
      content = content.push(v1_row("Genre", &v1.genre));
      content = content.push(v1_row("Comment", &v1.comment));
      content = content.push(
        button(text("Delete ID3v1").size(10))
          .on_press(Message::Id3v1Delete)
          .padding([2, 8])
          .style(button::danger),
      );
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

    scrollable(content.padding(2)).height(Length::Fill).into()
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

  let tag = editable_tag(&tagged_file);

  let (title, artist, comment) = if let Some(t) = tag {
    (
      t.title().map(|s| s.to_string()).unwrap_or_default(),
      t.artist().map(|s| s.to_string()).unwrap_or_default(),
      t.comment().map(|s| s.to_string()).unwrap_or_default(),
    )
  }
  else {
    Default::default()
  };

  Ok(FileInfo {
    path: path.to_path_buf(),
    filename,
    title,
    artist,
    comment,
    duration_secs: duration,
    size_bytes: size,
  })
}

fn editable_tag(tagged_file: &lofty::file::TaggedFile) -> Option<&Tag> {
  tagged_file
    .tags()
    .iter()
    .find(|t| t.tag_type() != TagType::Id3v1)
    .or_else(|| tagged_file.primary_tag())
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
    form.artist = tag.artist().map(|s| s.to_string()).unwrap_or_default();
    form.album = tag.album().map(|s| s.to_string()).unwrap_or_default();
    form.album_artist = tag
      .get_string(ItemKey::AlbumArtist)
      .map(|s| s.to_string())
      .unwrap_or_default();
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
    form.genre = tag.genre().map(|s| s.to_string()).unwrap_or_default();
    form.comment = tag.comment().map(|s| s.to_string()).unwrap_or_default();
    form.composer = tag
      .get_string(ItemKey::Composer)
      .map(|s| s.to_string())
      .unwrap_or_default();
    form.compilation = tag
      .get_string(ItemKey::FlagCompilation)
      .map(|v| matches!(v.trim(), "1" | "true" | "yes"))
      .unwrap_or(false);

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
  let tagged_file = lofty::read_from_path(path).map_err(|e| e.to_string())?;

  let mut tag = match tagged_file
    .tags()
    .iter()
    .find(|t| t.tag_type() != TagType::Id3v1)
    .cloned()
  {
    Some(t) => t,
    None => Tag::new(tagged_file.primary_tag_type()),
  };

  apply_picture_change(&mut tag, pic_change)?;

  set_or_remove_string(&mut tag, ItemKey::TrackTitle, &form.title, |t, v| {
    t.set_title(v)
  });
  set_or_remove_string(&mut tag, ItemKey::TrackArtist, &form.artist, |t, v| {
    t.set_artist(v)
  });
  set_or_remove_string(&mut tag, ItemKey::AlbumTitle, &form.album, |t, v| {
    t.set_album(v)
  });
  set_or_remove_string(&mut tag, ItemKey::Genre, &form.genre, |t, v| {
    t.set_genre(v)
  });
  set_or_remove_string(&mut tag, ItemKey::Comment, &form.comment, |t, v| {
    t.set_comment(v)
  });

  if form.album_artist.is_empty() {
    tag.remove_key(ItemKey::AlbumArtist);
  }
  else {
    tag.insert_text(ItemKey::AlbumArtist, form.album_artist.clone());
  }

  if form.composer.is_empty() {
    tag.remove_key(ItemKey::Composer);
  }
  else {
    tag.insert_text(ItemKey::Composer, form.composer.clone());
  }

  if form.compilation {
    tag.insert_text(ItemKey::FlagCompilation, "1".to_string());
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

  tag
    .save_to_path(path, WriteOptions::default())
    .map_err(|e| e.to_string())?;

  Ok(())
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
      let mut new_pic =
        Picture::from_reader(&mut reader).map_err(|e| e.to_string())?;

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
      tag.insert_text(ItemKey::ReleaseDate, ts.to_string());
    }
    None => tag.remove_key(ItemKey::ReleaseDate),
  }

  Ok(())
}

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
}

static PLAYBACK: OnceLock<mpsc::Sender<PlaybackCmd>> = OnceLock::new();

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

  while let Ok(cmd) = rx.recv() {
    match cmd {
      PlaybackCmd::Play(path) => {
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
              let reader = BufReader::new(file);
              let decoder_result =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                  rodio::Decoder::new(reader)
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
      PlaybackCmd::Pause => {
        if let Some(p) = &player {
          p.pause();
        }
      }
      PlaybackCmd::Resume => {
        if let Some(p) = &player {
          p.play();
        }
      }
      PlaybackCmd::Stop => {
        if let Some(p) = player.take() {
          p.stop();
        }
      }
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
}
