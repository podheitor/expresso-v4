//! Domain layer — calendars + events persistence + iCalendar helpers.

pub mod alarm_delivery;
pub mod calendar;
pub mod counter;
pub mod dead_props;
pub mod event;
pub mod freebusy;
pub mod ical;
pub mod itip;
pub mod rrule;
pub mod tombstone_gc;

pub use calendar::{Calendar, CalendarRepo, NewCalendar, UpdateCalendar};
pub use dead_props::{DeadProp, DeadPropRepo};
pub use event::{Event, EventQuery, EventRepo};
