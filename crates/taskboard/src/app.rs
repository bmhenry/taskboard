//! GUI state and the actions that change it. Kept free of rendering code so
//! it can be tested without a window.

use std::path::PathBuf;

use taskboard_core::{
    Board, DateField, Filter, Link, LinkKind, NewTask, Status, Store, Task, TaskPatch, Ulid,
    parse_date,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Board,
    List,
}

/// The task editor's target: an existing task, or a new one in `status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditTarget {
    Existing(Ulid),
    New(Status),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Drag {
    pub id: Ulid,
    pub start: (f32, f32),
    /// Becomes true once the pointer has moved far enough to count as a drag
    /// rather than a click.
    pub active: bool,
}

/// Distance in pixels the pointer must travel before a press becomes a drag.
pub const DRAG_THRESHOLD: f32 = 6.0;

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Select(Option<Ulid>),
    SetView(View),
    /// Open the editor; the UI fills its inputs on the next frame.
    Edit(EditTarget),
    /// The UI has copied the edit target into its inputs.
    EditorReady,
    CancelEdit,
    Save {
        title: String,
        description: String,
    },
    Move {
        id: Ulid,
        status: Status,
        position: Option<usize>,
    },
    Delete(Ulid),
    AddLink {
        id: Ulid,
        url: String,
        kind: LinkKind,
    },
    RemoveLink {
        id: Ulid,
        index: usize,
    },
    Attach {
        id: Ulid,
        path: String,
    },
    Detach {
        id: Ulid,
        file: Ulid,
    },
    ViewFile(Option<(Ulid, Ulid)>),
    SetSearch(String),
    ToggleStatusFilter(Status),
    SetDateField(DateField),
    SetDateBounds {
        after: String,
        before: String,
    },
    ClearFilters,
    OpenUrl(String),
    DismissError,
}

pub struct App {
    pub store: Store,
    pub board: Board,
    pub view: View,
    pub filter: Filter,
    pub selected: Option<Ulid>,
    pub editing: Option<EditTarget>,
    /// Set when the editor opens so the UI copies the task into its inputs once.
    pub editor_needs_init: bool,
    /// (task, file) shown in the file viewer.
    pub viewing_file: Option<(Ulid, Ulid)>,
    /// The raw date bound inputs, kept so invalid text can be shown and fixed.
    pub date_inputs: (String, String),
    pub error: Option<String>,
}

impl App {
    pub fn new(store: Store) -> App {
        let mut app = App {
            board: Board::default(),
            store,
            view: View::Board,
            filter: Filter::default(),
            selected: None,
            editing: None,
            editor_needs_init: false,
            viewing_file: None,
            date_inputs: (String::new(), String::new()),
            error: None,
        };
        app.reload();
        app
    }

    pub fn reload(&mut self) {
        match self.store.load() {
            Ok(board) => {
                self.board = board;
                // Drop references to tasks deleted elsewhere (e.g. by the CLI).
                if self.selected.is_some_and(|id| self.board.get(id).is_none()) {
                    self.selected = None;
                }
                if let Some(EditTarget::Existing(id)) = self.editing
                    && self.board.get(id).is_none()
                {
                    self.editing = None;
                }
                if self
                    .viewing_file
                    .is_some_and(|(t, _)| self.board.get(t).is_none())
                {
                    self.viewing_file = None;
                }
            }
            Err(e) => self.error = Some(format!("Couldn't read tasks: {e}")),
        }
    }

    pub fn selected_task(&self) -> Option<&Task> {
        self.selected.and_then(|id| self.board.get(id))
    }

    /// Tasks in a status column that pass the current filter, in board order.
    pub fn visible_column(&self, status: Status) -> Vec<&Task> {
        self.board
            .column(status)
            .into_iter()
            .filter(|t| self.filter.matches(t))
            .collect()
    }

    /// Every task passing the filter, ordered by status then board order.
    pub fn visible_list(&self) -> Vec<&Task> {
        Status::ALL
            .into_iter()
            .flat_map(|s| self.visible_column(s))
            .collect()
    }

    pub fn apply(&mut self, action: Action) {
        if let Err(e) = self.try_apply(action) {
            self.error = Some(e.to_string());
        }
    }

