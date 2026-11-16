//! Profile draft / dirty model (LOGIC-FIX-PLAN §4.4, D-1).
//!
//! The tuning screens edit an in-memory [`ProfileDraft`] of the active
//! profile, never the stored profile: `set_tuning` updates the draft
//! and marks it dirty, the pipeline push applies it live, and only an
//! explicit Save writes it to disk. A profile switch while the draft
//! is dirty must ask Save / Discard (preview §8.4) instead of
//! silently keeping or losing the edits.

use std::collections::BTreeSet;

use rsh_profiles::csharp_import::WheelProfile;

/// In-memory draft of the active profile: the stored profile plus
/// the unsaved tuning edits, and which modules they touch.
#[derive(Clone, Debug)]
pub struct ProfileDraft {
    /// The profile as edited (starts as a clone of the stored
    /// active profile).
    pub profile: WheelProfile,
    /// True once any tuning edit landed since the last save/discard.
    pub dirty: bool,
    /// Module ids edited since the last save/discard (canonical ids,
    /// e.g. `"steering"`, `"clutch"`), in sorted order.
    pub dirty_modules: BTreeSet<String>,
}

impl ProfileDraft {
    /// A clean draft over a stored profile.
    #[must_use]
    pub fn new(profile: WheelProfile) -> Self {
        Self {
            profile,
            dirty: false,
            dirty_modules: BTreeSet::new(),
        }
    }

    /// Record one module edit: the draft is dirty from here until a
    /// save or a discard.
    pub fn mark_dirty(&mut self, module: &str) {
        self.dirty = true;
        self.dirty_modules.insert(module.to_string());
    }

    /// The draft now matches disk (after a save).
    pub fn mark_clean(&mut self) {
        self.dirty = false;
        self.dirty_modules.clear();
    }

    /// Serializable status for the UI's Save / Discard prompt.
    #[must_use]
    pub fn status(&self, profile_id: &str) -> DraftStatus {
        DraftStatus {
            profile_id: profile_id.to_string(),
            profile_name: self.profile.name.clone(),
            dirty: self.dirty,
            dirty_modules: self.dirty_modules.iter().cloned().collect(),
        }
    }
}

/// Draft status, as the UI prompt (and the `AppApi` surface)
/// consumes it. Moved to `rsh_api::tuning` (L4) with the other
/// tuning wire types; re-exported here so the draft model keeps
/// its existing path.
pub use rsh_api::tuning::DraftStatus;

#[cfg(test)]
mod tests {
    use rsh_profiles::csharp_import::WheelProfile;

    use super::ProfileDraft;

    #[test]
    fn draft_starts_clean_and_tracks_dirty_modules() {
        let mut draft = ProfileDraft::new(WheelProfile {
            name: "GT3 Sprint".to_string(),
            ..WheelProfile::default()
        });
        assert!(!draft.dirty);
        assert!(draft.dirty_modules.is_empty());
        let status = draft.status("p1");
        assert_eq!(status.profile_name, "GT3 Sprint");
        assert!(!status.dirty);
        assert_eq!(status.dirty_modules, Vec::<String>::new());

        draft.mark_dirty("clutch");
        draft.mark_dirty("steering");
        assert!(draft.dirty);
        let status = draft.status("p1");
        assert!(status.dirty);
        assert_eq!(status.dirty_modules, vec!["clutch", "steering"]);
        // Serializes as the UI shape (camelCase).
        let json = serde_json::to_string(&status).expect("serialize");
        assert!(json.contains("\"profileName\":\"GT3 Sprint\""));
        assert!(json.contains("\"dirtyModules\":[\"clutch\",\"steering\"]"));

        draft.mark_clean();
        assert!(!draft.dirty);
        assert!(draft.dirty_modules.is_empty());
    }
}
