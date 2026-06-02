//! Exchange ActiveSync WBXML token tables (MS-ASWBXML).
//!
//! Each EAS namespace is a codepage; within a page a tag is a 6-bit token. We
//! only define the pages + tokens the MVP needs (AirSync, FolderHierarchy,
//! Provision) plus the two the mail Sync will need (Email, AirSyncBase). Tokens
//! are exposed as named constants so the server code reads clearly; `tag_name`
//! resolves `(page, token)` back to a name for logging/debugging.

/// EAS codepage numbers (the value carried by `SWITCH_PAGE`).
pub mod page {
    pub const AIR_SYNC: u8 = 0;
    pub const EMAIL: u8 = 2;
    pub const CALENDAR: u8 = 4;
    pub const FOLDER_HIERARCHY: u8 = 7;
    pub const CONTACTS: u8 = 1;
    pub const GET_ITEM_ESTIMATE: u8 = 6;
    pub const PING: u8 = 13;
    pub const PROVISION: u8 = 14;
    pub const AIR_SYNC_BASE: u8 = 17;
}

/// GetItemEstimate (page 6) tokens — pre-Sync item count (subset).
pub mod item_estimate {
    pub const GET_ITEM_ESTIMATE: u8 = 0x05;
    pub const RESPONSE: u8 = 0x07;
    pub const STATUS: u8 = 0x08;
    pub const COLLECTION: u8 = 0x09;
    pub const COLLECTION_ID: u8 = 0x0C;
    pub const ESTIMATE: u8 = 0x0D;
}

/// Calendar (page 4) tokens — Sync ApplicationData for calendar items (subset).
pub mod calendar {
    pub const TIMEZONE: u8 = 0x05;
    pub const ALL_DAY_EVENT: u8 = 0x06;
    pub const DTSTAMP: u8 = 0x08;
    pub const END_TIME: u8 = 0x09;
    pub const LOCATION: u8 = 0x0E;
    pub const REMINDER: u8 = 0x0F;
    pub const SUBJECT: u8 = 0x14;
    pub const START_TIME: u8 = 0x15;
    pub const UID: u8 = 0x17;
}

/// Contacts (page 1) tokens — Sync ApplicationData for contact items (subset).
pub mod contacts {
    pub const EMAIL1_ADDRESS: u8 = 0x0F;
    pub const FILE_AS: u8 = 0x13;
    pub const FIRST_NAME: u8 = 0x15;
    pub const LAST_NAME: u8 = 0x19;
    pub const MOBILE_PHONE: u8 = 0x1B;
    pub const COMPANY_NAME: u8 = 0x12;
}

/// Ping (page 13) tokens — Ping/Direct-Push command.
pub mod ping {
    pub const PING: u8 = 0x05;
    pub const STATUS: u8 = 0x07;
    pub const HEARTBEAT_INTERVAL: u8 = 0x08;
    pub const FOLDERS: u8 = 0x09;
    pub const FOLDER: u8 = 0x0A;
    pub const ID: u8 = 0x0B;
    pub const CLASS: u8 = 0x0C;
    pub const MAX_FOLDERS: u8 = 0x0D;
}

/// AirSync (page 0) tokens — the Sync command envelope.
pub mod air_sync {
    pub const SYNC: u8 = 0x05;
    pub const COLLECTIONS: u8 = 0x1C;
    pub const COLLECTION: u8 = 0x0F;
    pub const SYNC_KEY: u8 = 0x0B;
    pub const COLLECTION_ID: u8 = 0x12;
    pub const STATUS: u8 = 0x0E;
    pub const COMMANDS: u8 = 0x16;
    pub const ADD: u8 = 0x07;
    pub const DELETE: u8 = 0x09;
    pub const CHANGE: u8 = 0x08;
    pub const SERVER_ID: u8 = 0x0D;
    pub const APPLICATION_DATA: u8 = 0x1D;
    pub const MORE_AVAILABLE: u8 = 0x14;
    pub const WINDOW_SIZE: u8 = 0x15;
    pub const GET_CHANGES: u8 = 0x13;
}

/// FolderHierarchy (page 7) tokens — FolderSync command.
pub mod folder {
    pub const FOLDER_SYNC: u8 = 0x16;
    pub const STATUS: u8 = 0x0C;
    pub const SYNC_KEY: u8 = 0x12;
    pub const CHANGES: u8 = 0x0E;
    pub const COUNT: u8 = 0x17;
    pub const ADD: u8 = 0x0F;
    pub const SERVER_ID: u8 = 0x08;
    pub const PARENT_ID: u8 = 0x09;
    pub const DISPLAY_NAME: u8 = 0x0A;
    pub const TYPE: u8 = 0x0B;
}

/// Provision (page 14) tokens — Provision command.
pub mod provision {
    pub const PROVISION: u8 = 0x05;
    pub const POLICIES: u8 = 0x06;
    pub const POLICY: u8 = 0x07;
    pub const POLICY_TYPE: u8 = 0x08;
    pub const POLICY_KEY: u8 = 0x09;
    pub const STATUS: u8 = 0x0B;
    pub const DATA: u8 = 0x0A;
}

