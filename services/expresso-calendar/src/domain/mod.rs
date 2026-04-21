//! Domain layer — calendars + events persistence + iCalendar helpers.

pub mod calendar;
pub mod event;
pub mod ical;
pub mod freebusy;
pub mod itip;
pub mod rrule;

pub use calendar::{Calendar, CalendarRepo, NewCalendar, UpdateCalendar};
pub use event::{Event, EventRepo, EventQuery};
