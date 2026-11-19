//! An MCP server hub whose tools come from independently compiled modules.
//!
//! A module owns a set of tools whose names start with the module's name and
//! an underscore. The hub serves the tools of the modules it was given and
//! routes each call to the module that declared the tool.
#![forbid(unsafe_code)]

use std::{collections::BTreeMap, future::Future, pin::Pin, sync::Arc};

use rmcp::{
    ErrorData, RoleServer, ServerHandler,
    handler::server::{router::tool::ToolRouter, tool::ToolCallContext},
    model::{
        CallToolRequestParams, CallToolResponse, Implementation, ListToolsResult,
        PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
    },
    service::RequestContext,
};

/// A future returned by a module's tool call.
pub type CallFuture<'a> =
    Pin<Box<dyn Future<Output = Result<CallToolResponse, ErrorData>> + Send + 'a>>;

/// A group of tools the hub may serve.
pub trait Module: Send + Sync + 'static {
    /// Prefix of every tool name this module declares.
    fn name(&self) -> &'static str;
    /// One paragraph telling a client what the module is for.
    fn instructions(&self) -> &'static str;
    fn tools(&self) -> Vec<Tool>;
    fn call<'a>(
        &'a self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> CallFuture<'a>;
}

/// A module whose tools are declared with the `rmcp` tool macros on `S`.
pub struct Routed<S> {
    name: &'static str,
    instructions: &'static str,
    state: S,
    router: ToolRouter<S>,
}

impl<S: Send + Sync + 'static> Routed<S> {
    pub fn new(
        name: &'static str,
        instructions: &'static str,
        state: S,
        router: ToolRouter<S>,
    ) -> Self {
        Self {
            name,
            instructions,
            state,
            router,
        }
    }
}

impl<S: Send + Sync + 'static> Module for Routed<S> {
    fn name(&self) -> &'static str {
        self.name
    }
    fn instructions(&self) -> &'static str {
        self.instructions
    }
    fn tools(&self) -> Vec<Tool> {
        self.router.list_all()
    }
    fn call<'a>(
        &'a self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> CallFuture<'a> {
        Box::pin(
            self.router
                .call(ToolCallContext::new(&self.state, request, context)),
        )
    }
}

/// The MCP server: the tools of every module it was built with.
#[derive(Clone)]
pub struct Hub {
    modules: Arc<Vec<Arc<dyn Module>>>,
    owners: Arc<BTreeMap<String, usize>>,
    tools: Arc<Vec<Tool>>,
}

impl Hub {
    /// Build a hub over modules. Refuses a tool whose name lacks its module's
    /// prefix, and two modules declaring the same tool.
    pub fn new(modules: Vec<Arc<dyn Module>>) -> Result<Self, String> {
        let mut owners = BTreeMap::new();
        let mut tools = Vec::new();
        for (index, module) in modules.iter().enumerate() {
            let prefix = format!("{}_", module.name());
            for tool in module.tools() {
                if !tool.name.starts_with(&prefix) {
                    return Err(format!(
                        "module {} declares tool {} without its prefix",
                        module.name(),
                        tool.name
                    ));
                }
                if owners.insert(tool.name.to_string(), index).is_some() {
                    return Err(format!("tool {} is declared twice", tool.name));
                }
                tools.push(tool);
            }
        }
        tools.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(Self {
            modules: Arc::new(modules),
            owners: Arc::new(owners),
            tools: Arc::new(tools),
        })
    }

    pub fn module_names(&self) -> Vec<&'static str> {
        self.modules.iter().map(|module| module.name()).collect()
    }
}

impl ServerHandler for Hub {
    fn get_info(&self) -> ServerConfig {
        let instructions = self
            .modules
            .iter()
            .map(|module| format!("{}: {}", module.name(), module.instructions()))
            .collect::<Vec<_>>()
            .join("\n\n");
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                "fujimod-mcp",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(instructions)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult {
            tools: self.tools.as_ref().clone(),
            ..Default::default()
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let Some(&index) = self.owners.get(request.name.as_ref()) else {
            return Err(ErrorData::invalid_params(
                format!("unknown tool {}", request.name),
                None,
            ));
        };
        self.modules[index].call(request, context).await
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.tools.iter().find(|tool| tool.name == name).cloned()
    }
}
