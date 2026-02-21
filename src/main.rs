use std::env;

fn print_usage(prog: &str) {
    println!(
        "Usage : {} {{server,stop,start,startlive,powerdown,reset,restart,create,delete,mountiso,livemigrate,backup}}",
        prog
    );
}

#[actix_web::main]
async fn main() {
    let args: Vec<String> = env::args().collect();
    let prog = &args[0];

    if args.len() < 2 {
        print_usage(prog);
        return;
    }

    let mode = &args[1];

    // Server mode
    if mode == "server" {
        let bind_addr = if args.len() >= 3 {
            args[2].clone()
        } else {
            "0.0.0.0:8080".to_string()
        };
        vm_ctl::server::start_server(&bind_addr).await.unwrap();
        return;
    }

    // CLI mode - show usage
    if args.len() == 2 {
        match mode.as_str() {
            "stop" => println!("Usage : {} stop '{{\"node_ip\": \"10.40.1.31\",\"smac\": \"52-54-c4-ca-42-38\"}}'", prog),
            "start" => println!("Usage : {} start '{{\"node\": {{\"ip\": \"10.40.1.31\"}},\"cpu\": {{\"sockets\": \"1\",\"cores\": \"2\",\"threads\": \"1\"}},\"memory\": {{\"size\": \"2048\"}},\"features\": {{\"is_windows\": \"0\"}},\"network_adapters\": [{{\"netid\": \"0\",\"mac\": \"52:54:c4:ca:42:38\",\"vlan\": \"0\"}}],\"disks\": [{{\"diskid\": \"0\",\"diskname\": \"52-54-c4-ca-42-38\",\"iops-total\": \"9600\",\"iops-total-max\": \"11520\",\"iops-total-max-length\": \"60\"}}]}}'", prog),
            "startlive" => println!("Usage : {} startlive", prog),
            "powerdown" => println!("Usage : {} powerdown '{{\"node_ip\": \"10.40.1.31\",\"smac\": \"52-54-c4-ca-42-38\"}}'", prog),
            "reset" => println!("Usage : {} reset '{{\"node_ip\": \"10.40.1.31\",\"smac\": \"52-54-c4-ca-42-38\"}}'", prog),
            "create" => println!("Usage : {} create '{{\"node_ip\": \"10.40.1.31\",\"smac\": \"52-54-c4-ca-42-38\",\"size\": \"40G\"}}'", prog),
            "copyimage" => println!("Usage : {} copyimage '{{\"node_ip\": \"10.40.1.31\",\"itemplate\": \"CentOS-7-x86_64-GenericCloud-1907\",\"smac\": \"52-54-c4-ca-42-38\",\"size\": \"40G\"}}'", prog),
            "listimage" => println!("Usage : {} listimage '{{\"node_ip\": \"10.40.1.31\",\"smac\": \"52-54-c4-ca-42-38\"}}'", prog),
            "delete" => println!("Usage : {} delete '{{\"node_ip\": \"10.40.1.31\",\"smac\": \"52-54-c4-ca-42-38\"}}'", prog),
            "mountiso" => println!("Usage : {} mountiso '{{\"node_ip\": \"10.40.1.31\",\"smac\": \"52-54-c4-ca-42-38\",\"isoname\":\"CentOS-7-x86_64-Minimal-1810.iso\"}}'", prog),
            "livemigrate" => println!("Usage : {} livemigrate", prog),
            "backup" => println!("Usage : {} backup", prog),
            _ => print_usage(prog),
        }
        return;
    }

    // CLI mode - execute command
    if args.len() == 3 {
        let json_str = &args[2];
        let result = match mode.as_str() {
            "stop" => vm_ctl::operations::stop(json_str),
            "start" | "startlive" => vm_ctl::operations::start(json_str),
            "powerdown" => vm_ctl::operations::powerdown(json_str),
            "reset" => vm_ctl::operations::reset(json_str),
            "create" => vm_ctl::operations::create(json_str),
            "copyimage" => vm_ctl::operations::copyimage(json_str),
            "listimage" => vm_ctl::operations::listimage(json_str),
            "delete" => vm_ctl::operations::delete_vm(json_str),
            "mountiso" => vm_ctl::operations::mountiso(json_str),
            "livemigrate" => vm_ctl::operations::livemigrate(json_str),
            "backup" => vm_ctl::operations::backup(json_str),
            _ => {
                print_usage(prog);
                return;
            }
        };
        match result {
            Ok(output) => print!("{}", output),
            Err(e) => eprintln!("ERROR: {}", e),
        }
    }
}
