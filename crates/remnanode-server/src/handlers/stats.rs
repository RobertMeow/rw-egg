use axum::{extract::State, response::Json, body::Bytes};
use crate::state::AppState;

fn parse_body(body: &Bytes) -> serde_json::Value {
    if body.is_empty() {
        return serde_json::json!({});
    }
    if let Ok(v) = serde_json::from_slice(body) {
        return v;
    }
    let s = String::from_utf8_lossy(body);
    if let Some(pos) = s.find('{') {
        if let Ok(v) = serde_json::from_str(&s[pos..]) {
            return v;
        }
    }
    serde_json::json!({})
}

fn body_reset(body: &serde_json::Value) -> bool {
    body.get("reset").and_then(|v| v.as_bool()).unwrap_or(false)
}

/// Запрос к Xray StatsService по префиксу; ошибки → пустой результат.
async fn query_raw(state: &AppState, pattern: &str, reset: bool) -> Vec<(String, i64)> {
    let mut xray = state.xray.write().await;
    let Some(client) = xray.stats_client.as_mut() else { return Vec::new() };
    match client.query_stats(pattern, reset).await {
        Ok(v) => v
            .as_object()
            .map(|o| o.iter().map(|(k, n)| (k.clone(), n.as_i64().unwrap_or(0))).collect())
            .unwrap_or_default(),
        Err(e) => {
            tracing::warn!("query_stats({pattern}) failed: {e}");
            Vec::new()
        }
    }
}

/// Сворачивает `<kind>>>>NAME>>>traffic>>>uplink|downlink` в [(NAME, uplink, downlink)].
fn aggregate_traffic(raw: Vec<(String, i64)>, kind: &str) -> Vec<(String, i64, i64)> {
    let prefix = format!("{kind}>>>");
    let mut map: std::collections::BTreeMap<String, (i64, i64)> = Default::default();
    for (name, value) in raw {
        let Some(rest) = name.strip_prefix(&prefix) else { continue };
        let Some((key, dir)) = rest.rsplit_once(">>>traffic>>>") else { continue };
        let entry = map.entry(key.to_string()).or_default();
        match dir {
            "uplink" => entry.0 += value,
            "downlink" => entry.1 += value,
            _ => {}
        }
    }
    map.into_iter().map(|(k, (up, down))| (k, up, down)).collect()
}

fn traffic_list(items: Vec<(String, i64, i64)>, key: &str) -> Vec<serde_json::Value> {
    items
        .into_iter()
        .filter(|(k, _, _)| k != "REMNAWAVE_API_INBOUND" && k != "REMNAWAVE_API")
        .map(|(k, up, down)| serde_json::json!({ key: k, "uplink": up, "downlink": down }))
        .collect()
}

pub async fn get_user_online_status(
    State(state): State<AppState>,
    body: Bytes,
) -> Json<serde_json::Value> {
    let body = parse_body(&body);
    let username = body.get("username").and_then(|v| v.as_str()).unwrap_or("");
    let name = format!("user>>>{username}>>>online");
    let mut xray = state.xray.write().await;
    let is_online = match xray.stats_client.as_mut() {
        Some(client) => client.get_user_online(&name, false).await.unwrap_or(false),
        None => false,
    };
    Json(serde_json::json!({"response": {"isOnline": is_online}}))
}

/// Панель 3.x шлёт `{reset}` и ждёт `{response:{users:[{username,uplink,downlink}]}}`.
pub async fn get_users_stats(
    State(state): State<AppState>,
    body: Bytes,
) -> Json<serde_json::Value> {
    let reset = body_reset(&parse_body(&body));
    let raw = query_raw(&state, "user>>>", reset).await;
    let users: Vec<serde_json::Value> = aggregate_traffic(raw, "user")
        .into_iter()
        .filter(|(_, up, down)| *up > 0 || *down > 0)
        .map(|(u, up, down)| serde_json::json!({"username": u, "uplink": up, "downlink": down}))
        .collect();
    Json(serde_json::json!({"response": {"users": users}}))
}

