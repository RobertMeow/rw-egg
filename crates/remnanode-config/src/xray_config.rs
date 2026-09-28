use crate::MtlsCerts;

#[allow(clippy::too_many_arguments)]
pub fn generate_api_config(
    panel_config: &serde_json::Value,
    xtls_api_port: u16,
    xray_proxy_port: u16,
    node_port: u16,
    mtls: &MtlsCerts,
    torrent_blocker_enabled: bool,
    torrent_include_tags: &std::collections::HashSet<String>,
    internal_socket_path: &str,
    internal_token: &str,
) -> serde_json::Value {
    let mut config = panel_config.clone();

    // Merge: stats
    if config.get("stats").is_none() {
        config["stats"] = serde_json::json!({});
    }

    // Merge: api
    config["api"] = serde_json::json!({
        "services": ["HandlerService", "StatsService", "RoutingService"],
        "tag": "REMNAWAVE_API"
    });

    // Build API inbound
    let api_inbound = build_api_inbound(xtls_api_port, mtls);

    // Merge: inbounds (prepend API inbound)
    let mut inbounds = vec![api_inbound];
    if let Some(existing) = config.get("inbounds").and_then(|v| v.as_array()) {
        for mut inbound in existing.clone() {
            if let Some(obj) = inbound.as_object_mut() {
                let is_api = obj.get("tag").and_then(|v| v.as_str()) == Some("REMNAWAVE_API_INBOUND");
                let is_hysteria = obj.get("protocol").and_then(|v| v.as_str()) == Some("hysteria");

                if is_api {
                    // leave untouched
                } else if is_hysteria {
                    // Hysteria2 runs over QUIC/UDP, which the TCP-only SNI
                    // multiplexer can't carry. It gets its own UDP socket on
                    // the same port number as the public TCP mux — TCP and
                    // UDP are independent socket namespaces, so there's no
                    // conflict, and Pterodactyl allocations forward both
                    // protocols for a single port number.
                    obj.insert("listen".to_string(), serde_json::json!("0.0.0.0"));
                    obj.insert("port".to_string(), serde_json::json!(node_port));

                    // The panel's inbound template points at cert paths
                    // (e.g. `/certs/fullchain.cer`) that don't exist in this
                    // container. Always point at the fixed local path that
                    // `acme::ensure_cert` issues into instead.
                    let cert = crate::acme::cert_paths();
                    if let Some(tls) = obj
                        .get_mut("streamSettings")
                        .and_then(|v| v.as_object_mut())
                        .and_then(|s| s.get_mut("tlsSettings"))
                        .and_then(|v| v.as_object_mut())
                    {
                        tls.insert(
                            "certificates".to_string(),
                            serde_json::json!([{
                                "certificateFile": cert.cert_file.to_string_lossy(),
                                "keyFile": cert.key_file.to_string_lossy(),
                            }]),
                        );
                    }
                } else {
                    // Redirect all other non-API (TCP) inbounds to the
                    // internal proxy port for the mux to forward to.
                    obj.insert("listen".to_string(), serde_json::json!("127.0.0.1"));
                    obj.insert("port".to_string(), serde_json::json!(xray_proxy_port));
                }
            }
            inbounds.push(inbound);
        }
    }
    config["inbounds"] = serde_json::Value::Array(inbounds);

    // Merge: outbounds
    if config.get("outbounds").is_none() {
        config["outbounds"] = serde_json::json!([]);
    }

    // Build policy.
    //
    // `statsUserOnline` is forced off: xray's `>>>online` stat needs kernel
    // connection tracking (CAP_NET_ADMIN) which a non-root Pterodactyl container
    // lacks, and the node-side traffic accumulator now derives online status from
    // traffic recency. Keeping it off also means the `"user>>>"` QueryStats
    // pattern matches only `traffic>>>uplink|downlink` counters, so the
    // accumulator's reset:true polls own exactly those counters with no
    // interference. Per-user uplink/downlink remain unconditionally enabled.
    config["policy"] = serde_json::json!({
        "levels": {
            "0": {
                "statsUserUplink": true,
                "statsUserDownlink": true,
                "statsUserOnline": false
            }
        },
        "system": {
            "statsInboundDownlink": true,
            "statsInboundUplink": true,
            "statsOutboundDownlink": true,
            "statsOutboundUplink": true
        }
    });

    // Merge: routing rules (prepend API routing rule)
    let api_routing_rule = serde_json::json!({
        "inboundTag": ["REMNAWAVE_API_INBOUND"],
        "outboundTag": "REMNAWAVE_API"
    });

    let mut rules = vec![api_routing_rule];
    if let Some(existing_rules) = config.pointer("/routing/rules").and_then(|v| v.as_array()) {
        rules.extend(existing_rules.clone());
    }

    // Torrent blocker
    if torrent_blocker_enabled {
        let webhook_url = format!(
            "/{internal_socket_path}:/internal/webhook?token={internal_token}"
        );

        // Add blackhole outbound
        if let Some(outbounds) = config.get_mut("outbounds").and_then(|v| v.as_array_mut()) {
            outbounds.push(serde_json::json!({
                "tag": "RW_TB_OUTBOUND_BLOCK",
                "protocol": "blackhole"
            }));
        }

        // Add torrent routing rule at position 1
        let torrent_rule = serde_json::json!({
            "protocol": ["bittorrent"],
            "outboundTag": "RW_TB_OUTBOUND_BLOCK",
            "webhook": {
                "url": webhook_url,
                "deduplication": 5
            }
        });

        if rules.len() > 1 {
            rules.insert(1, torrent_rule);
        } else {
            rules.push(torrent_rule);
        }

        // Inject webhooks into matching rule tags
        for rule in &mut rules {
            if let Some(obj) = rule.as_object_mut() {
                if let Some(tag) = obj.get("ruleTag").and_then(|v| v.as_str()) {
                    if torrent_include_tags.contains(tag) {
                        obj.insert(
                            "webhook".to_string(),
                            serde_json::json!({
                                "url": webhook_url,
                                "deduplication": 5
                            }),
                        );
                    }
                }
            }
        }
    }

    // Set routing
    let mut routing = config.get("routing").cloned().unwrap_or(serde_json::json!({}));
    routing["rules"] = serde_json::Value::Array(rules);
    config["routing"] = routing;

    config
}

fn build_api_inbound(port: u16, _mtls: &MtlsCerts) -> serde_json::Value {
    // Use a plaintext (no TLS) API inbound. The API port is bound to
    // 127.0.0.1 and is only reachable from the node process itself, so TLS
    // is unnecessary and avoids compatibility issues between tonic/rustls and
    // Xray-core's mTLS API inbound on some setups.
    serde_json::json!({
        "tag": "REMNAWAVE_API_INBOUND",
        "port": port,
        "listen": "127.0.0.1",
        "protocol": "dokodemo-door",
        "settings": {
            "address": "127.0.0.1"
        }
    })
}
