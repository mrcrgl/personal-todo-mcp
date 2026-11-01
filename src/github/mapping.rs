//! Explicit mapping between GitHub issues and todos.
//!
//! Priority and list are `priority:*` and `list:*` labels, and any other label
//! is a tag. The due date is a plain `Due: YYYY-MM-DD` last line of the issue
//! body, so it stays visible and editable in the GitHub UI and does not leave
//! one label per date behind.

use chrono::NaiveDate;

use crate::domain::todo::{
    LabelName, LabelRef, Priority, Title, Todo, TodoComment, TodoId, TodoLabels, TodoState,
};
use crate::github::models::{CommentRow, IssueRow, LabelRow};

const PRIORITY_PREFIX: &str = "priority:";
const LIST_PREFIX: &str = "list:";
const DUE_PREFIX: &str = "Due: ";
const GHOST_AUTHOR: &str = "ghost";

pub fn todo_from_issue(row: IssueRow) -> Todo {
    let (notes, due) = parse_body(row.body.as_deref().unwrap_or_default());
    let (priority, list, tags) = parse_labels(&row.labels);
    Todo {
        id: TodoId(row.number),
        title: Title::from_stored(row.title),
        notes,
        state: state_from_issue(&row.state, row.state_reason.as_deref()),
        priority,
        list,
        tags,
        due,
        created_at: row.created_at,
        updated_at: row.updated_at,
        url: row.html_url,
    }
}

pub fn comment_from_row(row: CommentRow) -> TodoComment {
    TodoComment {
        author: row
            .user
            .map_or_else(|| GHOST_AUTHOR.to_string(), |user| user.login),
        created_at: row.created_at,
        body: row.body.unwrap_or_default(),
    }
}

fn state_from_issue(state: &str, reason: Option<&str>) -> TodoState {
    match (state, reason) {
        ("open", _) => TodoState::Open,
        (_, Some("not_planned" | "duplicate")) => TodoState::Dropped,
        _ => TodoState::Done,
    }
}

/// GitHub's `state` and `state_reason` values for a todo state.
pub fn issue_state(state: TodoState) -> (&'static str, Option<&'static str>) {
    match state {
        TodoState::Open => ("open", None),
        TodoState::Done => ("closed", Some("completed")),
        TodoState::Dropped => ("closed", Some("not_planned")),
    }
}

/// Only the first `priority:` and `list:` label take effect. Surplus ones,
/// which can only come from manual edits, stay as tags so a save keeps them.
fn parse_labels(labels: &[LabelRow]) -> (Option<Priority>, Option<LabelName>, Vec<LabelName>) {
    let mut priority = None;
    let mut list = None;
    let mut tags = Vec::new();
    for label in labels {
        let name = label.name.as_str();
        if priority.is_none()
            && let Some(parsed) = name
                .strip_prefix(PRIORITY_PREFIX)
                .and_then(|value| value.parse().ok())
        {
            priority = Some(parsed);
        } else if list.is_none()
            && let Some(value) = name.strip_prefix(LIST_PREFIX)
        {
            list = Some(LabelName::from_stored(value.to_string()));
        } else {
            tags.push(LabelName::from_stored(label.name.clone()));
        }
    }
    (priority, list, tags)
}

pub fn priority_label(priority: Priority) -> String {
    format!("{PRIORITY_PREFIX}{}", priority.as_ref())
}

pub fn list_label(list: &LabelName) -> String {
    format!("{LIST_PREFIX}{}", list.as_str())
}

pub fn label_name(label: &LabelRef) -> String {
    match label {
        LabelRef::List(list) => list_label(list),
        LabelRef::Tag(tag) => tag.as_str().to_string(),
    }
}

pub fn issue_labels(
    priority: Option<Priority>,
    list: Option<&LabelName>,
    tags: &[LabelName],
) -> Vec<String> {
    priority
        .map(priority_label)
        .into_iter()
        .chain(list.map(list_label))
        .chain(tags.iter().map(|tag| tag.as_str().to_string()))
        .collect()
}

pub fn labels_summary(labels: Vec<LabelRow>) -> TodoLabels {
    let mut summary = TodoLabels::default();
    for label in labels {
        if let Some(list) = label.name.strip_prefix(LIST_PREFIX) {
            summary.lists.push(LabelName::from_stored(list.to_string()));
        } else if !label.name.starts_with(PRIORITY_PREFIX) {
            summary.tags.push(LabelName::from_stored(label.name));
        }
    }
    summary
}

pub fn compose_body(notes: &str, due: Option<NaiveDate>) -> String {
    match (notes.is_empty(), due) {
        (_, None) => notes.to_string(),
        (true, Some(due)) => format!("{DUE_PREFIX}{due}"),
        (false, Some(due)) => format!("{}\n\n{DUE_PREFIX}{due}", notes.trim_end()),
    }
}