/// Ответ в формате remnanode 2.8+/3.x (GetSystemStatsCommand): панель 3.x отключает ноду,
/// если `xrayInfo` отсутствует («Required info is missing. Outdated version?»).
pub async fn get_system_stats(State(state): State<AppState>) -> Json<serde_json::Value> {
    // gRPC-клиенты создаются при старте xray и после его рестарта/обрыва остаются «мёртвыми»:
    // при ошибке переподключаемся и пробуем снова (до 3 раз).
    let sys_stats = {
        let mut xray = state.xray.write().await;
        let mut result = None;
        for attempt in 1..=3u32 {
            let outcome = match xray.stats_client.as_mut() {
                Some(client) => client.get_sys_stats().await,
                None => Err("stats client is not connected".to_string()),
            };
            match outcome {
                Ok(s) => {
                    tracing::info!("get_sys_stats succeeded on attempt {attempt}");
                    result = Some(s);
                    break;
                }
                Err(e) => {
                    tracing::warn!("get_sys_stats attempt {attempt} failed: {e}");
                    if xray.process.is_none() {
                        break; // xray не запущен — статистики не будет, ждём команду панели
                    }
                    let port = xray.xtls_api_port;
                    if let Err(ce) = xray.connect_grpc(port, &state.mtls_certs).await {
                        tracing::warn!("gRPC reconnect failed: {ce}");
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                }
            }
        }
        result
    };

    // get_sys_stats отдаёт ключи в стиле Xray (NumGoroutine…), контракт — camelCase.
    let xray_info = sys_stats.map(|s| {
        let n = |k: &str| s.get(k).and_then(|v| v.as_f64()).unwrap_or(0.0);
        serde_json::json!({
            "numGoroutine": n("NumGoroutine"),
            "numGC": n("NumGC"),
            "alloc": n("Alloc"),
            "totalAlloc": n("TotalAlloc"),
            "sys": n("Sys"),
            "mallocs": n("Mallocs"),
            "frees": n("Frees"),
            "liveObjects": n("LiveObjects"),
            "pauseTotalNs": n("PauseTotalNs"),
            "uptime": n("Uptime"),
        })
    });

    let reports_count = state.plugins.read().await.torrent_blocker.reports.len();

    Json(serde_json::json!({
        "response": {
            "xrayInfo": xray_info,
            "plugins": {
                "torrentBlocker": { "reportsCount": reports_count }
            },
            "system": {
                "stats": read_system_stats()
            }
        }
    }))
}

/// Лимит и потребление памяти контейнера (cgroup v2/v1), иначе — /proc/meminfo хоста.
fn memory_total_used() -> (f64, f64) {
    let read_num = |p: &str| -> Option<f64> {
        std::fs::read_to_string(p).ok().and_then(|s| s.trim().parse::<f64>().ok())
    };
    let meminfo = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let mem_kb = |key: &str| -> f64 {
        meminfo
            .lines()
            .find(|l| l.starts_with(key))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(0.0)
            * 1024.0
    };
    let host_total = mem_kb("MemTotal:");
    let host_used = (host_total - mem_kb("MemAvailable:")).max(0.0);

    let cgroup = read_num("/sys/fs/cgroup/memory.max")
        .zip(read_num("/sys/fs/cgroup/memory.current"))
        .or_else(|| {
            read_num("/sys/fs/cgroup/memory/memory.limit_in_bytes")
                .zip(read_num("/sys/fs/cgroup/memory/memory.usage_in_bytes"))
        });
    match cgroup {
        // "max" не парсится в число, огромный лимит v1 — тоже «без лимита»
        Some((limit, used)) if limit > 0.0 && (host_total == 0.0 || limit < host_total) => (limit, used.min(limit)),
        _ => (host_total, host_used),
    }
}

fn read_trimmed(p: &str) -> String {
    std::fs::read_to_string(p).map(|s| s.trim().to_string()).unwrap_or_default()
}

/// Интерфейсы из /proc/net/dev: (имя, rx_bytes, tx_bytes), без loopback.
fn net_interfaces() -> Vec<(String, u64, u64)> {
    std::fs::read_to_string("/proc/net/dev")
        .unwrap_or_default()
        .lines()
        .skip(2)
        .filter_map(|l| {
            let (name, rest) = l.split_once(':')?;
            let name = name.trim();
            if name == "lo" {
                return None;
            }
            let f: Vec<u64> = rest.split_whitespace().filter_map(|v| v.parse().ok()).collect();
            Some((name.to_string(), *f.first()?, *f.get(8)?))
        })
        .collect()
}

/// NodeSystemInfoSchema — отдаётся панели в ответе на старт xray.
pub fn system_info() -> serde_json::Value {
    let cpuinfo = std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
    let cpu_model = cpuinfo
        .lines()
        .find(|l| l.starts_with("model name"))
        .and_then(|l| l.split_once(':'))
        .map(|(_, v)| v.trim().to_string())
        .unwrap_or_default();
    let (memory_total, _) = memory_total_used();
    serde_json::json!({
        "arch": match std::env::consts::ARCH { "x86_64" => "x64", "aarch64" => "arm64", a => a },
        "cpus": num_cpus::get(),
        "cpuModel": cpu_model,
        "memoryTotal": memory_total,
        "hostname": read_trimmed("/proc/sys/kernel/hostname"),
        "platform": std::env::consts::OS,
        "release": read_trimmed("/proc/sys/kernel/osrelease"),
        "type": "Linux",
        "version": read_trimmed("/proc/sys/kernel/version"),
        "networkInterfaces": net_interfaces().into_iter().map(|(n, _, _)| n).collect::<Vec<_>>(),
    })
}

/// Предыдущий замер трафика интерфейса для расчёта скорости.
static NET_SAMPLE: std::sync::Mutex<Option<(std::time::Instant, u64, u64)>> = std::sync::Mutex::new(None);

fn interface_stats() -> serde_json::Value {
    // Основной интерфейс — с наибольшим трафиком
    let Some((name, rx, tx)) = net_interfaces().into_iter().max_by_key(|(_, rx, tx)| rx + tx) else {
        return serde_json::Value::Null;
    };
    let now = std::time::Instant::now();
    let (rx_rate, tx_rate) = {
        let mut prev = NET_SAMPLE.lock().unwrap_or_else(|e| e.into_inner());
        let rates = match *prev {
            Some((t, prx, ptx)) if rx >= prx && tx >= ptx => {
                let dt = now.duration_since(t).as_secs_f64().max(0.001);
                ((rx - prx) as f64 / dt, (tx - ptx) as f64 / dt)
            }
            _ => (0.0, 0.0),
        };
        *prev = Some((now, rx, tx));
        rates
    };
    serde_json::json!({
        "interface": name,
        "rxBytesPerSec": rx_rate.round(),
        "txBytesPerSec": tx_rate.round(),
        "rxTotal": rx,
        "txTotal": tx,
    })
}

/// NodeSystemStatsSchema: memoryFree/memoryUsed (байты), uptime (сек), loadAvg[3], interface|null.
pub fn read_system_stats() -> serde_json::Value {
    let (total, used) = memory_total_used();

    let uptime = std::fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|s| s.split_whitespace().next().and_then(|v| v.parse::<f64>().ok()))
        .unwrap_or(0.0);

    let load_avg: Vec<f64> = std::fs::read_to_string("/proc/loadavg")
        .unwrap_or_default()
        .split_whitespace()
        .take(3)
        .filter_map(|v| v.parse::<f64>().ok())
        .collect();
    let load_avg = if load_avg.len() == 3 { load_avg } else { vec![0.0, 0.0, 0.0] };

    serde_json::json!({
        "memoryFree": (total - used).max(0.0),
        "memoryUsed": used,
        "uptime": uptime,
        "loadAvg": load_avg,
        "interface": interface_stats(),
    })
}

