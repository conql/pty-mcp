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

        let ssh_tools = if self.app().ssh_mount_feature_available() {
            "ssh_connect, ssh_session_spawn, ssh_exec, ssh_run, ssh_read_file, \
             ssh_write_file, ssh_list_dir, ssh_mkdir, ssh_tunnel_open, \
             ssh_tunnel_close, ssh_mount, ssh_unmount, ssh_list, and ssh_disconnect"
        } else {
            "ssh_connect, ssh_session_spawn, ssh_exec, ssh_run, ssh_read_file, \
             ssh_write_file, ssh_list_dir, ssh_mkdir, ssh_tunnel_open, \
             ssh_tunnel_close, ssh_list, and ssh_disconnect"
        };
        let ssh_resources = if self.app().ssh_mount_feature_available() {
            "ssh://connections, ssh://tunnels, and ssh://mounts"
        } else {
            "ssh://connections and ssh://tunnels"
        };

        ServerInfo::new(capabilities).with_instructions(
            format!(
                "Manage PTY sessions through tools. Use pty_spawn, pty_write, pty_read, \
                 pty_list, pty_kill, and pty_wait for the main PTY workflow. PTY reads are \
                 agent-first: default to compact page.text, request line_number_mode=embedded \
                 only when precise line references matter, and set capture_limit only when you \
                 need initial_output. output_view=raw cannot be combined with \
                 line_number_mode=embedded. Use {ssh_tools} to manage SSH connections, remote \
                 sessions, remote files, tunnel summaries, and mount summaries. ssh_connect \
                 requires explicit auth_kind. SSH tunnels default to \
                 bind_host=127.0.0.1 and remote_host=127.0.0.1; non-loopback bind_host values must \
                 be explicitly allowed by policy. Resources expose read-only snapshots, including \
                 pty://sessions plus {ssh_resources}. \
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
