//! In-process presence tracker — who is currently online, with TTL expiry.
//!
//! A heartbeat (POST .../presence/heartbeat) upserts the caller's last-seen
//! `Instant`. A user counts as online while their last heartbeat is younger
//! than `ttl`. A background sweep task (spawned in `main`) periodically drops
//! entries older than `ttl` and reports them so the caller can publish an
//! `offline` event on the bus.
//!
//! Scope: process-local by design, mirroring `ChatBus` — every chat write goes
//! through this instance, and presence is the most ephemeral signal of all (a
//! stale entry self-heals within `ttl`). Cross-instance presence would need a
//! shared store (Redis/NATS); deliberately out of scope for this MVP, same as
//! the bus's documented in-process limitation.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use uuid::Uuid;

/// Default time a heartbeat keeps a user online. A client should heartbeat at
/// roughly `ttl / 2` to stay continuously present across one missed beat.
pub const DEFAULT_TTL: Duration = Duration::from_secs(30);

/// Tracks last-seen instants keyed by `(tenant_id, user_id)`.
pub struct PresenceTracker {
    ttl: Duration,
    seen: Mutex<HashMap<(Uuid, Uuid), Instant>>,
}

impl PresenceTracker {
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            seen: Mutex::new(HashMap::new()),
        }
    }

    pub fn ttl(&self) -> Duration {
        self.ttl
    }

    /// Record a heartbeat for `(tenant, user)` at `now`. Returns `true` when the
    /// user transitioned from offline→online (no live entry existed), so the
    /// caller knows to publish an `online` event; `false` on a refresh.
    pub fn heartbeat_at(&self, tenant: Uuid, user: Uuid, now: Instant) -> bool {
        let mut map = self.seen.lock().expect("presence mutex");
        let was_online = map
            .get(&(tenant, user))
            .is_some_and(|seen| now.duration_since(*seen) < self.ttl);
        map.insert((tenant, user), now);
        !was_online
    }

    /// The set of users in `tenant` whose last heartbeat is younger than `ttl`
    /// as of `now`. Order is unspecified.
    pub fn online_at(&self, tenant: Uuid, now: Instant) -> Vec<Uuid> {
        let map = self.seen.lock().expect("presence mutex");
        map.iter()
            .filter(|((t, _), seen)| *t == tenant && now.duration_since(**seen) < self.ttl)
            .map(|((_, u), _)| *u)
            .collect()
    }

    /// Drop entries older than `ttl` as of `now`; return the expired keys so the
    /// caller can publish `offline` events. Called by the sweep task.
    pub fn sweep_at(&self, now: Instant) -> Vec<(Uuid, Uuid)> {
        let mut map = self.seen.lock().expect("presence mutex");
        let expired: Vec<(Uuid, Uuid)> = map
            .iter()
            .filter(|(_, seen)| now.duration_since(**seen) >= self.ttl)
            .map(|(k, _)| *k)
            .collect();
        for k in &expired {
            map.remove(k);
        }
        expired
    }
}

impl Default for PresenceTracker {
    fn default() -> Self {
        Self::new(DEFAULT_TTL)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids() -> (Uuid, Uuid) {
        (Uuid::new_v4(), Uuid::new_v4())
    }

    #[test]
    fn first_heartbeat_marks_transition_online() {
        let p = PresenceTracker::new(Duration::from_secs(30));
        let (t, u) = ids();
        let now = Instant::now();
        assert!(p.heartbeat_at(t, u, now), "first beat = offline→online");
        assert!(!p.heartbeat_at(t, u, now), "refresh = no transition");
    }

    #[test]
    fn refresh_after_expiry_is_a_new_transition() {
        let ttl = Duration::from_secs(30);
        let p = PresenceTracker::new(ttl);
        let (t, u) = ids();
        let t0 = Instant::now();
        assert!(p.heartbeat_at(t, u, t0));
        // A heartbeat past the TTL counts as a fresh online transition.
        let later = t0 + Duration::from_secs(31);
        assert!(p.heartbeat_at(t, u, later));
    }

    #[test]
    fn online_snapshot_excludes_expired() {
        let ttl = Duration::from_secs(30);
        let p = PresenceTracker::new(ttl);
        let (t, u1) = ids();
        let u2 = Uuid::new_v4();
        let t0 = Instant::now();
        p.heartbeat_at(t, u1, t0);
        p.heartbeat_at(t, u2, t0 + Duration::from_secs(20));
        // At t0+35s, u1 (35s old) is gone, u2 (15s old) remains.
        let online = p.online_at(t, t0 + Duration::from_secs(35));
        assert_eq!(online, vec![u2]);
    }

    #[test]
    fn online_snapshot_is_tenant_scoped() {
        let p = PresenceTracker::new(Duration::from_secs(30));
        let (t1, u) = ids();
        let t2 = Uuid::new_v4();
        let now = Instant::now();
        p.heartbeat_at(t1, u, now);
        assert_eq!(p.online_at(t1, now), vec![u]);
        assert!(p.online_at(t2, now).is_empty());
    }

    #[test]
    fn sweep_returns_and_removes_expired_only() {
        let ttl = Duration::from_secs(30);
        let p = PresenceTracker::new(ttl);
        let (t, u1) = ids();
        let u2 = Uuid::new_v4();
        let t0 = Instant::now();
        p.heartbeat_at(t, u1, t0);
        p.heartbeat_at(t, u2, t0 + Duration::from_secs(25));
        // At t0+31s: u1 expired (31s), u2 live (6s).
        let expired = p.sweep_at(t0 + Duration::from_secs(31));
        assert_eq!(expired, vec![(t, u1)]);
        // u1 removed → no longer reported; second sweep finds nothing new.
        assert!(p.sweep_at(t0 + Duration::from_secs(31)).is_empty());
        assert_eq!(p.online_at(t, t0 + Duration::from_secs(31)), vec![u2]);
    }

    #[test]
    fn online_reflects_ttl_boundary() {
        let ttl = Duration::from_secs(30);
        let p = PresenceTracker::new(ttl);
        let (t, u) = ids();
        let t0 = Instant::now();
        p.heartbeat_at(t, u, t0);
        assert_eq!(p.online_at(t, t0 + Duration::from_secs(29)), vec![u]);
        assert!(p.online_at(t, t0 + Duration::from_secs(30)).is_empty());
    }

    #[test]
    fn default_ttl_is_thirty_seconds() {
        assert_eq!(PresenceTracker::default().ttl(), DEFAULT_TTL);
    }
}
