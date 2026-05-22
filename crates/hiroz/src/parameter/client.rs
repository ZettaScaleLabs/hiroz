//! Remote ROS 2 parameter client helpers.

use std::sync::Arc;

use zenoh::Result;

use super::{
    Parameter, ParameterDescriptor, ParameterType, ParameterValue, SetParametersResult,
    wire_types::{
        DEPTH_RECURSIVE, DescribeParametersRequest, DescribeParametersSrv,
        GetParameterTypesRequest, GetParameterTypesSrv, GetParametersRequest, GetParametersSrv,
        ListParametersRequest, ListParametersSrv, SetParametersAtomicallyRequest,
        SetParametersAtomicallySrv, SetParametersRequest, SetParametersSrv,
    },
};
use crate::{Builder, node::ZNode, service::ZClient};

/// Target node for remote parameter operations.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ParameterTarget {
    pub namespace: String,
    pub name: String,
}

impl ParameterTarget {
    pub fn new(namespace: impl Into<String>, name: impl Into<String>) -> Self {
        let namespace = normalize_namespace(namespace.into());
        Self {
            namespace,
            name: name.into(),
        }
    }

    pub fn from_fqn(fqn: &str) -> Option<Self> {
        let fqn = fqn.trim();
        if !fqn.starts_with('/') {
            return None;
        }

        let mut parts = fqn.rsplitn(2, '/');
        let name = parts.next()?.to_string();
        let namespace = parts.next().unwrap_or("").to_string();
        Some(Self::new(namespace, name))
    }

    pub fn fully_qualified_name(&self) -> String {
        if self.namespace == "/" {
            format!("/{}", self.name)
        } else {
            format!("{}/{}", self.namespace, self.name)
        }
    }

    fn service_name(&self, suffix: &str) -> String {
        format!("{}/{}", self.fully_qualified_name(), suffix)
    }
}

fn normalize_namespace(namespace: String) -> String {
    let namespace = namespace.trim();
    if namespace.is_empty() || namespace == "/" {
        "/".to_string()
    } else if namespace.starts_with('/') {
        namespace.to_string()
    } else {
        format!("/{namespace}")
    }
}

/// Result of `list_parameters`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParameterList {
    pub names: Vec<String>,
    pub prefixes: Vec<String>,
}

/// High-level remote parameter client.
///
/// Clients are created once and reuse their Zenoh Queriers across calls,
/// avoiding per-call advertisement races. Use [`ParameterClient::new`] to
/// construct; the constructor is fallible because it declares Zenoh resources.
#[derive(Debug)]
pub struct ParameterClient {
    target: ParameterTarget,
    get_client: ZClient<GetParametersSrv>,
    set_client: ZClient<SetParametersSrv>,
    set_atomically_client: ZClient<SetParametersAtomicallySrv>,
    get_types_client: ZClient<GetParameterTypesSrv>,
    list_client: ZClient<ListParametersSrv>,
    describe_client: ZClient<DescribeParametersSrv>,
}

impl ParameterClient {
    /// Create a high-level client for the target node's parameter services.
    ///
    /// Declares all six Zenoh Queriers up front so they are ready before the
    /// first RPC call. Returns an error if any service client cannot be built.
    pub fn new(node: Arc<ZNode>, target: ParameterTarget) -> Result<Self> {
        let get_client = node
            .create_client::<GetParametersSrv>(&target.service_name("get_parameters"))
            .build()?;
        let set_client = node
            .create_client::<SetParametersSrv>(&target.service_name("set_parameters"))
            .build()?;
        let set_atomically_client = node
            .create_client::<SetParametersAtomicallySrv>(
                &target.service_name("set_parameters_atomically"),
            )
            .build()?;
        let get_types_client = node
            .create_client::<GetParameterTypesSrv>(&target.service_name("get_parameter_types"))
            .build()?;
        let list_client = node
            .create_client::<ListParametersSrv>(&target.service_name("list_parameters"))
            .build()?;
        let describe_client = node
            .create_client::<DescribeParametersSrv>(&target.service_name("describe_parameters"))
            .build()?;

        Ok(Self {
            target,
            get_client,
            set_client,
            set_atomically_client,
            get_types_client,
            list_client,
            describe_client,
        })
    }

    /// Return the target node this client addresses.
    pub fn target(&self) -> &ParameterTarget {
        &self.target
    }

    /// Describe remote parameters by name.
    pub async fn describe(&self, names: &[impl AsRef<str>]) -> Result<Vec<ParameterDescriptor>> {
        let response = self
            .describe_client
            .call(&DescribeParametersRequest {
                names: names.iter().map(|name| name.as_ref().to_string()).collect(),
            })
            .await?;
        Ok(response
            .descriptors
            .iter()
            .map(ParameterDescriptor::from_wire)
            .collect())
    }

