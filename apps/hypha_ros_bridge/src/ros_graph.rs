//! ROS 1 master graph polling for automatic bridge mode.

use std::collections::BTreeMap;

use crate::automatic::RosTopicState;

/// A snapshot of the ROS 1 publish-subscribe graph.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RosGraphSnapshot {
    /// Topic states keyed by global topic name.
    pub topics: BTreeMap<String, RosTopicState>,
}

/// A small XML-RPC client for the ROS 1 master graph calls.
#[derive(Clone)]
pub struct RosGraphClient {
    client: reqwest::Client,
    master_uri: String,
    caller_id: String,
}

impl RosGraphClient {
    /// Creates a graph client for one registered ROS node.
    #[must_use]
    pub fn new(master_uri: &str, node_name: &str) -> Self {
        let caller_id =
            if node_name.starts_with('/') { node_name.to_owned() } else { format!("/{node_name}") };
        Self { client: reqwest::Client::new(), master_uri: master_uri.to_owned(), caller_id }
    }

    /// Reads publishers, subscribers, and topic types from the master.
    ///
    /// # Errors
    /// Returns an error when the master cannot be queried or returns an invalid response.
    pub async fn snapshot(&self) -> Result<RosGraphSnapshot, String> {
        let (state_result, types_result) = tokio::join!(
            self.call::<Vec<Vec<(String, Vec<String>)>>>(
                &self.master_uri,
                "getSystemState",
                vec![]
            ),
            self.call::<Vec<(String, String)>>(&self.master_uri, "getTopicTypes", vec![]),
        );
        let state = state_result?;
        let types = types_result?;
        if state.len() != 3 {
            return Err(format!("ROS master returned {} system-state sections", state.len()));
        }

        let mut topics = BTreeMap::new();
        for (topic, type_name) in types {
            topics.entry(topic).or_insert_with(RosTopicState::default).type_name = type_name;
        }
        add_roles(&mut topics, state.first(), true, &self.caller_id)?;
        add_roles(&mut topics, state.get(1), false, &self.caller_id)?;
        Ok(RosGraphSnapshot { topics })
    }

    /// The caller ID this client presents to the master and to publishers.
    #[must_use]
    pub fn caller_id(&self) -> &str {
        &self.caller_id
    }

    /// Asks the master for the XML-RPC URI of one node.
    ///
    /// # Errors
    /// Returns an error when the master cannot be queried or does not know the node.
    pub async fn lookup_node(&self, node: &str) -> Result<String, String> {
        self.call(&self.master_uri, "lookupNode", vec![node.into()]).await
    }

    /// Asks one publisher for the TCPROS endpoint of `topic`.
    ///
    /// # Errors
    /// Returns an error when the publisher cannot be queried or offers no TCPROS endpoint.
    pub async fn request_topic(
        &self,
        publisher_uri: &str,
        topic: &str,
    ) -> Result<(String, u16), String> {
        let protocols =
            serde_xmlrpc::Value::Array(vec![serde_xmlrpc::Value::Array(vec!["TCPROS".into()])]);
        let (protocol, host, port) = self
            .call::<(String, String, i32)>(
                publisher_uri,
                "requestTopic",
                vec![topic.into(), protocols],
            )
            .await?;
        if protocol != "TCPROS" {
            return Err(format!("publisher {publisher_uri} offers {protocol}, not TCPROS"));
        }
        let port = u16::try_from(port)
            .map_err(|_| format!("publisher {publisher_uri} offers invalid port {port}"))?;
        Ok((host, port))
    }

    /// Calls `method` at `uri`; every ROS call takes the caller ID first, so it
    /// goes in front of `args`.
    async fn call<T>(
        &self,
        uri: &str,
        method: &str,
        args: Vec<serde_xmlrpc::Value>,
    ) -> Result<T, String>
    where
        T: serde::de::DeserializeOwned,
    {
        let mut params = vec![self.caller_id.clone().into()];
        params.extend(args);
        let body = serde_xmlrpc::request_to_string(method, params)
            .map_err(|error| format!("cannot encode ROS request {method}: {error}"))?;
        let response = self
            .client
            .post(uri)
            .body(body)
            .send()
            .await
            .map_err(|error| format!("ROS request {method} to {uri} failed: {error}"))?
            .text()
            .await
            .map_err(|error| format!("cannot read ROS response {method}: {error}"))?;
        let (status, message, value) =
            serde_xmlrpc::response_from_str::<(i8, String, T)>(&response)
                .map_err(|error| format!("cannot parse ROS response {method}: {error}"))?;
        if status != 1 {
            return Err(format!("{uri} rejected {method}: {message}"));
        }
        Ok(value)
    }
}

/// Merges one `getSystemState` section into the topic map.
///
/// `published` selects the publisher section over the subscriber section. A
/// topic whose only endpoint is the bridge itself is skipped, or the bridge
/// would treat its own port as demand and route a topic back to its source.
///
/// # Errors
/// Returns an error when the master omitted the section.
pub fn add_roles(
    topics: &mut BTreeMap<String, RosTopicState>,
    section: Option<&Vec<(String, Vec<String>)>>,
    published: bool,
    caller_id: &str,
) -> Result<(), String> {
    let Some(section) = section else {
        return Err("ROS master returned an incomplete system state".into());
    };
    for (topic, nodes) in section {
        let external_nodes: Vec<&String> = nodes.iter().filter(|node| *node != caller_id).collect();
        if external_nodes.is_empty() {
            continue;
        }
        let state = topics.entry(topic.clone()).or_default();
        if published {
            state.publishers.extend(external_nodes.into_iter().cloned());
        } else {
            state.subscribed = true;
        }
    }
    Ok(())
}
