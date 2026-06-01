//! Domain layer — addressbooks + contacts persistence + vCard helpers.

pub mod addressbook;
pub mod contact;
pub mod dead_props;
pub mod group;
pub mod tombstone_gc;
pub mod vcard;

pub use addressbook::{Addressbook, AddressbookRepo, NewAddressbook, UpdateAddressbook};
pub use contact::{ContactAddressRow, ContactEmailRow, ContactRenameEntry, ContactRepo};
pub use dead_props::{DeadProp, DeadPropRepo};
pub use group::{ContactGroup, ContactGroupRepo, NewContactGroup, UpdateContactGroup};
