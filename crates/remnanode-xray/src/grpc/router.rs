use tonic::transport::Channel;
use remnanode_proto::xray::app::router::command::routing_service_client::RoutingServiceClient;
use remnanode_proto::xray::common::serial::TypedMessage;

pub struct RouterClient {
    inner: RoutingServiceClient<Channel>,
}

impl RouterClient {
    pub async fn connect(
        addr: &str,
        _ca_cert: &[u8],
        _client_cert: &[u8],
        _client_key: &[u8],
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let channel = Channel::from_shared(format!("http://{addr}"))?
            .connect()
            .await?;

        Ok(Self {
            inner: RoutingServiceClient::new(channel),
        })
    }

    pub async fn add_rule(
        &mut self,
        _rule_tag: &str,
        source_ips: &[String],
        outbound_tag: &str,
    ) -> Result<(), String> {
        use remnanode_proto::xray::app::router::command::AddRuleRequest;

        // Build the routing rule as a TypedMessage containing the rule config
        let rule_json = serde_json::json!({
            "type": "field",
            "source": source_ips,
            "outboundTag": outbound_tag,
        });

        let request = AddRuleRequest {
            config: Some(TypedMessage {
                r#type: "type.googleapis.com/xray.app.router.RoutingRule".to_string(),
                value: serde_json::to_vec(&rule_json).unwrap_or_default(),
            }),
            should_append: false,
        };

        self.inner.add_rule(request)
            .await
            .map_err(|e| e.to_string())?;

        Ok(())
    }

    pub async fn remove_rule(&mut self, rule_tag: &str) -> Result<(), String> {
        use remnanode_proto::xray::app::router::command::RemoveRuleRequest;

        let request = RemoveRuleRequest {
            rule_tag: rule_tag.to_string(),
        };

        self.inner.remove_rule(request)
            .await
            .map_err(|e| e.to_string())?;

        Ok(())
    }
}
