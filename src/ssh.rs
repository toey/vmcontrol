use ssh2::Session;
use std::io::Read;
use std::net::TcpStream;
use std::path::Path;

pub fn send_cmd(address: &str, command: &str) -> Result<String, String> {
    let port = "22";
    let key_path = "/root/.ssh/id_rsa";

    let tcp = TcpStream::connect(format!("{}:{}", address, port))
        .map_err(|e| format!("unable to connect: {}", e))?;

    let mut sess = Session::new().map_err(|e| format!("unable to create session: {}", e))?;
    sess.set_tcp_stream(tcp);
    sess.handshake()
        .map_err(|e| format!("SSH handshake failed: {}", e))?;

    sess.userauth_pubkey_file("root", None, Path::new(key_path), None)
        .map_err(|e| format!("unable to authenticate: {}", e))?;

    let mut channel = sess
        .channel_session()
        .map_err(|e| format!("unable to create channel: {}", e))?;

    channel
        .exec(command)
        .map_err(|e| format!("unable to exec command: {}", e))?;

    let mut output = String::new();
    channel
        .read_to_string(&mut output)
        .map_err(|e| format!("unable to read output: {}", e))?;

    channel.wait_close().ok();
    Ok(output)
}
