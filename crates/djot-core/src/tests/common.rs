use std::path::PathBuf;

use crate::Workspace;

pub(super) struct WorkspaceFixture {
    pub(super) workspace: Workspace,
    pub(super) index: PathBuf,
    pub(super) topic: PathBuf,
    pub(super) renamed: PathBuf,
    pub(super) index_text: &'static str,
}

pub(super) fn workspace_fixture() -> WorkspaceFixture {
    let index = PathBuf::from("/notes/index.dj");
    let topic = PathBuf::from("/notes/topic.dj");
    let renamed = PathBuf::from("/notes/sub/renamed.dj");
    let index_text = "# Index\n\n[topic](topic.dj#topic) [missing](missing.dj)\n\n{#blocked created=\"2026-06-18T09:00:00Z\" depends=\"#open\"}\n::: task\nBlocked task.\n:::\n\n{#open created=\"2026-06-18T09:00:00Z\"}\n::: task\nOpen task.\n:::\n";
    let topic_text = "{#topic}\nTopic\n";
    let mut workspace = Workspace::new();
    workspace.insert(index.clone(), index_text.to_string());
    workspace.insert(topic.clone(), topic_text.to_string());
    WorkspaceFixture {
        workspace,
        index,
        topic,
        renamed,
        index_text,
    }
}
