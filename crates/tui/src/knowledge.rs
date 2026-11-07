//! The Knowledge screen: rules, facts and decisions by kind and standing,
//! the rule registry, stale items to confirm and shadow rules' hits, and
//! the keys that add, edit, promote, confirm and retire them.

use std::rc::Rc;

use crossterm::event::KeyCode;
use swarmux_application::{AppError, AppResult, ContractRow, ContractService, KnowledgeService};
use swarmux_domain::AttentionItem;
use swarmux_domain::knowledge::{Draft, Home, Item, ItemId, Kind, Registry, Status};

use crate::app::{App, InputPurpose, Mode, Screen};
use crate::ui::input::Input;

/// The Knowledge screen's selection.
#[derive(Default)]
pub struct KnowledgeScreen {
    /// The selected item, by place among the items listed.
    pub selected: usize,
    /// The item whose text is being edited.
    pub editing: Option<ItemId>,
    /// Showing the contracts between repositories instead of the items.
    pub contracts_tab: bool,
    /// The selected contract, by place among `contract_rows`.
    pub contract_selected: usize,
    /// The workspace whose contracts are shown, by place in the list.
    pub contract_workspace: usize,
    /// The contracts of that workspace, read when the tab opens or `r`.
    pub contract_rows: Vec<ContractRow>,
}

/// One row of the screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KnowledgeRow {
    /// A group and how many items follow.
    Header(String, usize),
    Item(Box<Item>),
}

/// The groups, in the order they are listed: what needs the owner first.
fn group_of(i: &Item) -> (u8, &'static str) {
    match (i.status, i.kind) {
        (Status::Stale, _) => (0, "stale: confirm (c) or edit (e)"),
        (Status::Proposed, _) => (1, "proposed"),
        (Status::Shadow, _) => (2, "rules in shadow: they log, p makes them act"),
        (_, Kind::Rule) => (3, "rules"),
        (_, Kind::Fact) => (4, "facts"),
        (_, Kind::Decision) => (5, "decisions"),
        _ => (6, "other"),
    }
}

impl App {
    /// Lets the Knowledge screen show and change the items, and puts
    /// stale ones in the inbox.
    pub fn with_knowledge(mut self, knowledge: Option<Rc<KnowledgeService>>) -> App {
        self.set_knowledge(knowledge);
        self
    }

    pub(crate) fn set_knowledge(&mut self, knowledge: Option<Rc<KnowledgeService>>) {
        self.engine.knowledge = knowledge;
    }

    /// Whether knowledge is on.
    pub fn knowledge_on(&self) -> bool {
        self.engine.knowledge.is_some()
    }

    /// Lets the Contracts tab show who consumes what, and puts changes
    /// that break repositories outside their mission in the inbox.
    pub fn with_contracts(mut self, contracts: Option<Rc<ContractService>>) -> App {
        self.engine.contracts = contracts;
        self
    }

    /// Stale knowledge and contract breaks outside a mission, for the inbox.
    pub(crate) fn knowledge_items(&self) -> Vec<AttentionItem> {
        let mut items = self
            .engine
            .knowledge
            .as_ref()
            .map(|k| k.stale_items())
            .unwrap_or_default();
        if let Some(c) = &self.engine.contracts {
            items.extend(c.attention());
        }
        items
    }

    /// The workspace whose contracts the tab shows.
    pub fn contract_workspace(&self) -> Option<&swarmux_domain::Workspace> {
        let n = self.workspaces.len();
        (n > 0).then(|| &self.workspaces[self.knowledge_screen.contract_workspace % n])
    }

    /// Reads the shown workspace's contracts again, files that changed only.
    fn refresh_contracts(&mut self, rescan: bool) {
        let (Some(c), Some(w)) = (
            self.engine.contracts.clone(),
            self.contract_workspace().cloned(),
        ) else {
            self.knowledge_screen.contract_rows.clear();
            return;
        };
        if rescan && let Err(e) = c.rescan(&w) {
            self.error = Some(e.to_string());
        }
        match c.rows(w.id) {
            Ok(rows) => {
                self.knowledge_screen.contract_selected = self
                    .knowledge_screen
                    .contract_selected
                    .min(rows.len().saturating_sub(1));
                self.knowledge_screen.contract_rows = rows;
            }
            Err(e) => self.error = Some(e.to_string()),
        }
    }

