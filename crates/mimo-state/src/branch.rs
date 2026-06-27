use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
};

use anyhow::{Result, bail};
use mimo_protocol::ChatMessage;
use serde::{Deserialize, Serialize};

pub type MessageId = String;
pub type BranchId = String;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConversationNode {
    #[serde(default)]
    pub id: MessageId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<MessageId>,
    #[serde(default)]
    pub child_ids: Vec<MessageId>,
    // Keep ChatMessage transport-only for MiMo API compatibility.
    // Branch metadata lives in ConversationTree instead of ChatMessage.
    pub message: ChatMessage,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConversationBranch {
    #[serde(default)]
    pub id: BranchId,
    #[serde(default)]
    pub path: Vec<MessageId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_from: Option<MessageId>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BranchSummary {
    pub id: BranchId,
    pub is_current: bool,
    pub message_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_from: Option<MessageId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_from_message_number: Option<usize>,
    #[serde(default)]
    pub last_message_preview: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConversationTree {
    #[serde(default)]
    pub nodes: BTreeMap<MessageId, ConversationNode>,
    #[serde(default)]
    pub root_ids: Vec<MessageId>,
    #[serde(default)]
    pub branches: BTreeMap<BranchId, ConversationBranch>,
    #[serde(default = "default_branch_id")]
    pub current_branch_id: BranchId,
    #[serde(default = "default_next_message_seq")]
    pub next_message_seq: u64,
    #[serde(default = "default_next_branch_seq")]
    pub next_branch_seq: u64,
}

impl Default for ConversationTree {
    fn default() -> Self {
        Self::new()
    }
}

impl ConversationTree {
    pub fn new() -> Self {
        let branch_id = default_branch_id();
        let branch = ConversationBranch {
            id: branch_id.clone(),
            path: Vec::new(),
            created_from: None,
        };

        Self {
            nodes: BTreeMap::new(),
            root_ids: Vec::new(),
            branches: BTreeMap::from([(branch_id.clone(), branch)]),
            current_branch_id: branch_id,
            next_message_seq: default_next_message_seq(),
            next_branch_seq: default_next_branch_seq(),
        }
    }

    pub fn from_flat_messages(messages: Vec<ChatMessage>) -> Self {
        let mut tree = Self::new();
        for message in messages {
            tree.add_message_to_current(message);
        }
        tree
    }

    pub fn is_empty(&self) -> bool {
        self.current_path().is_empty()
    }

    pub fn current_branch_id(&self) -> &str {
        &self.current_branch_id
    }

    pub fn current_path(&self) -> &[MessageId] {
        self.branches
            .get(&self.current_branch_id)
            .map(|branch| branch.path.as_slice())
            .unwrap_or(&[])
    }

    pub fn current_messages(&self) -> Vec<ChatMessage> {
        self.current_path()
            .iter()
            .filter_map(|message_id| self.nodes.get(message_id))
            .map(|node| node.message.clone())
            .collect()
    }

    pub fn add_message_to_current(&mut self, message: ChatMessage) -> MessageId {
        let message_id = self.next_message_id();
        let parent_id = self.current_path().last().cloned();
        let node = ConversationNode {
            id: message_id.clone(),
            parent_id: parent_id.clone(),
            child_ids: Vec::new(),
            message,
        };

        self.nodes.insert(message_id.clone(), node);
        if let Some(parent_id) = parent_id {
            if let Some(parent) = self.nodes.get_mut(&parent_id) {
                push_unique(&mut parent.child_ids, message_id.clone());
                parent
                    .child_ids
                    .sort_by(|left, right| compare_message_ids(left, right));
            }
        } else {
            push_unique(&mut self.root_ids, message_id.clone());
            self.root_ids
                .sort_by(|left, right| compare_message_ids(left, right));
        }

        if let Some(branch) = self.branches.get_mut(&self.current_branch_id) {
            branch.path.push(message_id.clone());
        }

        message_id
    }

    pub fn create_branch_from_message(&mut self, message_id: &str) -> Result<BranchId> {
        let Some(index) = self.current_index_of_message(message_id) else {
            bail!("message {message_id} is not in the active branch");
        };

        let branch_id = self.next_branch_id();
        let path = self.current_path()[..=index].to_vec();
        let branch = ConversationBranch {
            id: branch_id.clone(),
            path,
            created_from: Some(message_id.to_string()),
        };

        self.branches.insert(branch_id.clone(), branch);
        self.current_branch_id = branch_id.clone();
        Ok(branch_id)
    }

    pub fn switch_branch(&mut self, branch_id: &str) -> Result<()> {
        if !self.branches.contains_key(branch_id) {
            bail!("unknown branch {branch_id}");
        }

        self.current_branch_id = branch_id.to_string();
        Ok(())
    }

    pub fn message_mut(&mut self, message_id: &str) -> Option<&mut ChatMessage> {
        self.nodes.get_mut(message_id).map(|node| &mut node.message)
    }

    pub fn message_id_at_current_index(&self, zero_based_index: usize) -> Option<&str> {
        self.current_path()
            .get(zero_based_index)
            .map(|id| id.as_str())
    }

    pub fn current_index_of_message(&self, message_id: &str) -> Option<usize> {
        self.current_path().iter().position(|id| id == message_id)
    }

    pub fn is_branch_point(&self, message_id: &str) -> bool {
        let mut next_steps = BTreeSet::new();

        for branch in self.branches.values() {
            if let Some(index) = branch.path.iter().position(|id| id == message_id) {
                next_steps.insert(branch.path.get(index + 1).cloned());
            }
        }

        next_steps.len() > 1
    }

    pub fn branch_summaries(&self) -> Vec<BranchSummary> {
        let mut summaries = self
            .branches
            .values()
            .map(|branch| {
                let last_message_preview = branch
                    .path
                    .last()
                    .and_then(|message_id| self.nodes.get(message_id))
                    .map(|node| preview_message(&node.message.content))
                    .unwrap_or_else(|| "Empty branch".to_string());
                let created_from_message_number =
                    branch.created_from.as_ref().and_then(|message_id| {
                        branch
                            .path
                            .iter()
                            .position(|path_id| path_id == message_id)
                            .map(|index| index + 1)
                    });

                BranchSummary {
                    id: branch.id.clone(),
                    is_current: branch.id == self.current_branch_id,
                    message_count: branch.path.len(),
                    created_from: branch.created_from.clone(),
                    created_from_message_number,
                    last_message_preview,
                }
            })
            .collect::<Vec<_>>();

        summaries.sort_by(|left, right| compare_branch_ids(&left.id, &right.id));
        summaries
    }

    pub fn branch_count(&self) -> usize {
        self.branches.len()
    }

    pub fn replace_current_branch_messages(&mut self, messages: Vec<ChatMessage>) {
        if let Some(branch) = self.branches.get_mut(&self.current_branch_id) {
            branch.path.clear();
            branch.created_from = None;
        }

        for message in messages {
            self.add_message_to_current(message);
        }

        self.prune_unreferenced_nodes();
    }

    pub fn validate_or_repair(&mut self) -> Result<()> {
        if self.branches.is_empty() {
            if self.nodes.is_empty() {
                *self = Self::new();
                return Ok(());
            }

            bail!("conversation tree has message nodes but no branches");
        }

        for (branch_id, branch) in &mut self.branches {
            branch.id = branch_id.clone();
        }
        for (message_id, node) in &mut self.nodes {
            node.id = message_id.clone();
        }

        if self.current_branch_id.trim().is_empty()
            || !self.branches.contains_key(&self.current_branch_id)
        {
            self.current_branch_id = sorted_branch_ids(self.branches.keys())
                .into_iter()
                .next()
                .unwrap_or_else(default_branch_id);
        }

        let mut referenced_ids = BTreeSet::new();
        let mut expected_parents = BTreeMap::<MessageId, Option<MessageId>>::new();

        for branch in self.branches.values_mut() {
            let mut seen = BTreeSet::new();
            for (index, message_id) in branch.path.iter().enumerate() {
                if !self.nodes.contains_key(message_id) {
                    bail!(
                        "branch {} references missing message {message_id}",
                        branch.id
                    );
                }
                if !seen.insert(message_id.clone()) {
                    bail!(
                        "branch {} contains duplicate message {message_id}",
                        branch.id
                    );
                }

                let expected_parent = if index == 0 {
                    None
                } else {
                    Some(branch.path[index - 1].clone())
                };
                if let Some(existing_parent) = expected_parents.get(message_id)
                    && existing_parent.as_deref() != expected_parent.as_deref()
                {
                    bail!("message {message_id} has inconsistent parents across branches");
                }

                expected_parents.insert(message_id.clone(), expected_parent);
                referenced_ids.insert(message_id.clone());
            }

            if let Some(created_from) = &branch.created_from
                && !branch
                    .path
                    .iter()
                    .any(|message_id| message_id == created_from)
            {
                branch.created_from = None;
            }
        }

        if referenced_ids.is_empty() && !self.nodes.is_empty() {
            bail!("conversation tree has message nodes but no branch paths");
        }

        self.nodes
            .retain(|message_id, _| referenced_ids.contains(message_id));
        self.rebuild_structure_from_paths();
        self.next_message_seq =
            next_sequence(self.nodes.keys(), "msg-", default_next_message_seq());
        self.next_branch_seq =
            next_sequence(self.branches.keys(), "branch-", default_next_branch_seq());

        Ok(())
    }

    fn next_message_id(&mut self) -> MessageId {
        let id = format_message_id(self.next_message_seq);
        self.next_message_seq += 1;
        id
    }

    fn next_branch_id(&mut self) -> BranchId {
        let id = format_branch_id(self.next_branch_seq);
        self.next_branch_seq += 1;
        id
    }

    fn prune_unreferenced_nodes(&mut self) {
        let referenced_ids = self
            .branches
            .values()
            .flat_map(|branch| branch.path.iter().cloned())
            .collect::<BTreeSet<_>>();
        self.nodes
            .retain(|message_id, _| referenced_ids.contains(message_id));
        self.rebuild_structure_from_paths();
    }

    fn rebuild_structure_from_paths(&mut self) {
        for node in self.nodes.values_mut() {
            node.parent_id = None;
            node.child_ids.clear();
        }

        let mut root_ids = Vec::new();
        for branch_id in sorted_branch_ids(self.branches.keys()) {
            let Some(branch) = self.branches.get(&branch_id) else {
                continue;
            };

            if let Some(root_id) = branch.path.first() {
                push_unique(&mut root_ids, root_id.clone());
            }

            for window in branch.path.windows(2) {
                let parent_id = &window[0];
                let child_id = &window[1];

                if let Some(child) = self.nodes.get_mut(child_id) {
                    child.parent_id = Some(parent_id.clone());
                }

                if let Some(parent) = self.nodes.get_mut(parent_id) {
                    push_unique(&mut parent.child_ids, child_id.clone());
                }
            }
        }

        root_ids.sort_by(|left, right| compare_message_ids(left, right));
        self.root_ids = root_ids;

        for node in self.nodes.values_mut() {
            node.child_ids
                .sort_by(|left, right| compare_message_ids(left, right));
            node.child_ids.dedup();
        }
    }
}

fn default_branch_id() -> BranchId {
    format_branch_id(1)
}

fn default_next_message_seq() -> u64 {
    1
}

fn default_next_branch_seq() -> u64 {
    2
}

fn format_message_id(seq: u64) -> MessageId {
    format!("msg-{seq}")
}

fn format_branch_id(seq: u64) -> BranchId {
    format!("branch-{seq}")
}

fn compare_message_ids(left: &str, right: &str) -> Ordering {
    compare_sequenced_ids(left, right, "msg-")
}

fn compare_branch_ids(left: &str, right: &str) -> Ordering {
    compare_sequenced_ids(left, right, "branch-")
}

fn compare_sequenced_ids(left: &str, right: &str, prefix: &str) -> Ordering {
    match (parse_sequence(left, prefix), parse_sequence(right, prefix)) {
        (Some(left), Some(right)) => left.cmp(&right),
        _ => left.cmp(right),
    }
}

fn parse_sequence(id: &str, prefix: &str) -> Option<u64> {
    id.strip_prefix(prefix)?.parse().ok()
}

fn next_sequence<'a>(ids: impl Iterator<Item = &'a String>, prefix: &str, minimum: u64) -> u64 {
    ids.filter_map(|id| parse_sequence(id, prefix))
        .max()
        .map(|value| value + 1)
        .unwrap_or(minimum)
        .max(minimum)
}

fn sorted_branch_ids<'a>(branch_ids: impl Iterator<Item = &'a String>) -> Vec<String> {
    let mut branch_ids = branch_ids.cloned().collect::<Vec<_>>();
    branch_ids.sort_by(|left, right| compare_branch_ids(left, right));
    branch_ids
}

fn preview_message(content: &str) -> String {
    let line = content.lines().next().unwrap_or_default().trim();
    if line.is_empty() {
        return "<empty>".to_string();
    }

    let preview = line.chars().take(60).collect::<String>();
    if line.chars().count() > 60 {
        format!("{preview}…")
    } else {
        preview
    }
}

fn push_unique<T: PartialEq>(items: &mut Vec<T>, value: T) {
    if !items.iter().any(|item| item == &value) {
        items.push(value);
    }
}

#[cfg(test)]
mod tests {
    use mimo_protocol::{ChatMessage, Role};

    use super::ConversationTree;

    fn sample_messages() -> Vec<ChatMessage> {
        vec![
            ChatMessage::user("hello"),
            ChatMessage::assistant("hi"),
            ChatMessage::user("branch here"),
            ChatMessage::assistant("original reply"),
        ]
    }

    #[test]
    fn empty_tree_defaults_are_branch_aware() {
        let tree = ConversationTree::new();

        assert!(tree.is_empty());
        assert_eq!(tree.current_branch_id(), "branch-1");
        assert_eq!(tree.branch_count(), 1);
        assert!(tree.current_path().is_empty());
        assert!(tree.current_messages().is_empty());
    }

    #[test]
    fn flat_messages_migrate_to_single_branch() {
        let messages = sample_messages();
        let tree = ConversationTree::from_flat_messages(messages.clone());

        assert_eq!(tree.branch_count(), 1);
        assert_eq!(tree.current_branch_id(), "branch-1");
        assert_eq!(tree.current_messages(), messages);
        assert_eq!(tree.current_path().len(), 4);
        assert_eq!(tree.next_message_seq, 5);
        assert_eq!(tree.next_branch_seq, 2);
    }

    #[test]
    fn create_branch_from_middle_message_preserves_original_branch() {
        let mut tree = ConversationTree::from_flat_messages(sample_messages());
        let branch_id = tree
            .create_branch_from_message("msg-2")
            .expect("branch should be created");

        assert_eq!(branch_id, "branch-2");
        assert_eq!(tree.current_branch_id(), "branch-2");
        assert_eq!(
            tree.current_path()
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec!["msg-1", "msg-2"]
        );
        assert_eq!(tree.branch_count(), 2);
        assert!(tree.is_branch_point("msg-2"));
        assert_eq!(
            tree.branches
                .get("branch-1")
                .expect("original branch should remain")
                .path
                .len(),
            4
        );
    }

    #[test]
    fn switching_branches_restores_each_path() {
        let mut tree = ConversationTree::from_flat_messages(sample_messages());
        tree.create_branch_from_message("msg-2")
            .expect("branch should be created");
        tree.add_message_to_current(ChatMessage::assistant("branched reply"));

        tree.switch_branch("branch-1")
            .expect("original branch should be selectable");
        assert_eq!(tree.current_messages(), sample_messages());

        tree.switch_branch("branch-2")
            .expect("branched path should be selectable");
        assert_eq!(tree.current_path().len(), 3);
        assert_eq!(
            tree.current_messages()
                .last()
                .map(|message| message.content.clone()),
            Some("branched reply".to_string())
        );
    }

    #[test]
    fn adding_messages_only_extends_the_active_branch() {
        let mut tree = ConversationTree::from_flat_messages(sample_messages());
        tree.create_branch_from_message("msg-3")
            .expect("branch should be created");
        let branched_message_id = tree.add_message_to_current(ChatMessage::assistant("new path"));

        assert_eq!(branched_message_id, "msg-5");
        assert_eq!(
            tree.current_path().last().map(String::as_str),
            Some("msg-5")
        );

        tree.switch_branch("branch-1")
            .expect("original branch should be selectable");
        assert_eq!(
            tree.current_path().last().map(String::as_str),
            Some("msg-4")
        );
        assert_eq!(
            tree.current_messages()
                .last()
                .map(|message| message.content.clone()),
            Some("original reply".to_string())
        );
        assert!(tree.is_branch_point("msg-3"));
    }

    #[test]
    fn replace_current_branch_messages_only_changes_active_branch() {
        let mut tree = ConversationTree::from_flat_messages(sample_messages());
        tree.create_branch_from_message("msg-2")
            .expect("branch should be created");
        tree.add_message_to_current(ChatMessage::user("new request"));
        tree.add_message_to_current(ChatMessage::assistant("new answer"));

        tree.replace_current_branch_messages(vec![
            ChatMessage::system("summary"),
            ChatMessage::user("new request"),
            ChatMessage::assistant("new answer"),
        ]);

        assert_eq!(tree.current_messages()[0].role, Role::System);
        assert_eq!(tree.current_messages()[0].content, "summary");

        tree.switch_branch("branch-1")
            .expect("original branch should still exist");
        assert_eq!(tree.current_messages(), sample_messages());
    }

    #[test]
    fn validate_or_repair_rebuilds_structure_and_prunes_orphans() {
        let mut tree = ConversationTree::from_flat_messages(sample_messages());
        tree.create_branch_from_message("msg-2")
            .expect("branch should be created");
        tree.nodes
            .get_mut("msg-1")
            .expect("node should exist")
            .child_ids
            .clear();
        tree.root_ids.clear();
        tree.nodes.insert(
            "msg-99".to_string(),
            super::ConversationNode {
                id: "msg-99".to_string(),
                parent_id: None,
                child_ids: Vec::new(),
                message: ChatMessage::assistant("orphan"),
            },
        );

        tree.validate_or_repair().expect("tree should repair");

        assert_eq!(
            tree.nodes
                .get("msg-1")
                .expect("node should remain")
                .child_ids,
            vec!["msg-2".to_string()]
        );
        assert_eq!(tree.root_ids, vec!["msg-1".to_string()]);
        assert!(!tree.nodes.contains_key("msg-99"));
    }
}
