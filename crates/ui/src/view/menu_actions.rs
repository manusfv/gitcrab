//! The actions a repository view's context menus dispatch.
use super::*;
use components::menu::register;
use components::MenuTarget;
use gitcrab_core::{git::FileChange, history_columns::HistoryColumn};

/// A context menu item that acts on a repository view.
#[derive(Clone, PartialEq, Debug, gpui::Action)]
#[action(namespace = gitcrab, no_json)]
pub(super) struct RepositoryMenu(MenuTarget<GitCrab>, MenuCommand);

#[derive(Clone, PartialEq, Debug)]
pub(super) enum MenuCommand {
    /// Runs a command through the shared command model.
    Run(Command, String),
    /// Prompts for a branch starting at the reference or commit.
    CreateBranch(String),
    ToggleColumn(HistoryColumn),
    ResetColumns,
    /// Confirms a conflicted file's working-tree content as its resolution.
    Resolve(String),
    Stage {
        change: FileChange,
        unstage: bool,
    },
}

impl MenuCommand {
    fn run(self, v: &mut GitCrab, window: &mut Window, cx: &mut Context<GitCrab>) {
        match self {
            Self::Run(command, target) => v.run_command(command, target, window, cx),
            Self::CreateBranch(start) => v.prompt_branch(Some(start), window, cx),
            Self::ToggleColumn(column) => {
                v.materialize_columns();
                v.history_columns.toggle(column);
                v.columns_changed(window, cx);
            }
            Self::ResetColumns => {
                v.history_columns = Default::default();
                v.columns_changed(window, cx);
            }
            Self::Resolve(path) => v.review_resolution(path, window, cx),
            Self::Stage { change, unstage } => v.mutate(
                Action::StageFile {
                    reviewed: change,
                    unstage,
                },
                window,
                cx,
            ),
        }
    }
}

impl MenuTarget<GitCrab> {
    /// An action running `command` on this view.
    pub(super) fn command(&self, command: MenuCommand) -> RepositoryMenu {
        RepositoryMenu(self.clone(), command)
    }
}

pub(super) fn register_menu_actions(cx: &mut App) {
    register(
        cx,
        |RepositoryMenu(target, command): &RepositoryMenu, cx| {
            let command = command.clone();
            target.update(cx, move |v, window, cx| command.run(v, window, cx));
        },
    );
}
