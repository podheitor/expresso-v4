//! Domain layer — addressbooks + contacts persistence + vCard helpers.

pub mod addressbook;
pub mod contact;
pub mod vcard;

pub use addressbook::{Addressbook, AddressbookRepo, NewAddressbook, UpdateAddressbook};
pub use contact::{Contact, ContactRepo};
