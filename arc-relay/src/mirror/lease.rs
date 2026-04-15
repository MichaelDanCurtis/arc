use beacon_proto::ipc::DriverInfo;
use std::collections::HashMap;
use uuid::Uuid;

/// Manages the single-driver lease for one mirrored session.
///
/// Only the session owner can approve, deny, or revoke control;
/// eligible non-owners may request it via `try_grab`.
pub struct LeaseManager {
    owner_user_id: Uuid,
    eligible: Vec<Uuid>,           // user_ids that can request control
    driver: Option<DriverInfo>,
    pending: HashMap<String, PendingRequest>,
}

#[derive(Clone)]
pub struct PendingRequest {
    pub request_id: String,
    pub from_user: Uuid,
    pub from_device: String,
    pub created_at: i64,
}

/// Outcome returned by `try_grab`.
#[derive(Debug)]
pub enum GrabOutcome {
    Granted { driver: DriverInfo },
    Pending { request_id: String },
    Rejected { reason: String },
}

/// Outcome returned by `approve`.
#[derive(Debug)]
pub enum ApproveOutcome {
    Granted { driver: DriverInfo, request_id: String },
    AlreadyResolved,
    UnknownRequest,
    /// Returned when `by_user` is not the session owner.
    /// Distinct from `AlreadyResolved` so callers can emit a 403
    /// rather than silently treating it as a benign race.
    Unauthorized,
}

impl LeaseManager {
    pub fn new(owner_user_id: Uuid, eligible: Vec<Uuid>) -> Self {
        Self { owner_user_id, eligible, driver: None, pending: HashMap::new() }
    }

    pub fn driver(&self) -> Option<&DriverInfo> { self.driver.as_ref() }

    /// Return the Uuid of the session owner.
    pub fn owner_user_id(&self) -> Uuid { self.owner_user_id }

    pub fn pending_requests(&self) -> impl Iterator<Item = (&String, &PendingRequest)> {
        self.pending.iter()
    }

    /// Attempt to take the driver seat.
    ///
    /// - Owner: always granted immediately; clears any pending requests.
    /// - Eligible non-owner: creates a pending request awaiting owner approval.
    /// - Ineligible caller: rejected outright.
    pub fn try_grab(&mut self, user_id: Uuid, device_label: String) -> GrabOutcome {
        if user_id == self.owner_user_id {
            self.pending.clear();   // owner reclaim cancels pending requests
            let info = DriverInfo {
                user_id: user_id.to_string(),
                device_label,
                since: now_secs(),
            };
            self.driver = Some(info.clone());
            return GrabOutcome::Granted { driver: info };
        }
        if !self.eligible.contains(&user_id) {
            return GrabOutcome::Rejected { reason: "not eligible".into() };
        }
        let request_id = Uuid::new_v4().to_string();
        self.pending.insert(request_id.clone(), PendingRequest {
            request_id: request_id.clone(),
            from_user: user_id,
            from_device: device_label,
            created_at: now_secs(),
        });
        GrabOutcome::Pending { request_id }
    }

    /// Owner-class clients accept input even when they aren't currently
    /// the named driver — first keystroke just reclaims silently.
    pub fn try_implicit_reclaim(&mut self, user_id: Uuid, device_label: String) -> GrabOutcome {
        if user_id != self.owner_user_id {
            return GrabOutcome::Rejected { reason: "not owner-class".into() };
        }
        self.try_grab(user_id, device_label)
    }

    /// Owner approves a pending grab request, making that caller the driver.
    ///
    /// Returns `Unauthorized` if `by_user` is not the owner (caller should 403).
    /// Returns `AlreadyResolved` if the request_id is no longer in the pending map.
    pub fn approve(&mut self, request_id: &str, by_user: Uuid) -> ApproveOutcome {
        if by_user != self.owner_user_id {
            return ApproveOutcome::Unauthorized;
        }
        let req = match self.pending.remove(request_id) {
            Some(r) => r,
            None => return ApproveOutcome::AlreadyResolved,
        };
        // Drop any other pending requests too — only one driver at a time.
        self.pending.clear();
        let info = DriverInfo {
            user_id: req.from_user.to_string(),
            device_label: req.from_device,
            since: now_secs(),
        };
        self.driver = Some(info.clone());
        ApproveOutcome::Granted { driver: info, request_id: request_id.to_string() }
    }

    /// Owner denies a pending grab request. Returns true if the request existed.
    pub fn deny(&mut self, request_id: &str, by_user: Uuid) -> bool {
        if by_user != self.owner_user_id { return false; }
        self.pending.remove(request_id).is_some()
    }

