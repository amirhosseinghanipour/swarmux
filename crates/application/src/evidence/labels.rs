//! Outcome labels: what became of the delivered work, asked of GitHub and
//! appended to the ledger as it is learned.

use swarmux_domain::evidence::{LabelKind, SETTLE_MS, label_of};

use super::{EvidenceService, ProbeRequest, Stored};
use crate::{AppError, AppResult};

impl EvidenceService {
    /// A bundle whose label can still change: it has a branch, was not
    /// reverted and wasn't merged long enough ago to be settled.
    fn open_to_change(&self, s: &Stored) -> bool {
        if s.bundle.branch.as_deref().is_none_or(str::is_empty) {
            return false;
        }
        match s.labels.last() {
            Some(l) if l.kind == LabelKind::Reverted => false,
            Some(l) if l.kind == LabelKind::Merged => self.clock.now() - l.at < SETTLE_MS,
            _ => true,
        }
    }

    /// Asks GitHub about the branch of every bundle whose label can still
    /// change. Answers arrive through [`EvidenceService::pump_labels`].
    /// Returns how many questions were asked.
    pub fn request_labels(&self) -> AppResult<usize> {
        let Some(probe) = &self.probe else {
            return Err(AppError::Mission("GitHub is not available".into()));
        };
        let mut asked = 0;
        for s in self.store.list()? {
            let slug = s.bundle.contract.slug.clone();
            if !self.open_to_change(&s) || self.probing.borrow().contains(&slug) {
                continue;
            }
            let Some(branch) = s.bundle.branch.clone() else {
                continue;
            };
            self.probing.borrow_mut().insert(slug.clone());
            let tx = self.probe_tx.clone();
            let key = slug.clone();
            probe.probe(
                ProbeRequest {
                    slug,
                    repo: s.bundle.repo.clone().into(),
                    branch,
                },
                Box::new(move |reply| {
                    // Nobody listening (the app is closing): nothing to keep.
                    let _ = tx.send((key, reply));
                }),
            );
            asked += 1;
        }
        Ok(asked)
    }

    /// Questions are out.
    pub fn probing_labels(&self) -> bool {
        !self.probing.borrow().is_empty()
    }

    /// Why the last question failed, if it did.
    pub fn label_problem(&self) -> Option<String> {
        self.problem.borrow().clone()
    }

    /// Takes the answers that arrived and labels the missions they are
    /// about. The slugs whose label changed.
    pub fn pump_labels(&self) -> Vec<String> {
        let mut changed = Vec::new();
        let rx = self.probe_rx.borrow();
        while let Ok((slug, reply)) = rx.try_recv() {
            self.probing.borrow_mut().remove(&slug);
            match reply {
                Ok(Some(pr)) => match self.label(&slug, label_of(&pr, self.clock.now())) {
                    Ok(true) => changed.push(slug),
                    Ok(false) => {}
                    Err(e) => *self.problem.borrow_mut() = Some(e.to_string()),
                },
                Ok(None) => {}
                Err(why) => *self.problem.borrow_mut() = Some(why),
            }
        }
        changed
    }

    /// Attaches `label` unless the mission already carries it.
    fn label(&self, slug: &str, label: swarmux_domain::evidence::Label) -> AppResult<bool> {
        let Some(mut stored) = self.store.get(slug)? else {
            return Ok(false);
        };
        let same = stored
            .labels
            .last()
            .is_some_and(|l| l.kind == label.kind && l.pr == label.pr);
        if same {
            return Ok(false);
        }
        stored.labels.push(label);
        self.store.set_labels(slug, &stored.labels)?;
        self.ledger
            .record_label(slug, Some(stored.bundle.job_id()), &label, self.clock.now())?;
        Ok(true)
    }
}
