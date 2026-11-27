//! 프로세스 표. pane의 전면 프로세스 찾기, 호출자 PID가 어느 pane에 속하는지 판별,
//! pane별 CPU·메모리 합계에 쓴다.

use std::collections::HashMap;
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

#[derive(Debug, Clone, PartialEq)]
pub struct ProcEntry {
    pub pid: u32,
    pub ppid: Option<u32>,
    /// 실행 파일 이름(소문자, .exe 제거)
    pub name: String,
    /// 명령줄 전체(공백으로 이음)
    pub cmdline: String,
    pub cpu: f32,
    pub mem: u64,
    pub start_time: u64,
}

/// 한 시점의 프로세스 목록.
#[derive(Default)]
pub struct ProcTable {
    sys: Option<System>,
    pub procs: HashMap<u32, ProcEntry>,
    children: HashMap<u32, Vec<u32>>,
}

impl ProcTable {
    pub fn new() -> Self {
        Self::default()
    }

    /// 시스템에서 프로세스 목록을 새로 읽는다. CPU 사용률은 두 번째 갱신부터 의미가 있다.
    pub fn refresh(&mut self) {
        if self.sys.is_none() {
            self.sys = System::new().ok();
        }
        let Some(sys) = self.sys.as_mut() else {
            return;
        };
        sys.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing()
                .with_cpu()
                .with_memory()
                .with_cmd(UpdateKind::OnlyIfNotSet),
        );
        let mut procs = HashMap::new();
        let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
        for (pid, p) in sys.processes() {
            let pid = pid.as_u32();
            let ppid = p.parent().map(|x| x.as_u32());
            let name = normalize_name(&p.name().to_string_lossy());
            let cmdline = p
                .cmd()
                .iter()
                .map(|s| s.to_string_lossy())
                .collect::<Vec<_>>()
                .join(" ");
            if let Some(pp) = ppid {
                children.entry(pp).or_default().push(pid);
            }
            procs.insert(
                pid,
                ProcEntry {
                    pid,
                    ppid,
                    name,
                    cmdline,
                    cpu: p.cpu_usage(),
                    mem: p.memory(),
                    start_time: p.start_time(),
                },
            );
        }
        self.procs = procs;
        self.children = children;
    }

    /// 테스트용: 임의의 표를 만든다.
    pub fn from_entries(entries: Vec<ProcEntry>) -> Self {
        let mut t = Self::default();
        for e in entries {
            if let Some(pp) = e.ppid {
                t.children.entry(pp).or_default().push(e.pid);
            }
            t.procs.insert(e.pid, e);
        }
        t
    }

    pub fn get(&self, pid: u32) -> Option<&ProcEntry> {
        self.procs.get(&pid)
    }

    /// `root`의 모든 자손(자신 제외).
    pub fn descendants(&self, root: u32) -> Vec<u32> {
        let mut out = Vec::new();
        let mut stack = vec![root];
        while let Some(p) = stack.pop() {
            if let Some(kids) = self.children.get(&p) {
                for &k in kids {
                    if k != root && !out.contains(&k) {
                        out.push(k);
                        stack.push(k);
                    }
                }
            }
        }
        out
    }

    /// `pid`가 `root` 자신이거나 그 자손인지. 조상 체인을 따라 올라간다.
    pub fn is_within(&self, pid: u32, root: u32) -> bool {
        let mut cur = Some(pid);
        let mut hops = 0;
        while let Some(p) = cur {
            if p == root {
                return true;
            }
            hops += 1;
            if hops > 64 {
                return false;
            }
            cur = self.procs.get(&p).and_then(|e| e.ppid);
        }
        false
    }

    /// pane의 전면 프로세스. Unix는 전면 프로세스 그룹 리더를 쓰고,
    /// 없으면(Windows) 가장 깊고 가장 늦게 시작한 자손을 고른다.
    pub fn foreground(&self, root: u32, pgid_hint: Option<u32>) -> Option<&ProcEntry> {
        if let Some(pg) = pgid_hint {
            if let Some(e) = self.procs.get(&pg) {
                if self.is_within(pg, root) {
                    return Some(e);
                }
            }
        }
        let mut best: Option<(&ProcEntry, usize)> = None;
        for pid in self.descendants(root) {
            let Some(e) = self.procs.get(&pid) else {
                continue;
            };
            if is_helper(&e.name) {
                continue;
            }
            let depth = self.depth(pid, root);
            let better = match best {
                None => true,
                Some((b, d)) => depth > d || (depth == d && e.start_time >= b.start_time),
            };
            if better {
                best = Some((e, depth));
            }
        }
        best.map(|b| b.0).or_else(|| self.procs.get(&root))
    }

    /// 전면 프로세스에서 pane 루트까지의 조상 체인(전면 프로세스가 맨 앞).
    pub fn chain(&self, from: u32, root: u32) -> Vec<&ProcEntry> {
        let mut out = Vec::new();
        let mut cur = Some(from);
        while let Some(p) = cur {
            let Some(e) = self.procs.get(&p) else { break };
            out.push(e);
            if p == root || out.len() > 32 {
                break;
            }
            cur = e.ppid;
        }
        out
    }

    fn depth(&self, pid: u32, root: u32) -> usize {
        let mut d = 0;
        let mut cur = Some(pid);
        while let Some(p) = cur {
            if p == root || d > 64 {
                break;
            }
            d += 1;
            cur = self.procs.get(&p).and_then(|e| e.ppid);
        }
        d
    }

    /// pane 프로세스 트리 전체의 CPU·메모리·프로세스 수.
    pub fn usage(&self, root: u32) -> (f32, u64, u32) {
        let mut cpu = 0.0;
        let mut mem = 0;
        let mut n = 0;
        for pid in std::iter::once(root).chain(self.descendants(root)) {
            if let Some(e) = self.procs.get(&pid) {
                cpu += e.cpu;
                mem += e.mem;
                n += 1;
            }
        }
        (cpu, mem, n)
    }
}

