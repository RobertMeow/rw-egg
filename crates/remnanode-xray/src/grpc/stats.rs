use tonic::transport::{Channel, ClientTlsConfig, Certificate, Identity};
use remnanode_proto::xray::app::stats::command::stats_service_client::StatsServiceClient;

pub struct StatsClient {
    inner: StatsServiceClient<Channel>,
}

impl StatsClient {
    pub async fn connect(
        addr: &str,
        ca_cert: &[u8],
        client_cert: &[u8],
        client_key: &[u8],
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let tls = ClientTlsConfig::new()
            .ca_certificate(Certificate::from_pem(ca_cert))
            .identity(Identity::from_pem(client_cert, client_key))
            .domain_name("internal.remnawave.local");

        let channel = Channel::from_shared(format!("http://{addr}"))?
            .tls_config(tls)?
            .connect()
            .await?;

        Ok(Self {
            inner: StatsServiceClient::new(channel),
        })
    }

    pub async fn get_sys_stats(&mut self) -> Result<serde_json::Value, String> {
        use remnanode_proto::xray::app::stats::command::SysStatsRequest;
        let response = self.inner.get_sys_stats(SysStatsRequest {})
            .await
            .map_err(|e| e.to_string())?;
        let s = response.into_inner();

        Ok(serde_json::json!({
            "NumGoroutine": s.num_goroutine,
            "NumGC": s.num_gc,
            "Alloc": s.alloc,
            "TotalAlloc": s.total_alloc,
            "Sys": s.sys,
            "Mallocs": s.mallocs,
            "Frees": s.frees,
            "LiveObjects": s.live_objects,
            "PauseTotalNs": s.pause_total_ns,
            "Uptime": s.uptime,
        }))
    }

    pub async fn query_stats(&mut self, pattern: &str, reset: bool) -> Result<serde_json::Value, String> {
        use remnanode_proto::xray::app::stats::command::QueryStatsRequest;
        let request = QueryStatsRequest {
            pattern: pattern.to_string(),
            reset,
        };
        let response = self.inner.query_stats(request)
            .await
            .map_err(|e| e.to_string())?;
        let stats = response.into_inner();

        let result: serde_json::Map<String, serde_json::Value> = stats.stat.iter()
            .map(|s| (s.name.clone(), serde_json::Value::Number(s.value.into())))
            .collect();

        Ok(serde_json::Value::Object(result))
    }

    pub async fn get_user_online(&mut self, name: &str, reset: bool) -> Result<bool, String> {
        use remnanode_proto::xray::app::stats::command::GetStatsRequest;
        let request = GetStatsRequest {
            name: name.to_string(),
            reset,
        };
        match self.inner.get_stats_online(request).await {
            Ok(response) => {
                let inner = response.into_inner();
                // If stat exists and has value > 0, user is online
                Ok(inner.stat.map_or(false, |s| s.value > 0))
            }
            Err(e) => {
                if e.code() == tonic::Code::NotFound {
                    Ok(false)
                } else {
                    Err(e.to_string())
                }
            }
        }
    }

    pub async fn get_online_ip_list(&mut self, name: &str, reset: bool) -> Result<serde_json::Value, String> {
        use remnanode_proto::xray::app::stats::command::GetStatsRequest;
        let request = GetStatsRequest {
            name: name.to_string(),
            reset,
        };
        match self.inner.get_stats_online_ip_list(request).await {
            Ok(response) => {
                let result = response.into_inner();
                let ips: serde_json::Map<String, serde_json::Value> = result.ips
                    .into_keys()
                    .map(|k| (k, serde_json::Value::Bool(true)))
                    .collect();
                Ok(serde_json::json!({"ips": serde_json::Value::Object(ips)}))
            }
            Err(e) => {
                if e.code() == tonic::Code::NotFound {
                    Ok(serde_json::json!({"ips": {}}))
                } else {
                    Err(e.to_string())
                }
            }
        }
    }
}
