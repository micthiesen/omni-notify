//! The shared workspace service: one instance backs the tasks, the email
//! handler, the routes and the MCP tools, so the in-process approval and
//! delivery exclusion holds across all of them.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use omni_ai::{Ai, AiTool};
use omni_api::workspaces::WorkspaceDefinition;
use omni_config::Config;
use omni_runtime::Ports;
use omni_tasks::TaskRegistry;
use tokio_util::task::TaskTracker;

use crate::notifications::{NotificationDelivery, NotificationOutbox, WorkspaceNotifier};
use crate::persistence::WorkspaceRepo;

/// What the service is built from (production: [`crate::subsystem`]).
pub struct WorkspaceDeps {
    pub repo: WorkspaceRepo,
    pub config: Arc<Config>,
    pub ai: Ai,
    pub web_search: Arc<dyn AiTool>,
    pub fetch_url: Arc<dyn AiTool>,
    pub notifier: Arc<dyn WorkspaceNotifier>,
    /// `None` uses the repository.
    pub outbox: Option<Arc<dyn NotificationOutbox>>,
    /// The `CalendarWriter` port is read at approval time.
    pub ports: Ports,
    pub tasks: TaskRegistry,
    pub tracker: TaskTracker,
}

/// Cheap to clone.
#[derive(Clone)]
pub struct WorkspaceService {
    pub(crate) inner: Arc<ServiceInner>,
}

pub(crate) struct ServiceInner {
    pub(crate) repo: WorkspaceRepo,
    pub(crate) definitions: Vec<WorkspaceDefinition>,
    pub(crate) config: Arc<Config>,
    pub(crate) ai: Ai,
    pub(crate) web_search: Arc<dyn AiTool>,
    pub(crate) fetch_url: Arc<dyn AiTool>,
    pub(crate) delivery: Arc<NotificationDelivery>,
    pub(crate) ports: Ports,
    pub(crate) tasks: TaskRegistry,
    pub(crate) tracker: TaskTracker,
    /// Actions with an approval or rejection in flight.
    pub(crate) resolving: Mutex<HashSet<String>>,
}

impl WorkspaceService {
    pub fn new(deps: WorkspaceDeps) -> Self {
        let outbox = deps
            .outbox
            .unwrap_or_else(|| Arc::new(deps.repo.clone()) as Arc<dyn NotificationOutbox>);
        let definitions =
            crate::definitions::workspace_definitions(&deps.config.workspace_schedule);
        Self {
            inner: Arc::new(ServiceInner {
                repo: deps.repo,
                definitions,
                config: deps.config,
                ai: deps.ai,
                web_search: deps.web_search,
                fetch_url: deps.fetch_url,
                delivery: Arc::new(NotificationDelivery::new(outbox, deps.notifier)),
                ports: deps.ports,
                tasks: deps.tasks,
                tracker: deps.tracker,
                resolving: Mutex::new(HashSet::new()),
            }),
        }
    }

    pub fn repo(&self) -> &WorkspaceRepo {
        &self.inner.repo
    }

    pub fn definitions(&self) -> &[WorkspaceDefinition] {
        &self.inner.definitions
    }

    pub fn definition(&self, id: &str) -> Option<&WorkspaceDefinition> {
        self.inner.definitions.iter().find(|d| d.id == id)
    }

    pub fn delivery(&self) -> &Arc<NotificationDelivery> {
        &self.inner.delivery
    }

    pub fn tasks(&self) -> &TaskRegistry {
        &self.inner.tasks
    }

    /// `WORKSPACES_PUBLIC_URL` (trailing slashes trimmed by the config).
    pub fn public_url(&self) -> &str {
        &self.inner.config.workspaces_public_url
    }
}