    /// Owner revokes the current driver, clearing the driver slot.
    ///
    /// Returns true if there was a driver to remove.
    ///
    /// Note (sanctioned deviation): the driver slot is set to `None` rather
    /// than re-seating the owner with a synthetic `"owner"` device label.
    /// A fabricated DriverInfo would confuse presence broadcasts and
    /// `on_disconnect` matching (which keys on user_id + device_label).
    /// Callers that want immediate owner re-seat should follow with `try_grab`.
    pub fn revoke(&mut self, by_user: Uuid) -> bool {
        if by_user != self.owner_user_id { return false; }
        let was_some = self.driver.is_some();
        self.driver = None;
        was_some
    }

    /// Removes the driver and any pending requests associated with the
    /// disconnecting (user_id, device_label) pair.
    pub fn on_disconnect(&mut self, user_id: Uuid, device_label: &str) {
        if let Some(d) = &self.driver {
            if d.user_id == user_id.to_string() && d.device_label == device_label {
                self.driver = None;
            }
        }
        self.pending.retain(|_, p| p.from_user != user_id);
    }
}

fn now_secs() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn lease_for(owner: Uuid) -> LeaseManager {
        LeaseManager::new(owner, vec![])
    }

    #[test]
    fn owner_self_grab_unconditional() {
        let owner = Uuid::new_v4();
        let mut m = lease_for(owner);
        let outcome = m.try_grab(owner, "MacBook".into());
        assert!(matches!(outcome, GrabOutcome::Granted { .. }));
        assert_eq!(m.driver().unwrap().user_id, owner.to_string());
    }

    #[test]
    fn non_owner_request_creates_pending() {
        let owner = Uuid::new_v4();
        let bob = Uuid::new_v4();
        let mut m = LeaseManager::new(owner, vec![bob]);
        let outcome = m.try_grab(bob, "Bobs-iPad".into());
        assert!(matches!(outcome, GrabOutcome::Pending { .. }));
    }

    #[test]
    fn owner_reclaim_during_pending_cancels_request() {
        let owner = Uuid::new_v4();
        let bob = Uuid::new_v4();
        let mut m = LeaseManager::new(owner, vec![bob]);
        let GrabOutcome::Pending { request_id } = m.try_grab(bob, "B".into()) else { panic!() };
        let _ = m.try_grab(owner, "Mac".into());
        // Approval should now fail.
        assert!(matches!(m.approve(&request_id, owner), ApproveOutcome::AlreadyResolved));
        assert_eq!(m.driver().unwrap().user_id, owner.to_string());
    }

    #[test]
    fn first_approval_wins() {
        let owner = Uuid::new_v4();
        let bob = Uuid::new_v4();
        let mut m = LeaseManager::new(owner, vec![bob]);
        let GrabOutcome::Pending { request_id } = m.try_grab(bob, "B".into()) else { panic!() };
        assert!(matches!(m.approve(&request_id, owner), ApproveOutcome::Granted { .. }));
        assert!(matches!(m.approve(&request_id, owner), ApproveOutcome::AlreadyResolved));
    }

    #[test]
    fn driver_disconnect_clears_lease() {
        let owner = Uuid::new_v4();
        let mut m = lease_for(owner);
        m.try_grab(owner, "Mac".into());
        m.on_disconnect(owner, "Mac");
        assert!(m.driver().is_none());
    }

    #[test]
    fn implicit_reclaim_succeeds_for_owner() {
        let owner = Uuid::new_v4();
        let bob = Uuid::new_v4();
        let mut m = LeaseManager::new(owner, vec![bob]);
        let _ = m.try_grab(bob, "B".into());
        let req_id = m.pending_requests().next().unwrap().0.clone();
        m.approve(&req_id, owner);
        let result = m.try_implicit_reclaim(owner, "Mac".into());
        assert!(matches!(result, GrabOutcome::Granted { .. }));
        assert_eq!(m.driver().unwrap().user_id, owner.to_string());
    }

    #[test]
    fn implicit_reclaim_rejected_for_non_owner() {
        let owner = Uuid::new_v4();
        let bob = Uuid::new_v4();
        let mut m = LeaseManager::new(owner, vec![bob]);
        m.try_grab(owner, "Mac".into());
        let result = m.try_implicit_reclaim(bob, "B".into());
        assert!(matches!(result, GrabOutcome::Rejected { .. }));
    }

    #[test]
    fn approve_by_non_owner_returns_unauthorized() {
        let owner = Uuid::new_v4();
        let bob = Uuid::new_v4();
        let eve = Uuid::new_v4();
        let mut m = LeaseManager::new(owner, vec![bob, eve]);
        let GrabOutcome::Pending { request_id } = m.try_grab(bob, "B".into()) else { panic!() };
        let outcome = m.approve(&request_id, eve);
        assert!(matches!(outcome, ApproveOutcome::Unauthorized));
        // Request should still be pending — unauthorized approve is a no-op.
        assert!(m.pending_requests().any(|(id, _)| id == &request_id));
    }
}
