use tonic::transport::Channel;
use remnanode_proto::xray::app::stats::command::stats_service_client::StatsServiceClient;

pub struct StatsClient {
    inner: StatsServiceClient<Channel>,
}

impl StatsClient {
    pub async fn connect(
        addr: &str,
        _ca_cert: &[u8],
        _client_cert: &[u8],
        _client_key: &[u8],
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        // The API inbound is plaintext localhost-only, so no TLS is needed.
        let channel = Channel::from_shared(format!("http://{addr}"))?
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
            "numGoroutine": s.num_goroutine,
            "numGC": s.num_gc,
            "alloc": s.alloc,
            "totalAlloc": s.total_alloc,
            "sys": s.sys,
            "mallocs": s.mallocs,
            "frees": s.frees,
            "liveObjects": s.live_objects,
            "pauseTotalNs": s.pause_total_ns,
            "uptime": s.uptime,
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
                Ok(inner.stat.is_some_and(|s| s.value > 0))
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

    pub async fn get_online_ip_list(&mut self, name: &str, reset: bool) -> Result<std::collections::HashMap<String, i64>, String> {
        use remnanode_proto::xray::app::stats::command::GetStatsRequest;
        let request = GetStatsRequest {
            name: name.to_string(),
            reset,
        };
        match self.inner.get_stats_online_ip_list(request).await {
            Ok(response) => {
                let result = response.into_inner();
                Ok(result.ips)
            }
            Err(e) => {
                if e.code() == tonic::Code::NotFound {
                    Ok(std::collections::HashMap::new())
                } else {
                    Err(e.to_string())
                }
            }
        }
    }

    pub async fn get_all_online_users(&mut self) -> Result<Vec<String>, String> {
        use remnanode_proto::xray::app::stats::command::GetAllOnlineUsersRequest;
        let response = self.inner
            .get_all_online_users(GetAllOnlineUsersRequest {})
            .await
            .map_err(|e| e.to_string())?;
        Ok(response.into_inner().users)
    }

    pub async fn get_all_users_stats(&mut self, reset: bool) -> Result<Vec<(String, i64, i64)>, String> {
        // Use QueryStats instead of GetUsersStats: GetUsersStats is not present in
        // Xray-core releases up to (and including) v26.3.27, so calling it causes a
        // gRPC transport error that crashes the stats channel and makes the panel
        // mark the node as disconnected ("Required info is missing. Outdated version?").
        // QueryStats is available on all Xray-core versions and returns the same raw
        // user traffic counters, which we aggregate per email here.
        use remnanode_proto::xray::app::stats::command::QueryStatsRequest;
        let request = QueryStatsRequest {
            // Xray-core's QueryStats uses substring matching, not wildcards.
            // "user>>>" matches both "user>>>EMAIL>>>traffic>>>uplink"
            // and "user>>>EMAIL>>>traffic>>>downlink" counters.
            pattern: "user>>>".to_string(),
            reset,
        };
        let response = self.inner.query_stats(request)
            .await
            .map_err(|e| {
                tracing::error!("query_stats(pattern='user>>>', reset={reset}) failed: {e}");
                e.to_string()
            })?;
        let inner = response.into_inner();
        tracing::info!("query_stats(pattern='user>>>', reset={reset}) returned {} stat entries", inner.stat.len());

        let mut users: std::collections::HashMap<String, (i64, i64)> = std::collections::HashMap::new();
        for stat in inner.stat {
            let name = stat.name;
            let value = stat.value;

            // Counter names look like:
            //   user>>>EMAIL>>>traffic>>>uplink
            //   user>>>EMAIL>>>traffic>>>downlink
            let Some(rest) = name.strip_prefix("user>>>") else { continue };
            let Some((email, suffix)) = rest.split_once(">>>traffic>>>") else { continue };

            match suffix {
                "uplink" => users.entry(email.to_string()).or_default().0 = value,
                "downlink" => users.entry(email.to_string()).or_default().1 = value,
                _ => {}
            }
        }

        Ok(users.into_iter().map(|(email, (uplink, downlink))| (email, uplink, downlink)).collect())
    }

    /// Query traffic counters by a bare substring `prefix` and aggregate per tag.
    ///
    /// `prefix` carries no wildcard — xray's `QueryStats` does substring
    /// (`strings.Contains`) matching, so a literal `*` would never match (this
    /// is the bug the inbound/outbound handlers previously had). Examples:
    /// `"inbound>>>"`, `"outbound>>>"`, `"inbound>>>TAG>>>"`. Counter names look
    /// like `{prefix}TAG>>>traffic>>>(uplink|downlink)` and are aggregated into
    /// `(tag, uplink, downlink)`, mirroring how `get_all_users_stats` parses
    /// user counters.
    pub async fn get_traffic_stats(&mut self, prefix: &str, reset: bool) -> Result<Vec<(String, i64, i64)>, String> {
        use remnanode_proto::xray::app::stats::command::QueryStatsRequest;
        let request = QueryStatsRequest {
            pattern: prefix.to_string(),
            reset,
        };
        let response = self.inner.query_stats(request)
            .await
            .map_err(|e| e.to_string())?;
        let inner = response.into_inner();

        let mut map: std::collections::HashMap<String, (i64, i64)> = std::collections::HashMap::new();
        for stat in inner.stat {
            let Some(rest) = stat.name.strip_prefix(prefix) else { continue };
            let Some((tag, suffix)) = rest.split_once(">>>traffic>>>") else { continue };
            match suffix {
                "uplink" => map.entry(tag.to_string()).or_default().0 = stat.value,
                "downlink" => map.entry(tag.to_string()).or_default().1 = stat.value,
                _ => {}
            }
        }

        Ok(map.into_iter().map(|(tag, (up, down))| (tag, up, down)).collect())
    }
}