pub async fn get_inbound_stats(
    State(state): State<AppState>,
    body: Bytes,
) -> Json<serde_json::Value> {
    let body = parse_body(&body);
    let tag = body.get("tag").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let raw = query_raw(&state, &format!("inbound>>>{tag}>>>traffic>>>"), body_reset(&body)).await;
    let (up, down) = aggregate_traffic(raw, "inbound")
        .into_iter()
        .find(|(k, _, _)| *k == tag)
        .map(|(_, u, d)| (u, d))
        .unwrap_or((0, 0));
    Json(serde_json::json!({"response": {"inbound": tag, "uplink": up, "downlink": down}}))
}

pub async fn get_outbound_stats(
    State(state): State<AppState>,
    body: Bytes,
) -> Json<serde_json::Value> {
    let body = parse_body(&body);
    let tag = body.get("tag").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let raw = query_raw(&state, &format!("outbound>>>{tag}>>>traffic>>>"), body_reset(&body)).await;
    let (up, down) = aggregate_traffic(raw, "outbound")
        .into_iter()
        .find(|(k, _, _)| *k == tag)
        .map(|(_, u, d)| (u, d))
        .unwrap_or((0, 0));
    Json(serde_json::json!({"response": {"outbound": tag, "uplink": up, "downlink": down}}))
}

