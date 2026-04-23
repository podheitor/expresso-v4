//! Domain layer — addressbooks + contacts persistence + vCard helpers.

pub mod addressbook;
pub mod contact;
pub mod vcard;
pub mod dead_props;
pub mod tombstone_gc;

pub use addressbook::{Addressbook, AddressbookRepo, NewAddressbook, UpdateAddressbook};
pub use contact::{Contact, ContactRepo};
pub use dead_props::{DeadProp, DeadPropRepo};