/// 전면 프로세스 후보에서 빼는 보조 프로세스(Windows 콘솔 호스트 등).
fn is_helper(name: &str) -> bool {
    matches!(name, "conhost" | "openconsole" | "kmttt")
}

/// "Claude.EXE" → "claude"
pub fn normalize_name(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    lower
        .strip_suffix(".exe")
        .map(str::to_string)
        .unwrap_or(lower)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(pid: u32, ppid: Option<u32>, name: &str, start: u64) -> ProcEntry {
        ProcEntry {
            pid,
            ppid,
            name: name.into(),
            cmdline: name.into(),
            cpu: 1.0,
            mem: 100,
            start_time: start,
        }
    }

    fn table() -> ProcTable {
        ProcTable::from_entries(vec![
            e(1, None, "init", 0),
            e(10, Some(1), "bash", 1),
            e(11, Some(10), "node", 2),
            e(12, Some(11), "rg", 3),
            e(20, Some(1), "zsh", 1),
        ])
    }

    #[test]
    fn descendants_and_within() {
        let t = table();
        let mut d = t.descendants(10);
        d.sort();
        assert_eq!(d, vec![11, 12]);
        assert!(t.is_within(12, 10));
        assert!(!t.is_within(20, 10));
        assert!(t.is_within(10, 10));
    }

    #[test]
    fn foreground_prefers_hint_then_deepest() {
        let t = table();
        assert_eq!(t.foreground(10, Some(11)).unwrap().pid, 11);
        assert_eq!(t.foreground(10, None).unwrap().pid, 12);
        // 힌트가 다른 트리면 무시한다.
        assert_eq!(t.foreground(10, Some(20)).unwrap().pid, 12);
        // 자식이 없으면 자기 자신.
        assert_eq!(t.foreground(20, None).unwrap().pid, 20);
    }

    #[test]
    fn usage_sums_tree() {
        let (cpu, mem, n) = table().usage(10);
        assert_eq!(n, 3);
        assert_eq!(mem, 300);
        assert!((cpu - 3.0).abs() < 0.01);
    }

    #[test]
    fn chain_walks_up_to_root() {
        let t = table();
        let names: Vec<_> = t.chain(12, 10).iter().map(|e| e.name.clone()).collect();
        assert_eq!(names, vec!["rg", "node", "bash"]);
    }

    #[test]
    fn names_normalized() {
        assert_eq!(normalize_name("Claude.EXE"), "claude");
        assert_eq!(normalize_name("node"), "node");
    }

    #[test]
    fn real_refresh_finds_self() {
        let mut t = ProcTable::new();
        t.refresh();
        assert!(t.get(std::process::id()).is_some());
    }
}