    /// Fetch remote parameter values by name.
    pub async fn get(&self, names: &[impl AsRef<str>]) -> Result<Vec<ParameterValue>> {
        let response = self
            .get_client
            .call(&GetParametersRequest {
                names: names.iter().map(|name| name.as_ref().to_string()).collect(),
            })
            .await?;
        Ok(response
            .values
            .iter()
            .map(ParameterValue::from_wire)
            .collect())
    }

    /// Fetch remote parameter types by name.
    pub async fn get_types(&self, names: &[impl AsRef<str>]) -> Result<Vec<ParameterType>> {
        let response = self
            .get_types_client
            .call(&GetParameterTypesRequest {
                names: names.iter().map(|name| name.as_ref().to_string()).collect(),
            })
            .await?;
        Ok(response
            .types
            .into_iter()
            .map(|type_id| ParameterType::try_from(type_id).unwrap_or(ParameterType::NotSet))
            .collect())
    }

    /// List remote parameters, optionally constrained by prefixes and depth.
    pub async fn list(
        &self,
        prefixes: &[impl AsRef<str>],
        depth: Option<u64>,
    ) -> Result<ParameterList> {
        let response = self
            .list_client
            .call(&ListParametersRequest {
                prefixes: prefixes
                    .iter()
                    .map(|prefix| prefix.as_ref().to_string())
                    .collect(),
                depth: depth.unwrap_or(DEPTH_RECURSIVE),
            })
            .await?;
        Ok(ParameterList {
            names: response.result.names,
            prefixes: response.result.prefixes,
        })
    }

    /// Set one or more remote parameters non-atomically.
    pub async fn set(&self, parameters: &[Parameter]) -> Result<Vec<SetParametersResult>> {
        let response = self
            .set_client
            .call(&SetParametersRequest {
                parameters: parameters.iter().map(Parameter::to_wire).collect(),
            })
            .await?;
        Ok(response
            .results
            .into_iter()
            .map(|result| SetParametersResult {
                successful: result.successful,
                reason: result.reason,
            })
            .collect())
    }

