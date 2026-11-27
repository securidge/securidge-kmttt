//! 원격 머신 프로필 관리. 추가할 때 SSH 연결·원격 kmttt·프로토콜 호환을 확인한다.

use crate::cli::MachineCmd;
use anyhow::{bail, Result};
use kmttt_core::client::RpcClient;
use kmttt_core::machines::{valid_target, MachineProfile, Machines};
use kmttt_core::paths;
use serde_json::json;
use std::time::Duration;

pub async fn run(mc: MachineCmd) -> Result<i32> {
    let file = paths::machines_file();
    let mut ms = Machines::load(&file)?;
    match mc {
        MachineCmd::Add {
            target,
            label,
            remote_session,
            bin,
            no_check,
        } => {
            if !valid_target(&target) {
                bail!("invalid ssh target '{target}'");
            }
            if !no_check {
                eprintln!("checking ssh {target} …");
                let (c, _) = tokio::time::timeout(
                    Duration::from_secs(20),
                    RpcClient::connect_ssh(&target, &remote_session, &bin),
                )
                .await
                .map_err(|_| anyhow::anyhow!("ssh timed out"))??;
                let st = c
                    .call_timeout("server.status", json!({}), Duration::from_secs(15))
                    .await
                    .map_err(|e| anyhow::anyhow!("remote kmttt did not answer ({e}); is `{bin}` installed on the remote PATH?"))?;
                if st["protocol"].as_u64() != Some(kmttt_proto::PROTOCOL_VERSION as u64) {
                    bail!(
                        "remote protocol {} is not compatible with this client ({})",
                        st["protocol"],
                        kmttt_proto::PROTOCOL_VERSION
                    );
                }
                eprintln!(
                    "ok: remote kmttt {} (session {})",
                    st["version"], st["session"]
                );
            }
            let id = ms.next_id();
            ms.machine.push(MachineProfile {
                id: id.clone(),
                label: label.unwrap_or_else(|| target.clone()),
                target,
                session: remote_session,
                enabled: true,
                bin,
            });
            ms.save(&file)?;
            println!("{}", json!({"added": id}));
        }
        MachineCmd::List => println!("{}", serde_json::to_string_pretty(&ms.machine)?),
        MachineCmd::Rename { id, label } => {
            ms.get_mut(&id)
                .ok_or_else(|| anyhow::anyhow!("no machine {id}"))?
                .label = label;
            ms.save(&file)?;
        }
        MachineCmd::Enable { id } => {
            ms.get_mut(&id)
                .ok_or_else(|| anyhow::anyhow!("no machine {id}"))?
                .enabled = true;
            ms.save(&file)?;
        }
        MachineCmd::Disable { id } => {
            ms.get_mut(&id)
                .ok_or_else(|| anyhow::anyhow!("no machine {id}"))?
                .enabled = false;
            ms.save(&file)?;
        }
        MachineCmd::Remove { id } => {
            let before = ms.machine.len();
            ms.machine.retain(|m| m.id != id);
            if ms.machine.len() == before {
                bail!("no machine {id}");
            }
            ms.save(&file)?;
        }
    }
    Ok(0)
}
