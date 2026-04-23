//! Domain layer — calendars + events persistence + iCalendar helpers.

pub mod calendar;
pub mod event;
pub mod ical;
pub mod freebusy;
pub mod itip;
pub mod rrule;
pub mod dead_props;
pub mod tombstone_gc;
pub mod counter;

pub use calendar::{Calendar, CalendarRepo, NewCalendar, UpdateCalendar};
pub use event::{Event, EventRepo, EventQuery};
pub use dead_props::{DeadProp, DeadPropRepo};
pub use counter::{CounterProposal, CounterRepo};