pub async fn get_all_outbounds_stats(
    State(state): State<AppState>,
    body: Bytes,
) -> Json<serde_json::Value> {
    let reset = body_reset(&parse_body(&body));
    let outbounds = traffic_list(aggregate_traffic(query_raw(&state, "outbound>>>", reset).await, "outbound"), "outbound");
    Json(serde_json::json!({"response": {"outbounds": outbounds}}))
}

pub async fn get_all_inbounds_stats(
    State(state): State<AppState>,
    body: Bytes,
) -> Json<serde_json::Value> {
    let reset = body_reset(&parse_body(&body));
    let inbounds = traffic_list(aggregate_traffic(query_raw(&state, "inbound>>>", reset).await, "inbound"), "inbound");
    Json(serde_json::json!({"response": {"inbounds": inbounds}}))
}

pub async fn get_combined_stats(
    State(state): State<AppState>,
    body: Bytes,
) -> Json<serde_json::Value> {
    let reset = body_reset(&parse_body(&body));
    let inbounds = traffic_list(aggregate_traffic(query_raw(&state, "inbound>>>", reset).await, "inbound"), "inbound");
    let outbounds = traffic_list(aggregate_traffic(query_raw(&state, "outbound>>>", reset).await, "outbound"), "outbound");
    Json(serde_json::json!({"response": {"inbounds": inbounds, "outbounds": outbounds}}))
}

fn ip_entries(ips: Vec<(String, i64)>) -> Vec<serde_json::Value> {
    ips.into_iter()
        .map(|(ip, last_seen)| serde_json::json!({"ip": ip, "lastSeen": iso_time(last_seen)}))
        .collect()
}

pub async fn get_user_ip_list(
    State(state): State<AppState>,
    body: Bytes,
) -> Json<serde_json::Value> {
    let body = parse_body(&body);
    let user_id = body.get("userId").or_else(|| body.get("username")).and_then(|v| v.as_str()).unwrap_or("");
    let name = format!("user>>>{user_id}>>>online");
    let mut xray = state.xray.write().await;

    let ips = match xray.stats_client.as_mut() {
        Some(client) => client.get_online_ip_list(&name, false).await.unwrap_or_default(),
        None => Vec::new(),
    };
    Json(serde_json::json!({"response": {"ips": ip_entries(ips)}}))
}

/// Активные сессии: все онлайн-юзеры с их IP (панель: «кто и с какого IP»).
pub async fn get_users_ip_list(State(state): State<AppState>) -> Json<serde_json::Value> {
    let mut xray = state.xray.write().await;
    let Some(client) = xray.stats_client.as_mut() else {
        return Json(serde_json::json!({"response": {"users": []}}));
    };
    let names = client.get_all_online_users().await.unwrap_or_else(|e| {
        tracing::warn!("get_all_online_users failed: {e}");
        Vec::new()
    });
    let mut users = Vec::new();
    for name in names {
        let Some(user_id) = name.strip_prefix("user>>>").and_then(|r| r.strip_suffix(">>>online")) else { continue };
        let ips = client.get_online_ip_list(&name, false).await.unwrap_or_default();
        if !ips.is_empty() {
            users.push(serde_json::json!({"userId": user_id, "ips": ip_entries(ips)}));
        }
    }
    Json(serde_json::json!({"response": {"users": users}}))
}

/// Unix-время (сек) в ISO 8601 UTC без внешних зависимостей.
fn iso_time(secs: i64) -> String {
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Алгоритм civil_from_days (Howard Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.000Z", rem / 3600, rem % 3600 / 60, rem % 60)
}
