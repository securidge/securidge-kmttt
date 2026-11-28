//! `kmttt` — Securidge KMttt (Trusted Terminal Tracker)의 단일 실행 파일.

mod cli;
mod cmd;
mod integrations;
mod machines_cmd;
mod notify_chain;
mod plugin_cmd;
mod update;

use clap::Parser;

fn main() {
    let args = cli::Cli::parse();
    // sandbox-exec는 tokio를 띄우기 전에, 단일 스레드에서 제한을 건 뒤 exec해야 한다.
    if let Some(cli::Cmd::SandboxExec(a)) = &args.cmd {
        let r = kmttt_sandbox::exec_restricted(&kmttt_sandbox::RestrictedArgs {
            root: a.root.clone(),
            memory_mb: a.memory_mb,
            processes: a.processes,
            home_writable: a.home_writable.clone(),
            egress_port: a.egress_port,
            allow_tcp_ports: a.allow_tcp_port.clone(),
            program: a.command[0].clone(),
            args: a.command[1..].to_vec(),
        });
        let Err(e) = r;
        eprintln!("kmttt sandbox-exec: {e:#}");
        std::process::exit(126);
    }
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let code = rt.block_on(cmd::run(args));
    // 남은 백그라운드 작업(키 입력 스레드 등)을 기다리지 않고 끝낸다.
    rt.shutdown_timeout(std::time::Duration::from_millis(200));
    std::process::exit(code);
}
