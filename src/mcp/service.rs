use std::sync::Arc;

use rmcp::{
    ErrorData as McpError, ServerHandler,
    handler::server::router::tool::ToolRouter,
    model::{
        ListResourceTemplatesResult, ListResourcesResult, PaginatedRequestParams,
        ReadResourceRequestParams, ReadResourceResult, ServerCapabilities, ServerInfo,
        TasksCapability, Tool,
    },
    service::{RequestContext, RoleServer},
    task_handler, tool_handler,
};

use crate::{
    AppState,
    mcp::{resources, tasks},
};

#[derive(Clone)]
pub struct PtyMcpServer {
    app: Arc<AppState>,
    tool_router: ToolRouter<Self>,
    processor: tasks::TaskProcessor,
}

impl PtyMcpServer {
    pub fn new(app: Arc<AppState>) -> Self {
        Self {
            app,
            tool_router: Self::tool_router(),
            processor: tasks::new_task_processor(),
        }
    }

    pub fn app(&self) -> &Arc<AppState> {
        &self.app
    }

    pub fn tool_definitions(&self) -> Vec<Tool> {
        self.tool_router.list_all().to_vec()
    }
}

#[tool_handler(router = self.tool_router)]
#[task_handler]
impl ServerHandler for PtyMcpServer {
    fn get_info(&self) -> ServerInfo {
        let mut capabilities = ServerCapabilities::builder()
            .enable_tools()
            .enable_resources()
            .build();
        capabilities.tasks = Some(TasksCapability::server_default());

        ServerInfo::new(capabilities).with_instructions(
            "Manage PTY sessions through tools. Use pty_spawn, pty_write, pty_read, pty_list, \
                 pty_kill, and pty_wait for the main PTY workflow. Use ssh_connect, \
                 ssh_session_spawn, ssh_mount, ssh_unmount, ssh_list, and ssh_disconnect to \
                 manage SSH connections, remote sessions, and mount summaries. Resources expose \
                 read-only snapshots, including pty://sessions plus ssh://connections and \
                 ssh://mounts, and tasks are available as an optional enhancement.",
        )
    }

    fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListResourcesResult, McpError>> + Send + '_ {
        std::future::ready(Ok(resources::list_resources(self.app())))
    }

    fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListResourceTemplatesResult, McpError>> + Send + '_ {
        std::future::ready(Ok(resources::list_resource_templates()))
    }

    fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ReadResourceResult, McpError>> + Send + '_ {
        std::future::ready(resources::read_resource(self.app(), &request.uri))
    }
}
