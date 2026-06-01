//! Domain layer — notes persistence.

pub mod note;
pub mod notebook;
pub mod tag;

pub use note::{NewNote, Note, NoteRepo, SharedNote, UpdateNote};
pub use notebook::{NewNotebook, Notebook, NotebookRepo, UpdateNotebook};
pub use tag::NoteTagRepo;