/// Email (page 2) tokens — used by mail Sync ApplicationData (subset).
pub mod email {
    pub const TO: u8 = 0x0F;
    pub const FROM: u8 = 0x11;
    pub const SUBJECT: u8 = 0x14;
    pub const DATE_RECEIVED: u8 = 0x10;
    pub const DISPLAY_TO: u8 = 0x12;
    pub const IMPORTANCE: u8 = 0x13;
    pub const READ: u8 = 0x15;
    pub const MESSAGE_CLASS: u8 = 0x1B;
}

/// AirSyncBase (page 17) tokens — body container for mail Sync (subset).
pub mod air_sync_base {
    pub const BODY_PREFERENCE: u8 = 0x05;
    pub const TYPE: u8 = 0x06;
    pub const TRUNCATION_SIZE: u8 = 0x07;
    pub const BODY: u8 = 0x0A;
    pub const DATA: u8 = 0x0B;
    pub const ESTIMATED_DATA_SIZE: u8 = 0x0C;
    pub const TRUNCATED: u8 = 0x0D;
    pub const NATIVE_BODY_TYPE: u8 = 0x16;
}

/// Resolve `(page, token)` to a human-readable `Page:Tag` name, or `None` when
/// unknown. For logging/debugging only — the wire format uses the numbers.
pub fn tag_name(page: u8, token: u8) -> Option<&'static str> {
    let name = match (page, token) {
        (page::AIR_SYNC, air_sync::SYNC) => "AirSync:Sync",
        (page::AIR_SYNC, air_sync::COLLECTIONS) => "AirSync:Collections",
        (page::AIR_SYNC, air_sync::COLLECTION) => "AirSync:Collection",
        (page::AIR_SYNC, air_sync::SYNC_KEY) => "AirSync:SyncKey",
        (page::AIR_SYNC, air_sync::COLLECTION_ID) => "AirSync:CollectionId",
        (page::AIR_SYNC, air_sync::STATUS) => "AirSync:Status",
        (page::AIR_SYNC, air_sync::COMMANDS) => "AirSync:Commands",
        (page::AIR_SYNC, air_sync::ADD) => "AirSync:Add",
        (page::AIR_SYNC, air_sync::APPLICATION_DATA) => "AirSync:ApplicationData",
        (page::FOLDER_HIERARCHY, folder::FOLDER_SYNC) => "FolderHierarchy:FolderSync",
        (page::FOLDER_HIERARCHY, folder::SYNC_KEY) => "FolderHierarchy:SyncKey",
        (page::FOLDER_HIERARCHY, folder::ADD) => "FolderHierarchy:Add",
        (page::FOLDER_HIERARCHY, folder::DISPLAY_NAME) => "FolderHierarchy:DisplayName",
        (page::FOLDER_HIERARCHY, folder::TYPE) => "FolderHierarchy:Type",
        (page::PROVISION, provision::PROVISION) => "Provision:Provision",
        (page::PROVISION, provision::POLICY_KEY) => "Provision:PolicyKey",
        (page::PROVISION, provision::STATUS) => "Provision:Status",
        (page::PING, ping::PING) => "Ping:Ping",
        (page::PING, ping::STATUS) => "Ping:Status",
        (page::PING, ping::FOLDER) => "Ping:Folder",
        _ => return None,
    };
    Some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_match_ms_aswbxml() {
        assert_eq!(page::AIR_SYNC, 0);
        assert_eq!(page::CONTACTS, 1);
        assert_eq!(page::EMAIL, 2);
        assert_eq!(page::CALENDAR, 4);
        assert_eq!(page::GET_ITEM_ESTIMATE, 6);
        assert_eq!(page::FOLDER_HIERARCHY, 7);
        assert_eq!(page::PING, 13);
        assert_eq!(page::PROVISION, 14);
        assert_eq!(page::AIR_SYNC_BASE, 17);
    }

    #[test]
    fn tag_name_resolves_known() {
        assert_eq!(
            tag_name(page::FOLDER_HIERARCHY, folder::FOLDER_SYNC),
            Some("FolderHierarchy:FolderSync")
        );
        assert_eq!(
            tag_name(page::AIR_SYNC, air_sync::SYNC),
            Some("AirSync:Sync")
        );
        assert_eq!(
            tag_name(page::PROVISION, provision::POLICY_KEY),
            Some("Provision:PolicyKey")
        );
    }

    #[test]
    fn tag_name_unknown_is_none() {
        assert_eq!(tag_name(99, 0x01), None);
        assert_eq!(tag_name(page::AIR_SYNC, 0x3F), None);
    }

    #[test]
    fn tokens_are_within_6_bits() {
        // Every tag token must fit the low 6 bits (the high 2 are global flags).
        for t in [
            air_sync::SYNC,
            air_sync::APPLICATION_DATA,
            folder::FOLDER_SYNC,
            provision::PROVISION,
            email::MESSAGE_CLASS,
            air_sync_base::NATIVE_BODY_TYPE,
            ping::PING,
            ping::MAX_FOLDERS,
            calendar::UID,
            calendar::START_TIME,
            contacts::FIRST_NAME,
            contacts::EMAIL1_ADDRESS,
        ] {
            assert!(t <= 0x3F, "token {t:#x} exceeds 6 bits");
        }
    }
}
