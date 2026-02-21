use crate::config::get_conf;
use crate::ssh::send_cmd;

pub fn curl_request(url: &str) {
    match reqwest::blocking::get(url) {
        Ok(resp) => {
            let body = resp.text().unwrap_or_default();
            println!("DEBUG: size=> {}", body.len());
            println!("DEBUG: content=> {}", body);
        }
        Err(e) => {
            eprintln!("ERROR: {}", e);
        }
    }
}

pub fn set_ma_mode(mode: &str, smac: &str) {
    let domain = get_conf("domain");
    curl_request(&format!(
        "https://{}/api/v1.0/instances/{}/update-ma-mode/{}",
        domain, smac, mode
    ));
}

pub fn set_update_status(mode: &str, smac: &str) {
    let domain = get_conf("domain");
    curl_request(&format!(
        "https://{}/api/v1.0/instances/{}/update-status/{}",
        domain, smac, mode
    ));
}

pub fn send_cmd_pctl(ip: &str, mode: &str, smac: &str) -> String {
    let pctl_script = get_conf("pctl_script");
    let ctl_bin_path = get_conf("ctl_bin_path");
    let sendcmd = format!("{}/{} {} {}", ctl_bin_path, pctl_script, mode, smac);
    let mut output = format!("{}\n", sendcmd);
    if let Ok(out) = send_cmd(ip, &sendcmd) {
        output.push_str(&out);
    }
    output
}
