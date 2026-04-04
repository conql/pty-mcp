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
        let mut tool_router = Self::tool_router();
        if !app.ssh_mount_feature_available() {
            tool_router.remove_route("ssh_mount");
            tool_router.remove_route("ssh_unmount");
        }

        Self {
            app,
            tool_router,
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

        let ssh_mount_tools = if self.app().ssh_mount_feature_available() {
            ", ssh_mount, ssh_unmount,"
        } else {
            ","
        };
        let ssh_mount_resources = if self.app().ssh_mount_feature_available() {
            " and ssh://mounts"
        } else {
            ""
        };

        ServerInfo::new(capabilities).with_instructions(
            format!(
                "Manage PTY sessions through tools. Use pty_spawn, pty_write, pty_read, \
                 pty_list, pty_kill, and pty_wait for the main PTY workflow. PTY reads are \
                 agent-first: default to compact page.text, request line_number_mode=embedded \
                 only when precise line references matter, and set capture_limit only when you \
                 need initial_output. output_view=raw cannot be combined with \
                 line_number_mode=embedded. Use ssh_connect, ssh_session_spawn, ssh_exec, \
                 ssh_run, ssh_read_file, ssh_write_file, ssh_list_dir, ssh_mkdir{ssh_mount_tools} \
                 ssh_list, and ssh_disconnect to manage SSH connections, remote sessions, remote \
                 files, and mount summaries. ssh_connect requires explicit auth_kind. Resources \
                 expose read-only snapshots, including pty://sessions plus ssh://connections{ssh_mount_resources}. \
                 When SSH mount support is unavailable or a mount fails because local prerequisites \
                 are missing, read ssh://docs/mount-setup and the matching \
                 ssh://docs/mount-setup/{{platform}} guide before suggesting installation steps. \
                 Tasks are available as an optional enhancement."
            ),
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
        std::future::ready(Ok(resources::list_resource_templates(self.app())))
    }

    fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ReadResourceResult, McpError>> + Send + '_ {
        std::future::ready(resources::read_resource(self.app(), &request.uri))
    }
}