fn parse_body(body: &str) -> (String, Option<NaiveDate>) {
    let trimmed = body.trim_end();
    let (head, last) = trimmed.rsplit_once('\n').unwrap_or(("", trimmed));
    let due = last
        .trim_end()
        .strip_prefix(DUE_PREFIX)
        .and_then(|value| NaiveDate::parse_from_str(value, "%Y-%m-%d").ok());
    match due {
        Some(due) => (head.trim_end().to_string(), Some(due)),
        None => (body.to_string(), None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::DateTime;

    fn label(name: &str) -> LabelRow {
        LabelRow { name: name.into() }
    }

    fn issue(state: &str, reason: Option<&str>) -> IssueRow {
        IssueRow {
            number: 7,
            title: "Renew passport".into(),
            body: Some("Bring photos\n\nDue: 2026-11-03".into()),
            state: state.into(),
            state_reason: reason.map(String::from),
            labels: vec![label("priority:high"), label("list:admin"), label("errand")],
            created_at: DateTime::UNIX_EPOCH,
            updated_at: DateTime::UNIX_EPOCH,
            html_url: "https://github.test/issues/7".into(),
            pull_request: None,
        }
    }

    #[test]
    fn issue_maps_to_todo() {
        let todo = todo_from_issue(issue("open", None));
        assert_eq!(todo.id, TodoId(7));
        assert_eq!(todo.notes, "Bring photos");
        assert_eq!(todo.due, "2026-11-03".parse().ok());
        assert_eq!(todo.priority, Some(Priority::High));
        assert_eq!(todo.list.unwrap().as_str(), "admin");
        assert_eq!(todo.tags, [LabelName::from_stored("errand".into())]);
    }

    #[test]
    fn close_reason_decides_done_or_dropped() {
        assert_eq!(todo_from_issue(issue("open", None)).state, TodoState::Open);
        for (reason, state) in [
            (Some("completed"), TodoState::Done),
            (None, TodoState::Done),
            (Some("not_planned"), TodoState::Dropped),
            (Some("duplicate"), TodoState::Dropped),
        ] {
            assert_eq!(todo_from_issue(issue("closed", reason)).state, state);
        }
    }

    #[test]
    fn todo_states_map_back_to_github_values() {
        assert_eq!(issue_state(TodoState::Open), ("open", None));
        assert_eq!(issue_state(TodoState::Done), ("closed", Some("completed")));
        assert_eq!(
            issue_state(TodoState::Dropped),
            ("closed", Some("not_planned"))
        );
    }

    #[test]
    fn body_round_trips_with_and_without_due_date() {
        let due = "2026-11-03".parse().ok();
        for (notes, due) in [
            ("Bring photos", due),
            ("", due),
            ("Line one\n\nLine two", due),
            ("Bring photos", None),
            ("", None),
        ] {
            assert_eq!(
                parse_body(&compose_body(notes, due)),
                (notes.to_string(), due)
            );
        }
    }

    #[test]
    fn a_due_line_that_is_not_last_or_not_a_date_stays_in_the_notes() {
        for body in ["Due: soon", "Due: 2026-11-03\nmore text", "Due: 2026-13-40"] {
            assert_eq!(parse_body(body), (body.to_string(), None));
        }
    }

    #[test]
    fn surplus_structured_labels_are_preserved_as_tags() {
        let (priority, list, tags) = parse_labels(&[
            label("priority:high"),
            label("priority:low"),
            label("priority:urgent"),
            label("list:a"),
            label("list:b"),
        ]);
        assert_eq!(priority, Some(Priority::High));
        assert_eq!(list.unwrap().as_str(), "a");
        let tags: Vec<&str> = tags.iter().map(LabelName::as_str).collect();
        assert_eq!(tags, ["priority:low", "priority:urgent", "list:b"]);
    }

    #[test]
    fn labels_round_trip_through_issue_labels() {
        let list = LabelName::parse("home").unwrap();
        let tags = [LabelName::parse("errand").unwrap()];
        let names = issue_labels(Some(Priority::Low), Some(&list), &tags);
        assert_eq!(names, ["priority:low", "list:home", "errand"]);
    }

    #[test]
    fn summary_separates_lists_from_tags_and_hides_priorities() {
        let summary = labels_summary(vec![
            label("priority:high"),
            label("list:home"),
            label("errand"),
        ]);
        assert_eq!(summary.lists, [LabelName::from_stored("home".into())]);
        assert_eq!(summary.tags, [LabelName::from_stored("errand".into())]);
    }
}
