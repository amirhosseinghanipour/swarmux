//! The key list is the keymap and the catalog, written out.

use super::*;
use crate::app::tests_support::test_app;
use crate::screens::Screen;

#[test]
fn it_lists_the_screens_keys_then_the_answers_then_the_global_ones() {
    let mut t = test_app();
    t.app.screen = Screen::Queue;
    let found = lines(&t.app);
    let headings: Vec<&str> = found
        .iter()
        .filter_map(|l| match l {
            HelpLine::Heading { title, .. } => Some(title.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        headings,
        ["queue", "answers (when something asks you)", "everywhere"]
    );
    assert!(!matches!(found.last(), Some(HelpLine::Gap)));
}

#[test]
fn every_action_that_works_on_the_screen_has_a_row() {
    for screen in crate::screens::all() {
        let mut t = test_app();
        t.app.screen = screen;
        let found = lines(&t.app);
        let mut stack = screen.def().contexts(&t.app);
        stack.push(crate::keys::DECIDE);
        stack.push(swarmux_domain::keys::GLOBAL);
        let want: usize = stack.iter().map(|c| t.app.catalog().of(*c).count()).sum();
        let rows = found
            .iter()
            .filter(|l| matches!(l, HelpLine::Row { .. }))
            .count();
        assert_eq!(rows, want, "{}", screen.title());
    }
}

#[test]
fn quit_always_lists_ctrl_c_and_back_lists_esc() {
    let t = test_app();
    let found = lines(&t.app);
    let keys_of = |id: &str| {
        found.iter().find_map(|l| match l {
            HelpLine::Row { id: i, keys, .. } if i == id => Some(keys.clone()),
            _ => None,
        })
    };
    assert_eq!(keys_of("quit").as_deref(), Some("q, ctrl+c"));
    assert_eq!(keys_of("back").as_deref(), Some("esc"));
    assert_eq!(keys_of("decide.yes").as_deref(), Some("y"));
}

#[test]
fn the_fixed_rows_are_the_answers_and_back() {
    let t = test_app();
    let fixed: Vec<String> = lines(&t.app)
        .into_iter()
        .filter_map(|l| match l {
            HelpLine::Row {
                id, fixed: true, ..
            } => Some(id),
            _ => None,
        })
        .collect();
    let mut fixed_sorted = fixed.clone();
    fixed_sorted.sort();
    assert_eq!(
        fixed_sorted,
        [
            "back",
            "decide.always",
            "decide.cancel_plan",
            "decide.no",
            "decide.yes"
        ]
    );
}
