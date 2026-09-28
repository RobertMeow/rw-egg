use serde_json::json;
use sysinfo::{LoadAvg, Networks, System};

fn load_avg() -> [f64; 3] {
    let LoadAvg { one, five, fifteen } = System::load_average();
    [one, five, fifteen]
}

/// Collect a one-time system snapshot matching upstream `NodeSystemSchema`.
pub fn collect_system_snapshot() -> serde_json::Value {
    let mut sys = System::new_all();
    sys.refresh_all();

    let cpus = sys.cpus();
    let cpu_model = cpus
        .first()
        .map(|c| c.brand().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    let networks = Networks::new_with_refreshed_list();
    let network_interfaces: Vec<String> = networks.keys().cloned().collect();

    json!({
        "info": {
            "arch": std::env::consts::ARCH,
            "cpus": cpus.len().max(1),
            "cpuModel": cpu_model,
            "memoryTotal": sys.total_memory(),
            "hostname": System::host_name().unwrap_or_default(),
            "platform": std::env::consts::OS,
            "release": System::kernel_version().unwrap_or_default(),
            "type": System::name().unwrap_or_default(),
            "version": System::os_version().unwrap_or_default(),
            "networkInterfaces": network_interfaces,
        },
        "stats": {
            "memoryFree": sys.available_memory(),
            "memoryUsed": sys.used_memory(),
            "uptime": System::uptime(),
            "loadAvg": load_avg(),
            "interface": null
        }
    })
}

/// Collect the `system.stats` portion with live network interface rates.
pub fn collect_system_stats_with_interface(
    interface: Option<serde_json::Value>,
) -> serde_json::Value {
    let mut sys = System::new_all();
    sys.refresh_memory();

    json!({
        "memoryFree": sys.available_memory(),
        "memoryUsed": sys.used_memory(),
        "uptime": System::uptime(),
        "loadAvg": load_avg(),
        "interface": interface,
    })
}
