//! Tool arguments and results, with explicit mapping to and from the domain.

use chrono::{DateTime, NaiveDate, Utc};
use rmcp::schemars;
use serde::{Deserialize, Serialize};

use crate::domain::todo::{
    LabelName, LabelRef, NewTodo, Priority, StateFilter, Title, Todo, TodoComment, TodoFilter,
    TodoLabels, TodoPage, TodoPatch, TodoState, ValidationError, parse_tags, validate_notes,
};

const DEFAULT_LIMIT: u32 = 50;
const MAX_LIMIT: u32 = 200;

#[derive(Debug, Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum PriorityArg {
    High,
    Medium,
    Low,
}

impl From<PriorityArg> for Priority {
    fn from(value: PriorityArg) -> Self {
        match value {
            PriorityArg::High => Self::High,
            PriorityArg::Medium => Self::Medium,
            PriorityArg::Low => Self::Low,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum StateArg {
    #[default]
    Open,
    Done,
    Dropped,
    All,
}

impl From<StateArg> for StateFilter {
    fn from(value: StateArg) -> Self {
        match value {
            StateArg::Open => Self::Open,
            StateArg::Done => Self::Done,
            StateArg::Dropped => Self::Dropped,
            StateArg::All => Self::All,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ClearField {
    Priority,
    List,
    Tags,
    Due,
}

#[derive(Debug, Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum LabelKind {
    List,
    Tag,
}

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct ListArgs {
    /// Which todos to show. Defaults to "open".
    pub state: Option<StateArg>,
    pub priority: Option<PriorityArg>,
    /// Only todos on this list.
    pub list: Option<String>,
    /// Only todos with this tag.
    pub tag: Option<String>,
    /// Only todos due on or before this date (YYYY-MM-DD).
    pub due_on_or_before: Option<String>,
    /// Maximum results. Defaults to 50, at most 200.
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct IdArgs {
    /// Todo id.
    pub id: u64,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CreateArgs {
    pub title: String,
    /// Longer description, as Markdown.
    pub notes: Option<String>,
    pub priority: Option<PriorityArg>,
    /// The list this todo belongs to, e.g. "home". Created when new.
    pub list: Option<String>,
    /// Free-form tags. Created when new.
    pub tags: Option<Vec<String>>,
    /// Due date (YYYY-MM-DD).
    pub due: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct UpdateArgs {
    /// Todo id.
    pub id: u64,
    pub title: Option<String>,
    /// Replaces the notes.
    pub notes: Option<String>,
    pub priority: Option<PriorityArg>,
    pub list: Option<String>,
    /// Replaces all tags.
    pub tags: Option<Vec<String>>,
    /// Due date (YYYY-MM-DD).
    pub due: Option<String>,
    /// Fields to empty, e.g. ["due"] to remove the due date.
    pub clear: Option<Vec<ClearField>>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CommentArgs {
    /// Todo id.
    pub id: u64,
    /// Markdown comment.
    pub body: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct LabelDeleteArgs {
    pub kind: LabelKind,
    /// The list or tag name.
    pub name: String,
}

fn parse_date(value: &str) -> Result<NaiveDate, ValidationError> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .map_err(|_| ValidationError(format!("{value:?} is not a YYYY-MM-DD date")))
}

fn parse_label(value: Option<&str>) -> Result<Option<LabelName>, ValidationError> {
    value.map(LabelName::parse).transpose()
}

fn parse_due(value: Option<&str>) -> Result<Option<NaiveDate>, ValidationError> {
    value.map(parse_date).transpose()
}

impl ListArgs {
    pub fn into_filter(self) -> Result<TodoFilter, ValidationError> {
        Ok(TodoFilter {
            state: self.state.unwrap_or_default().into(),
            priority: self.priority.map(Into::into),
            list: parse_label(self.list.as_deref())?,
            tag: parse_label(self.tag.as_deref())?,
            due_on_or_before: parse_due(self.due_on_or_before.as_deref())?,
            limit: self.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT) as usize,
        })
    }
}

impl CreateArgs {
    pub fn into_new_todo(self) -> Result<NewTodo, ValidationError> {
        let notes = self.notes.unwrap_or_default();
        validate_notes(&notes)?;
        Ok(NewTodo {
            title: Title::parse(&self.title)?,
            notes,
            priority: self.priority.map(Into::into),
            list: parse_label(self.list.as_deref())?,
            tags: parse_tags(&self.tags.unwrap_or_default())?,
            due: parse_due(self.due.as_deref())?,
        })
    }
}

impl UpdateArgs {
    /// Explicit `clear` wins over a value given for the same field, because
    /// the two together can only be a mistake and clearing loses less.
    pub fn into_patch(self) -> Result<TodoPatch, ValidationError> {
        if let Some(notes) = &self.notes {
            validate_notes(notes)?;
        }
        let mut patch = TodoPatch {
            title: self.title.as_deref().map(Title::parse).transpose()?,
            notes: self.notes,
            priority: self.priority.map(|priority| Some(priority.into())),
            list: parse_label(self.list.as_deref())?.map(Some),
            tags: self.tags.as_deref().map(parse_tags).transpose()?,
            due: parse_due(self.due.as_deref())?.map(Some),
        };
        for field in self.clear.unwrap_or_default() {
            match field {
                ClearField::Priority => patch.priority = Some(None),
                ClearField::List => patch.list = Some(None),
                ClearField::Tags => patch.tags = Some(Vec::new()),
                ClearField::Due => patch.due = Some(None),
            }
        }
        Ok(patch)
    }
}

impl LabelDeleteArgs {
    pub fn into_label(self) -> Result<LabelRef, ValidationError> {
        let name = LabelName::parse(&self.name)?;
        Ok(match self.kind {
            LabelKind::List => LabelRef::List(name),
            LabelKind::Tag => LabelRef::Tag(name),
        })
    }
}

#[derive(Debug, Serialize)]
pub struct TodoDto {
    pub id: u64,
    pub title: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub notes: String,
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub list: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due: Option<NaiveDate>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub url: String,
}

impl From<Todo> for TodoDto {
    fn from(todo: Todo) -> Self {
        Self {
            id: todo.id.0,
            title: todo.title.as_str().to_string(),
            notes: todo.notes,
            state: match todo.state {
                TodoState::Open => "open",
                TodoState::Done => "done",
                TodoState::Dropped => "dropped",
            },
            priority: todo.priority.map(|p| p.as_ref().to_string()),
            list: todo.list.map(|l| l.as_str().to_string()),
            tags: todo.tags.iter().map(|t| t.as_str().to_string()).collect(),
            due: todo.due,
            created_at: todo.created_at,
            updated_at: todo.updated_at,
            url: todo.url,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct TodoPageDto {
    pub todos: Vec<TodoDto>,
    pub truncated: bool,
}

impl From<TodoPage> for TodoPageDto {
    fn from(page: TodoPage) -> Self {
        Self {
            todos: page.todos.into_iter().map(Into::into).collect(),
            truncated: page.truncated,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct CommentDto {
    pub author: String,
    pub created_at: DateTime<Utc>,
    pub body: String,
}

impl From<TodoComment> for CommentDto {
    fn from(comment: TodoComment) -> Self {
        Self {
            author: comment.author,
            created_at: comment.created_at,
            body: comment.body,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct TodoDetailDto {
    #[serde(flatten)]
    pub todo: TodoDto,
    pub comments: Vec<CommentDto>,
}

#[derive(Debug, Serialize)]
pub struct LabelsDto {
    pub lists: Vec<String>,
    pub tags: Vec<String>,
}

impl From<TodoLabels> for LabelsDto {
    fn from(labels: TodoLabels) -> Self {
        let names = |items: Vec<LabelName>| items.iter().map(|n| n.as_str().to_string()).collect();
        Self {
            lists: names(labels.lists),
            tags: names(labels.tags),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn update() -> UpdateArgs {
        UpdateArgs {
            id: 1,
            title: None,
            notes: None,
            priority: None,
            list: None,
            tags: None,
            due: None,
            clear: None,
        }
    }

    #[test]
    fn an_empty_update_touches_nothing() {
        assert_eq!(update().into_patch().unwrap(), TodoPatch::default());
    }

    #[test]
    fn clear_empties_fields_and_wins_over_a_value() {
        let patch = UpdateArgs {
            due: Some("2026-11-03".into()),
            clear: Some(vec![ClearField::Due, ClearField::Tags]),
            ..update()
        }
        .into_patch()
        .unwrap();
        assert_eq!(patch.due, Some(None));
        assert_eq!(patch.tags, Some(Vec::new()));
        assert_eq!(patch.priority, None);
    }

    #[test]
    fn bad_dates_name_the_expected_format() {
        let err = ListArgs {
            due_on_or_before: Some("tomorrow".into()),
            ..ListArgs::default()
        }
        .into_filter()
        .unwrap_err();
        assert!(err.0.contains("YYYY-MM-DD"));
    }

    #[test]
    fn list_defaults_to_open_and_clamps_the_limit() {
        let filter = ListArgs::default().into_filter().unwrap();
        assert_eq!(filter.state, StateFilter::Open);
        assert_eq!(filter.limit, 50);

        let filter = ListArgs {
            limit: Some(100_000),
            ..ListArgs::default()
        }
        .into_filter()
        .unwrap();
        assert_eq!(filter.limit, 200);
    }

    #[test]
    fn create_rejects_a_blank_title() {
        let args = CreateArgs {
            title: "  ".into(),
            notes: None,
            priority: None,
            list: None,
            tags: None,
            due: None,
        };
        assert!(args.into_new_todo().is_err());
    }

    #[test]
    fn deleting_a_reserved_label_is_refused() {
        let args = LabelDeleteArgs {
            kind: LabelKind::Tag,
            name: "priority:high".into(),
        };
        assert!(args.into_label().is_err());
    }
}
