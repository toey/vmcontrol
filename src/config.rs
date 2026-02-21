use std::collections::HashMap;

pub fn get_conf(name: &str) -> String {
    let config_path = "/opt/ctl/bin/config.yaml";
    let content = std::fs::read_to_string(config_path).unwrap_or_else(|e| {
        eprintln!("get conf error: cannot read {}: {}", config_path, e);
        String::new()
    });
    let map: HashMap<String, String> = serde_yaml::from_str(&content).unwrap_or_else(|e| {
        eprintln!("get conf error: parse yaml: {}", e);
        HashMap::new()
    });
    map.get(name).cloned().unwrap_or_else(|| {
        eprintln!("get conf error: key '{}' not found", name);
        String::new()
    })
}