    /// Set remote parameters atomically.
    pub async fn set_atomically(&self, parameters: &[Parameter]) -> Result<SetParametersResult> {
        let response = self
            .set_atomically_client
            .call(&SetParametersAtomicallyRequest {
                parameters: parameters.iter().map(Parameter::to_wire).collect(),
            })
            .await?;
        Ok(SetParametersResult {
            successful: response.result.successful,
            reason: response.result.reason,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use serial_test::serial;

    use super::{ParameterClient, ParameterTarget};
    use crate::{
        Builder,
        parameter::{
            Parameter, ParameterDescriptor, ParameterType, ParameterValue, SetParametersResult,
        },
        prelude::ZContextBuilder,
    };

    fn router_endpoint() -> String {
        std::env::var("ZENOH_ROUTER").unwrap_or_else(|_| "tcp/127.0.0.1:7447".to_string())
    }

    // ── ParameterTarget unit tests (no Zenoh needed) ─────────────────────────

    #[test]
    fn target_normalizes_namespace() {
        let target = ParameterTarget::new("robot", "nav");
        assert_eq!(target.namespace, "/robot");
        assert_eq!(target.fully_qualified_name(), "/robot/nav");
        assert_eq!(
            target.service_name("get_parameters"),
            "/robot/nav/get_parameters"
        );
    }

    #[test]
    fn target_root_namespace() {
        let target = ParameterTarget::new("/", "node");
        assert_eq!(target.namespace, "/");
        assert_eq!(target.fully_qualified_name(), "/node");
    }

    #[test]
    fn target_empty_namespace() {
        let target = ParameterTarget::new("", "node");
        assert_eq!(target.namespace, "/");
        assert_eq!(target.fully_qualified_name(), "/node");
    }

    #[test]
    fn target_from_fqn() {
        let target = ParameterTarget::from_fqn("/demo/controller").unwrap();
        assert_eq!(target.namespace, "/demo");
        assert_eq!(target.name, "controller");
    }

    #[test]
    fn target_from_fqn_root() {
        let target = ParameterTarget::from_fqn("/my_node").unwrap();
        assert_eq!(target.namespace, "/");
        assert_eq!(target.name, "my_node");
        assert_eq!(target.fully_qualified_name(), "/my_node");
    }

    #[test]
    fn target_from_fqn_rejects_relative() {
        assert!(ParameterTarget::from_fqn("no_slash").is_none());
    }

    #[test]
    fn target_accessor() {
        let target = ParameterTarget::new("ns", "n");
        // ParameterClient::new is fallible; verify the accessor without a live
        // router by checking the target directly on the struct value.
        assert_eq!(target.fully_qualified_name(), "/ns/n");
        assert_eq!(
            target.service_name("get_parameters"),
            "/ns/n/get_parameters"
        );
    }

    // ── ParameterClient integration tests (require Zenoh router) ─────────────

    fn make_server_and_client(
        server_name: &str,
        client_name: &str,
    ) -> (Arc<crate::node::ZNode>, ParameterClient) {
        let endpoint = router_endpoint();
        let ctx = ZContextBuilder::default()
            .with_connect_endpoints([endpoint])
            .build()
            .unwrap();
        let server = Arc::new(ctx.create_node(server_name).build().unwrap());
        let client_node = Arc::new(ctx.create_node(client_name).build().unwrap());
        let target = ParameterTarget::from_fqn(&format!("/{server_name}")).expect("valid fqn");
        let client = ParameterClient::new(Arc::clone(&client_node), target).unwrap();
        (server, client)
    }

    #[test]
    #[serial]
    fn client_get_and_set() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let (server, client) =
            make_server_and_client("param_client_get_set_srv", "param_client_get_set_cli");

        let desc = ParameterDescriptor::new("speed", ParameterType::Double);
        server
            .declare_parameter("speed", ParameterValue::Double(1.5), desc)
            .unwrap();

        rt.block_on(async {
            tokio::time::sleep(Duration::from_millis(300)).await;

            let values = client.get(&["speed"]).await.unwrap();
            assert_eq!(values, vec![ParameterValue::Double(1.5)]);

            let results = client
                .set(&[Parameter::new("speed", ParameterValue::Double(3.0))])
                .await
                .unwrap();
            assert_eq!(results, vec![SetParametersResult::success()]);

            let values = client.get(&["speed"]).await.unwrap();
            assert_eq!(values, vec![ParameterValue::Double(3.0)]);
        });
    }

    #[test]
    #[serial]
    fn client_get_types() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let (server, client) =
            make_server_and_client("param_client_types_srv", "param_client_types_cli");

        server
            .declare_parameter(
                "flag",
                ParameterValue::Bool(true),
                ParameterDescriptor::new("flag", ParameterType::Bool),
            )
            .unwrap();

        rt.block_on(async {
            tokio::time::sleep(Duration::from_millis(300)).await;

            let types = client.get_types(&["flag", "missing"]).await.unwrap();
            assert_eq!(types, vec![ParameterType::Bool, ParameterType::NotSet]);
        });
    }

    #[test]
    #[serial]
    fn client_describe() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let (server, client) =
            make_server_and_client("param_client_desc_srv", "param_client_desc_cli");

        server
            .declare_parameter(
                "count",
                ParameterValue::Integer(0),
                ParameterDescriptor::new("count", ParameterType::Integer),
            )
            .unwrap();

        rt.block_on(async {
            tokio::time::sleep(Duration::from_millis(300)).await;

            let descs = client.describe(&["count"]).await.unwrap();
            assert_eq!(descs.len(), 1);
            assert_eq!(descs[0].name, "count");
            assert_eq!(descs[0].type_, ParameterType::Integer);
        });
    }

    #[test]
    #[serial]
    fn client_list() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let (server, client) =
            make_server_and_client("param_client_list_srv", "param_client_list_cli");

        for name in ["a", "b", "c"] {
            server
                .declare_parameter(
                    name,
                    ParameterValue::Integer(1),
                    ParameterDescriptor::new(name, ParameterType::Integer),
                )
                .unwrap();
        }

        rt.block_on(async {
            tokio::time::sleep(Duration::from_millis(300)).await;

            let list = client.list(&[""], None).await.unwrap();
            assert!(list.names.contains(&"a".to_string()));
            assert!(list.names.contains(&"b".to_string()));
            assert!(list.names.contains(&"c".to_string()));
        });
    }

    #[test]
    #[serial]
    fn client_set_atomically_rejected() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let (server, client) =
            make_server_and_client("param_client_atomic_srv", "param_client_atomic_cli");

        server
            .declare_parameter(
                "val",
                ParameterValue::Integer(5),
                ParameterDescriptor::new("val", ParameterType::Integer),
            )
            .unwrap();

        server.on_set_parameters(|params| {
            for p in params {
                if let ParameterValue::Integer(v) = p.value
                    && v > 10
                {
                    return SetParametersResult::failure(format!("{} too large", p.name));
                }
            }
            SetParametersResult::success()
        });

        rt.block_on(async {
            tokio::time::sleep(Duration::from_millis(300)).await;

            let result = client
                .set_atomically(&[Parameter::new("val", ParameterValue::Integer(99))])
                .await
                .unwrap();
            assert!(!result.successful);
            assert!(result.reason.contains("too large"));

            // Value unchanged
            let values = client.get(&["val"]).await.unwrap();
            assert_eq!(values, vec![ParameterValue::Integer(5)]);
        });
    }
}
