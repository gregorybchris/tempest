//! What is worth telling the user about a transfer: that it completed or stopped on an error.
//! Both UIs notice the same things (D-226).

use std::collections::HashMap;

use tempest_protocol::BundleId;

use crate::{Status, TransferInfo};

/// Something worth telling the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    /// A download completed.
    Completed {
        /// Its name.
        name: String,
    },
    /// A transfer stopped on an error.
    Failed {
        /// Its name.
        name: String,
        /// Why.
        reason: String,
    },
}

impl Notice {
    /// A title and a sentence.
    pub fn text(&self) -> (String, String) {
        match self {
            Self::Completed { name } => (
                format!("{name} is complete"),
                "All files are downloaded and verified.".to_owned(),
            ),
            Self::Failed { name, reason } => (format!("{name} failed"), reason.clone()),
        }
    }
}

/// What a transfer's notices depend on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Seen {
    complete: bool,
    errored: bool,
}

impl Seen {
    /// What `transfer` shows now.
    pub fn of(transfer: &TransferInfo) -> Self {
        Self {
            complete: transfer.completed.is_some(),
            errored: matches!(transfer.status, Status::Errored { .. }),
        }
    }
}

/// What changed between `before` and `after` that is worth a notice. Transfers not seen
/// before, such as everything when the page opens, give none.
pub fn notices(before: &HashMap<BundleId, Seen>, after: &[TransferInfo]) -> Vec<Notice> {
    let mut notices = Vec::new();
    for transfer in after {
        let Some(was) = before.get(&transfer.id) else {
            continue;
        };
        let now = Seen::of(transfer);
        if now.complete && !was.complete {
            notices.push(Notice::Completed {
                name: transfer.name.clone(),
            });
        }
        if let Status::Errored { reason } = &transfer.status
            && !was.errored
        {
            notices.push(Notice::Failed {
                name: transfer.name.clone(),
                reason: reason.clone(),
            });
        }
    }
    notices
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completing_and_failing_are_noticed_once() {
        let mut download = TransferInfo::example(1, "film", 0);
        download.status = Status::Downloading;
        let mut other = TransferInfo::example(2, "music", 1);
        other.status = Status::Downloading;
        let before: HashMap<_, _> = [&download, &other]
            .iter()
            .map(|t| (t.id, Seen::of(t)))
            .collect();

        let mut done = download.clone();
        done.status = Status::Sharing;
        done.completed = Some(1);
        let mut failed = other.clone();
        failed.status = Status::Errored {
            reason: "The disk is full.".to_owned(),
        };
        let new = TransferInfo::example(3, "new", 2);
        let after = [done.clone(), failed.clone(), new];
        assert_eq!(
            notices(&before, &after),
            [
                Notice::Completed {
                    name: "film".to_owned()
                },
                Notice::Failed {
                    name: "music".to_owned(),
                    reason: "The disk is full.".to_owned()
                }
            ]
        );
        // Nothing new: nothing to say.
        let seen: HashMap<_, _> = after.iter().map(|t| (t.id, Seen::of(t))).collect();
        assert!(notices(&seen, &after).is_empty());
        // The first look says nothing.
        assert!(notices(&HashMap::new(), &after).is_empty());
    }
}