    fn try_apply(&mut self, action: Action) -> taskboard_core::Result<()> {
        let writes = matches!(
            action,
            Action::Save { .. }
                | Action::Move { .. }
                | Action::Delete(_)
                | Action::AddLink { .. }
                | Action::RemoveLink { .. }
                | Action::Attach { .. }
                | Action::Detach { .. }
        );
        match action {
            Action::Select(id) => {
                self.selected = id;
                if id.is_none() {
                    self.editing = None;
                }
            }
            Action::SetView(view) => self.view = view,
            Action::Edit(target) => {
                self.editing = Some(target);
                self.editor_needs_init = true;
                if let EditTarget::Existing(id) = target {
                    self.selected = Some(id);
                }
            }
            Action::EditorReady => self.editor_needs_init = false,
            Action::CancelEdit => self.editing = None,
            Action::Save { title, description } => {
                let Some(target) = self.editing else {
                    return Ok(());
                };
                let id = match target {
                    EditTarget::Existing(id) => {
                        let patch = TaskPatch {
                            title: Some(title),
                            description: Some(description),
                            status: None,
                        };
                        self.store.update(|b| b.edit(id, patch))?;
                        id
                    }
                    EditTarget::New(status) => {
                        let new = NewTask {
                            title,
                            description,
                            status: Some(status),
                            links: Vec::new(),
                        };
                        self.store.update(|b| b.create(new))?
                    }
                };
                self.editing = None;
                self.selected = Some(id);
            }
            Action::Move {
                id,
                status,
                position,
            } => {
                self.store.update(|b| b.move_to(id, status, position))?;
            }
            Action::Delete(id) => {
                self.store.delete(id)?;
                if self.selected == Some(id) {
                    self.selected = None;
                    self.editing = None;
                }
            }
            Action::AddLink { id, url, kind } => {
                let url = url.trim().to_string();
                if !url.is_empty() {
                    self.store.update(|b| {
                        b.add_link(
                            id,
                            Link {
                                url,
                                label: None,
                                kind,
                            },
                        )
                    })?;
                }
            }
            Action::RemoveLink { id, index } => {
                self.store.update(|b| b.remove_link(id, index))?;
            }
            Action::Attach { id, path } => {
                let path = PathBuf::from(expand_home(path.trim()));
                self.store.attach(id, &path)?;
            }
            Action::Detach { id, file } => {
                self.store.detach(id, file)?;
                if self.viewing_file == Some((id, file)) {
                    self.viewing_file = None;
                }
            }
            Action::ViewFile(f) => self.viewing_file = f,
            Action::SetSearch(text) => self.filter.text = text,
            Action::ToggleStatusFilter(status) => {
                if let Some(i) = self.filter.statuses.iter().position(|s| *s == status) {
                    self.filter.statuses.remove(i);
                } else {
                    self.filter.statuses.push(status);
                }
            }
            Action::SetDateField(field) => self.filter.date_field = field,
            Action::SetDateBounds { after, before } => {
                // Unparseable bounds are ignored (and flagged in the UI) rather
                // than hiding every task while the user is still typing.
                self.filter.after = parse_date(&after).ok();
                self.filter.before = parse_date(&before).ok();
                self.date_inputs = (after, before);
            }
            Action::ClearFilters => {
                self.filter = Filter::default();
                self.date_inputs = (String::new(), String::new());
            }
            Action::OpenUrl(url) => open_url(&url)?,
            Action::DismissError => self.error = None,
        }
        // Show our own write right away rather than waiting for the watcher.
        if writes {
            self.reload();
        }
        Ok(())
    }
}

/// Converts a drop at visible index `visible_index` (among the column's
/// *visible* tasks, excluding the dragged one) into a position in the full
/// column, so dropping into a filtered column lands next to the right task.
pub fn drop_position(
    board: &Board,
    status: Status,
    visible: &[Ulid],
    visible_index: usize,
    dragged: Ulid,
) -> usize {
    let full: Vec<Ulid> = board
        .column(status)
        .iter()
        .map(|t| t.id)
        .filter(|id| *id != dragged)
        .collect();
    let index_of = |id: &Ulid| full.iter().position(|f| f == id).unwrap_or(full.len());
    match visible.get(visible_index) {
        Some(next) => index_of(next),
        None => visible.last().map_or(full.len(), |last| index_of(last) + 1),
    }
}

/// Index at which a drop at height `y` lands, given the vertical midpoints of
/// the cards it could land between.
pub fn drop_index(card_midpoints: &[f32], y: f32) -> usize {
    card_midpoints.iter().filter(|mid| **mid < y).count()
}

fn expand_home(path: &str) -> String {
    match (path.strip_prefix("~/"), std::env::var("HOME")) {
        (Some(rest), Ok(home)) => format!("{home}/{rest}"),
        _ => path.to_string(),
    }
}

fn open_url(url: &str) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    let mut cmd = std::process::Command::new("open");
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", ""]);
        c
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut cmd = std::process::Command::new("xdg-open");
    cmd.arg(url).spawn().map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> (tempfile::TempDir, App) {
        let dir = tempfile::tempdir().unwrap();
        let app = App::new(Store::open(dir.path()).unwrap());
        (dir, app)
    }

    fn add(app: &mut App, title: &str) -> Ulid {
        app.apply(Action::Edit(EditTarget::New(Status::Backlog)));
        app.apply(Action::Save {
            title: title.into(),
            description: String::new(),
        });
        app.selected.unwrap()
    }

    #[test]
    fn create_and_drag_within_filtered_column() {
        let (_dir, mut app) = app();
        let a = add(&mut app, "alpha");
        let _b = add(&mut app, "beta");
        let c = add(&mut app, "gamma alpha");
        assert_eq!(app.error, None);

        // With "alpha" searched, only a and c are visible. Dragging c to the
        // top of the visible list must put it above a in the real column.
        app.apply(Action::SetSearch("alpha".into()));
        let visible: Vec<Ulid> = app
            .visible_column(Status::Backlog)
            .iter()
            .map(|t| t.id)
            .filter(|id| *id != c)
            .collect();
        assert_eq!(visible, [a]);
        let pos = drop_position(&app.board, Status::Backlog, &visible, 0, c);
        app.apply(Action::Move {
            id: c,
            status: Status::Backlog,
            position: Some(pos),
        });

        let order: Vec<&str> = app
            .board
            .column(Status::Backlog)
            .iter()
            .map(|t| t.title.as_str())
            .collect();
        assert_eq!(order, ["gamma alpha", "alpha", "beta"]);
    }

    #[test]
    fn selection_cleared_when_task_deleted_elsewhere() {
        let (_dir, mut app) = app();
        let a = add(&mut app, "alpha");
        app.store.delete(a).unwrap(); // as if `tb rm` ran
        app.reload();
        assert_eq!(app.selected, None);
    }

    #[test]
    fn drop_index_counts_cards_above() {
        assert_eq!(drop_index(&[10.0, 50.0, 90.0], 60.0), 2);
        assert_eq!(drop_index(&[], 60.0), 0);
    }
}
