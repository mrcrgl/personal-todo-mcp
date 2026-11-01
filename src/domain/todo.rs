//! The todo model and its invariants.

use std::cmp::Ordering;
use std::str::FromStr;

use chrono::{DateTime, NaiveDate, Utc};

/// GitHub's limit on label names, minus room for the `list:` prefix.
const MAX_LABEL_CHARS: usize = 44;
const MAX_TITLE_CHARS: usize = 256;
/// GitHub caps issue bodies at 65536 characters; the rest is headroom for the
/// due-date line.
pub const MAX_NOTES_CHARS: usize = 60_000;
const MAX_TAGS: usize = 10;
const RESERVED_PREFIXES: &[&str] = &["priority:", "list:"];

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ValidationError(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TodoId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TodoState {
    Open,
    Done,
    Dropped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    High,
    Medium,
    Low,
}

impl AsRef<str> for Priority {
    fn as_ref(&self) -> &str {
        match self {
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
        }
    }
}

impl FromStr for Priority {
    type Err = ValidationError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "high" => Ok(Self::High),
            "medium" => Ok(Self::Medium),
            "low" => Ok(Self::Low),
            other => Err(ValidationError(format!(
                "unknown priority {other:?}: use high, medium, or low"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Title(String);

impl Title {
    pub fn parse(value: &str) -> Result<Self, ValidationError> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(ValidationError("title must not be empty".into()));
        }
        if trimmed.chars().count() > MAX_TITLE_CHARS {
            return Err(ValidationError(format!(
                "title is longer than {MAX_TITLE_CHARS} characters"
            )));
        }
        Ok(Self(trimmed.to_string()))
    }

    /// For titles read back from storage, which were not necessarily written
    /// through this server.
    pub fn from_stored(value: String) -> Self {
        Self(value)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A list or tag name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelName(String);

impl LabelName {
    pub fn parse(value: &str) -> Result<Self, ValidationError> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(ValidationError(
                "list and tag names must not be empty".into(),
            ));
        }
        if trimmed.chars().count() > MAX_LABEL_CHARS {
            return Err(ValidationError(format!(
                "{trimmed:?} is longer than {MAX_LABEL_CHARS} characters"
            )));
        }
        if trimmed.contains(',') || trimmed.chars().any(char::is_control) {
            return Err(ValidationError(format!(
                "{trimmed:?} must not contain commas or control characters"
            )));
        }
        if RESERVED_PREFIXES.iter().any(|p| trimmed.starts_with(p)) {
            return Err(ValidationError(format!(
                "{trimmed:?} uses a reserved prefix: set priority and list through their own fields"
            )));
        }
        Ok(Self(trimmed.to_string()))
    }

    /// For labels read back from storage. Users can create any label in the
    /// GitHub UI, and rejecting one here would lose it on the next save.
    pub fn from_stored(value: String) -> Self {
        Self(value)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Validates a tag list, which must not outgrow what a todo can usefully carry.
pub fn parse_tags(values: &[String]) -> Result<Vec<LabelName>, ValidationError> {
    if values.len() > MAX_TAGS {
        return Err(ValidationError(format!(
            "a todo can have at most {MAX_TAGS} tags"
        )));
    }
    values.iter().map(|tag| LabelName::parse(tag)).collect()
}

pub fn validate_notes(notes: &str) -> Result<(), ValidationError> {
    if notes.chars().count() > MAX_NOTES_CHARS {
        return Err(ValidationError(format!(
            "notes are longer than {MAX_NOTES_CHARS} characters"
        )));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Todo {
    pub id: TodoId,
    pub title: Title,
    pub notes: String,
    pub state: TodoState,
    pub priority: Option<Priority>,
    pub list: Option<LabelName>,
    pub tags: Vec<LabelName>,
    pub due: Option<NaiveDate>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub url: String,
}

impl Todo {
    pub fn apply(&mut self, patch: TodoPatch) {
        if let Some(title) = patch.title {
            self.title = title;
        }
        if let Some(notes) = patch.notes {
            self.notes = notes;
        }
        if let Some(priority) = patch.priority {
            self.priority = priority;
        }
        if let Some(list) = patch.list {
            self.list = list;
        }
        if let Some(tags) = patch.tags {
            self.tags = tags;
        }
        if let Some(due) = patch.due {
            self.due = due;
        }
    }

    /// Agenda order: earliest due date first, undated last, then by priority.
    pub fn agenda_cmp(&self, other: &Self) -> Ordering {
        let due = match (self.due, other.due) {
            (Some(a), Some(b)) => a.cmp(&b),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        };
        let priority = match (self.priority, other.priority) {
            (Some(a), Some(b)) => a.cmp(&b),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        };
        due.then(priority).then(other.id.0.cmp(&self.id.0))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewTodo {
    pub title: Title,
    pub notes: String,
    pub priority: Option<Priority>,
    pub list: Option<LabelName>,
    pub tags: Vec<LabelName>,
    pub due: Option<NaiveDate>,
}

/// Outer `None` leaves a field alone; inner `None` clears it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TodoPatch {
    pub title: Option<Title>,
    pub notes: Option<String>,
    pub priority: Option<Option<Priority>>,
    pub list: Option<Option<LabelName>>,
    pub tags: Option<Vec<LabelName>>,
    pub due: Option<Option<NaiveDate>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateFilter {
    Open,
    Done,
    Dropped,
    All,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TodoFilter {
    pub state: StateFilter,
    pub priority: Option<Priority>,
    pub list: Option<LabelName>,
    pub tag: Option<LabelName>,
    pub due_on_or_before: Option<NaiveDate>,
    pub limit: usize,
}

impl TodoFilter {
    pub fn matches(&self, todo: &Todo) -> bool {
        let state = match self.state {
            StateFilter::Open => todo.state == TodoState::Open,
            StateFilter::Done => todo.state == TodoState::Done,
            StateFilter::Dropped => todo.state == TodoState::Dropped,
            StateFilter::All => true,
        };
        state
            && self.priority.is_none_or(|p| todo.priority == Some(p))
            && self
                .list
                .as_ref()
                .is_none_or(|l| todo.list.as_ref() == Some(l))
            && self.tag.as_ref().is_none_or(|t| todo.tags.contains(t))
            && self
                .due_on_or_before
                .is_none_or(|limit| todo.due.is_some_and(|due| due <= limit))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TodoPage {
    pub todos: Vec<Todo>,
    /// More matching todos may exist beyond this page.
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TodoComment {
    pub author: String,
    pub created_at: DateTime<Utc>,
    pub body: String,
}

/// The list and tag names in use in a repository.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TodoLabels {
    pub lists: Vec<LabelName>,
    pub tags: Vec<LabelName>,
}

/// A list or tag to remove. Priorities are fixed vocabulary and stay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LabelRef {
    List(LabelName),
    Tag(LabelName),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TodoRepository {
    pub full_name: String,
    pub url: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn todo(id: u64) -> Todo {
        Todo {
            id: TodoId(id),
            title: Title::parse("Water the plants").unwrap(),
            notes: String::new(),
            state: TodoState::Open,
            priority: None,
            list: None,
            tags: Vec::new(),
            due: None,
            created_at: DateTime::UNIX_EPOCH,
            updated_at: DateTime::UNIX_EPOCH,
            url: String::new(),
        }
    }

    fn filter() -> TodoFilter {
        TodoFilter {
            state: StateFilter::Open,
            priority: None,
            list: None,
            tag: None,
            due_on_or_before: None,
            limit: 50,
        }
    }

    fn date(value: &str) -> NaiveDate {
        value.parse().unwrap()
    }

    #[test]
    fn title_is_trimmed_and_must_not_be_blank() {
        assert_eq!(Title::parse("  Call Sam ").unwrap().as_str(), "Call Sam");
        assert!(Title::parse("   ").is_err());
        assert!(Title::parse(&"x".repeat(257)).is_err());
    }

    #[test]
    fn label_names_reject_reserved_prefixes_and_commas() {
        assert!(LabelName::parse("home").is_ok());
        assert!(LabelName::parse("priority:high").is_err());
        assert!(LabelName::parse("list:home").is_err());
        assert!(LabelName::parse("a,b").is_err());
        assert!(LabelName::parse(&"x".repeat(45)).is_err());
    }

    #[test]
    fn tags_are_capped() {
        let tags: Vec<String> = (0..11).map(|i| format!("t{i}")).collect();
        assert!(parse_tags(&tags).is_err());
        assert_eq!(parse_tags(&tags[..10]).unwrap().len(), 10);
    }

    #[test]
    fn priority_round_trips_through_its_string_form() {
        for priority in [Priority::High, Priority::Medium, Priority::Low] {
            assert_eq!(priority.as_ref().parse::<Priority>().unwrap(), priority);
        }
        assert!("urgent".parse::<Priority>().is_err());
    }

    #[test]
    fn patch_distinguishes_untouched_from_cleared() {
        let mut item = todo(1);
        item.priority = Some(Priority::High);
        item.due = Some(date("2026-11-01"));

        item.apply(TodoPatch {
            due: Some(None),
            ..TodoPatch::default()
        });

        assert_eq!(item.priority, Some(Priority::High));
        assert_eq!(item.due, None);
    }

    #[test]
    fn filter_applies_state_and_due_date() {
        let mut item = todo(1);
        item.due = Some(date("2026-11-01"));

        assert!(
            TodoFilter {
                due_on_or_before: Some(date("2026-11-01")),
                ..filter()
            }
            .matches(&item)
        );
        assert!(
            !TodoFilter {
                due_on_or_before: Some(date("2026-10-31")),
                ..filter()
            }
            .matches(&item)
        );
        assert!(
            !TodoFilter {
                state: StateFilter::Done,
                ..filter()
            }
            .matches(&item)
        );
    }

    #[test]
    fn undated_todos_do_not_match_a_due_filter() {
        let item = todo(1);
        assert!(
            !TodoFilter {
                due_on_or_before: Some(date("2030-01-01")),
                ..filter()
            }
            .matches(&item)
        );
    }

    #[test]
    fn agenda_puts_due_dates_first_then_priority() {
        let mut soon = todo(1);
        soon.due = Some(date("2026-11-01"));
        let mut later = todo(2);
        later.due = Some(date("2026-12-01"));
        let mut urgent = todo(3);
        urgent.priority = Some(Priority::High);
        let plain = todo(4);

        let mut items = [plain.clone(), urgent.clone(), later.clone(), soon.clone()];
        items.sort_by(Todo::agenda_cmp);

        let ids: Vec<u64> = items.iter().map(|t| t.id.0).collect();
        assert_eq!(ids, [1, 2, 3, 4]);
    }
}