    /// Who consumes the selected contract.
    pub fn selected_contract_sites(&self) -> Vec<swarmux_domain::contracts::Site> {
        let (Some(c), Some(w)) = (&self.engine.contracts, self.contract_workspace()) else {
            return Vec::new();
        };
        let Some(row) = self
            .knowledge_screen
            .contract_rows
            .get(self.knowledge_screen.contract_selected)
        else {
            return Vec::new();
        };
        c.consumers(w.id, &row.provided.sym.label())
            .unwrap_or_default()
            .into_iter()
            .find(|k| k.provided == row.provided)
            .map(|k| k.sites)
            .unwrap_or_default()
    }

    /// The registry over every home.
    pub fn knowledge_registry(&self) -> Option<Registry> {
        self.engine.knowledge.as_ref().map(|k| k.registry(None))
    }

    /// The screen's rows: retired items left out, each group under a header.
    pub fn knowledge_rows(&self) -> Vec<KnowledgeRow> {
        let Some(k) = &self.engine.knowledge else {
            return Vec::new();
        };
        let mut items: Vec<Item> = k
            .list()
            .into_iter()
            .filter(|i| i.status != Status::Retired)
            .collect();
        items.sort_by(|a, b| group_of(a).0.cmp(&group_of(b).0).then(a.id.cmp(&b.id)));
        let mut rows = Vec::new();
        let mut at = 0;
        while at < items.len() {
            let (rank, name) = group_of(&items[at]);
            let end = items[at..]
                .iter()
                .position(|i| group_of(i).0 != rank)
                .map_or(items.len(), |n| at + n);
            rows.push(KnowledgeRow::Header(name.to_string(), end - at));
            rows.extend(
                items[at..end]
                    .iter()
                    .map(|i| KnowledgeRow::Item(Box::new(i.clone()))),
            );
            at = end;
        }
        rows
    }

    /// The items of [`App::knowledge_rows`], without the headers.
    pub fn knowledge_list(&self) -> Vec<Item> {
        self.knowledge_rows()
            .into_iter()
            .filter_map(|r| match r {
                KnowledgeRow::Item(i) => Some(*i),
                KnowledgeRow::Header(..) => None,
            })
            .collect()
    }

    fn selected_knowledge(&self) -> Option<Item> {
        self.knowledge_list()
            .into_iter()
            .nth(self.knowledge_screen.selected)
    }

    /// Where a new item goes: the selected item's home, else the first
    /// workspace, else the folder swarmux was started in.
    fn knowledge_home(&self) -> Home {
        if let Some(i) = self.selected_knowledge() {
            return i.home;
        }
        match self.workspaces.first() {
            Some(w) => Home::Workspace(w.id),
            None => Home::repo(&self.cwd().to_string_lossy()),
        }
    }

