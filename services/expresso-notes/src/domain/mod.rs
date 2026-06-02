//! Domain layer — notes persistence.

pub mod note;
pub mod notebook;
pub mod tag;
pub mod version;

pub use note::{ExportNote, NewNote, Note, NoteRepo, SharedNote, UpdateNote};
pub use notebook::{NewNotebook, Notebook, NotebookRepo, UpdateNotebook};
pub use tag::{NoteTagRepo, TagCount, TagPairCount};
pub use version::{NoteSnapshot, NoteVersion, NoteVersionRepo};