    /// The Knowledge screen's keys: `↑↓` select, `a` add, `e` edit the
    /// text, `p` promote, `c` confirm, `x` retire. Whether the key was taken.
    pub(crate) fn knowledge_key(&mut self, code: KeyCode) -> bool {
        if self.screen != Screen::Knowledge {
            return false;
        }
        if code == KeyCode::Char('t') && self.engine.contracts.is_some() {
            self.knowledge_screen.contracts_tab = !self.knowledge_screen.contracts_tab;
            if self.knowledge_screen.contracts_tab {
                self.refresh_contracts(false);
            }
            return true;
        }
        if self.knowledge_screen.contracts_tab {
            let count = self.knowledge_screen.contract_rows.len();
            match code {
                KeyCode::Up | KeyCode::Char('k') => {
                    self.knowledge_screen.contract_selected =
                        self.knowledge_screen.contract_selected.saturating_sub(1);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.knowledge_screen.contract_selected =
                        (self.knowledge_screen.contract_selected + 1).min(count.saturating_sub(1));
                }
                KeyCode::Char('r') => {
                    self.refresh_contracts(true);
                    self.notice = Some("contracts read again (only changed files)".into());
                }
                KeyCode::Char('w') => {
                    self.knowledge_screen.contract_workspace += 1;
                    self.knowledge_screen.contract_selected = 0;
                    self.refresh_contracts(false);
                }
                _ => return false,
            }
            return true;
        }
        let count = self.knowledge_list().len();
        match code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.knowledge_screen.selected = self.knowledge_screen.selected.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.knowledge_screen.selected =
                    (self.knowledge_screen.selected + 1).min(count.saturating_sub(1));
            }
            KeyCode::Char('a') if self.knowledge_on() => {
                self.mode = Mode::Input(InputPurpose::KnowledgeAdd, Input::default());
            }
            KeyCode::Char('e') => {
                if let Some(i) = self.selected_knowledge() {
                    self.knowledge_screen.editing = Some(i.id.clone());
                    self.mode = Mode::Input(InputPurpose::KnowledgeEdit, Input::with_text(&i.text));
                }
            }
            KeyCode::Char('p') => {
                let r = self.with_selected(|k, id| k.promote(id).map(|i| promoted(&i)));
                self.show_notice(r);
            }
            KeyCode::Char('c') => {
                let r = self.with_selected(|k, id| {
                    k.confirm(id)
                        .map(|i| format!("{} confirmed: agents see it again", i.id))
                });
                self.show_notice(r);
            }
            KeyCode::Char('x') => {
                let r =
                    self.with_selected(|k, id| k.retire(id).map(|i| format!("{} retired", i.id)));
                self.show_notice(r);
            }
            _ => return false,
        }
        true
    }

    fn with_selected(
        &self,
        f: impl FnOnce(&KnowledgeService, &ItemId) -> AppResult<String>,
    ) -> AppResult<String> {
        let (Some(k), Some(item)) = (&self.engine.knowledge, self.selected_knowledge()) else {
            return Err(AppError::Knowledge("nothing selected".into()));
        };
        f(k, &item.id)
    }

    fn show_notice(&mut self, result: AppResult<String>) {
        match result {
            Ok(text) => self.notice = Some(text),
            Err(e) => self.error = Some(e.to_string()),
        }
    }

    /// `kind: text` (or just text, a fact) typed on the Knowledge screen.
    pub(crate) fn submit_knowledge_add(&mut self, text: &str) -> AppResult<()> {
        let Some(k) = self.engine.knowledge.clone() else {
            return Ok(());
        };
        let (kind, text) = match text.split_once(':') {
            Some((kind, rest)) => match Kind::parse(kind.trim().to_ascii_lowercase().as_str()) {
                Some(kind) => (kind, rest.trim()),
                None => (Kind::Fact, text.trim()),
            },
            None => (Kind::Fact, text.trim()),
        };
        let item = k.add(Draft::new(self.knowledge_home(), kind, text))?;
        self.notice = Some(format!(
            "added {} ({}, {}); `swarmux knowledge edit {}` sets its scope and enforcer",
            item.id,
            item.kind.as_str(),
            item.status.as_str(),
            item.id
        ));
        Ok(())
    }

    /// The new text of the item being edited.
    pub(crate) fn submit_knowledge_edit(&mut self, text: &str) -> AppResult<()> {
        let (Some(k), Some(id)) = (
            self.engine.knowledge.clone(),
            self.knowledge_screen.editing.take(),
        ) else {
            return Ok(());
        };
        let Some(old) = k.get(&id) else {
            return Ok(());
        };
        let draft = Draft {
            home: old.home.clone(),
            kind: old.kind,
            text: text.to_string(),
            why: old.why.clone(),
            scope: old.scope.clone(),
            anchors: old.anchors.clone(),
            core: old.core,
            enforcer: old.enforcer.clone(),
            status: None,
            source: old.provenance.source,
        };
        let item = k.edit(&id, draft)?;
        self.notice = Some(format!("{} edited ({})", item.id, item.status.as_str()));
        Ok(())
    }
}

fn promoted(i: &Item) -> String {
    match i.status {
        Status::Shadow => format!("{} is in shadow: it logs what it would do", i.id),
        Status::Enforced => format!("{} is enforced", i.id),
        s => format!("{} is {}", i.id, s.as_str()),
    }
}

#[cfg(test)]
mod tests;
