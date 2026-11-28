//! 명령줄 정의.

use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "kmttt",
    version,
    about = "Securidge KMttt — Trusted Terminal Tracker: a security-hardened terminal multiplexer and control center for AI coding agents",
    after_help = "Run `kmttt` with no command to attach (the server starts automatically).\nInside the UI press ctrl+b then ? for key bindings, ctrl+b then d for the Control Center.\nMade by Kwangmo Kim."
)]
pub struct Cli {
    /// 세션 이름(세션마다 서버·소켓·워크스페이스가 따로)
    #[arg(
        long,
        short = 's',
        global = true,
        default_value = "default",
        env = "KMTTT_SESSION_NAME"
    )]
    pub session: String,
    /// 원격 머신의 서버에 SSH로 붙는다: user@host 또는 ssh://user@host:port
    #[arg(long)]
    pub remote: Option<String>,
    /// 원격 서버의 세션 이름
    #[arg(long, default_value = "default")]
    pub remote_session: String,
    /// 원격 kmttt 실행 파일
    #[arg(long, default_value = "kmttt")]
    pub remote_bin: String,
    /// 관제 모드로 바로 연다
    #[arg(long)]
    pub control: bool,
    #[command(subcommand)]
    pub cmd: Option<Cmd>,
}

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// 서버 상태(클라이언트·서버 버전)
    Status,
    /// 서버 관리
    #[command(subcommand)]
    Server(ServerCmd),
    /// 세션 관리
    #[command(subcommand)]
    Session(SessionCmd),
    /// 워크스페이스
    #[command(subcommand)]
    Workspace(WorkspaceCmd),
    /// 탭
    #[command(subcommand)]
    Tab(TabCmd),
    /// pane
    #[command(subcommand)]
    Pane(PaneCmd),
    /// 에이전트
    #[command(subcommand)]
    Agent(AgentCmd),
    /// git worktree 워크스페이스
    #[command(subcommand)]
    Worktree(WorktreeCmd),
    /// 승인 대기열(사람만 결정할 수 있다)
    #[command(subcommand)]
    Approval(ApprovalCmd),
    /// 이벤트 구독(줄마다 JSON 하나)
    Events {
        /// 이벤트 종류(여러 번 가능). 없으면 pane.output을 뺀 전부
        #[arg(long = "kind")]
        kinds: Vec<String>,
    },
    /// 감사 기록(F2)
    #[command(subcommand)]
    Audit(AuditCmd),
    /// 정책(F1·F4·F5)
    #[command(subcommand)]
    Policy(PolicyCmd),
    /// 에이전트 훅 통합(F5)
    #[command(subcommand)]
    Integration(IntegrationCmd),
    /// 플러그인
    #[command(subcommand)]
    Plugin(PluginCmd),
    /// 원격 머신 프로필
    #[command(subcommand)]
    Machine(MachineCmd),
    /// 로컬 웹 관제 콘솔
    Console {
        #[arg(long, default_value_t = 0)]
        port: u16,
        /// 승인·거부 버튼 허용(기본은 읽기 전용)
        #[arg(long)]
        allow_actions: bool,
        /// 브라우저를 열지 않는다
        #[arg(long)]
        no_open: bool,
    },
    /// 관제 모드로 붙는다(`kmttt --control`과 같음)
    Control,
    /// OpenRPC 문서 출력
    Schema,
    /// 셸 자동완성 스크립트(bash, zsh, fish, powershell, elvish)
    Completion { shell: clap_complete::Shell },
    /// 새 버전 확인·설치(서명 검증 후에만 교체)
    Update {
        #[arg(long)]
        check: bool,
    },
    /// 서버를 멈추고 통합·설정을 지운다
    Uninstall {
        #[arg(long)]
        yes: bool,
        /// 감사 기록까지 지운다(기본은 남김)
        #[arg(long)]
        purge_ledger: bool,
    },
    /// (내부) 에이전트 훅에서 호출
    #[command(hide = true)]
    Hook {
        agent: String,
        event: String,
        /// Codex notify처럼 JSON을 인자로 주는 경우
        payload: Option<String>,
    },
    /// (내부) Claude Code 상태 줄: 입력 JSON에서 요금제 사용량만 저장하고 한 줄을 출력
    #[command(hide = true)]
    Statusline,
    /// Claude Code·Codex 요금제 사용량(남은 % · 리셋 시각)
    Usage,
    /// (내부) restricted 격리 실행
    #[command(hide = true)]
    SandboxExec(SandboxExecArgs),
    /// (개발용) 단일 pane을 서버 없이 띄운다
    #[command(hide = true)]
    DebugPane {
        #[arg(trailing_var_arg = true)]
        command: Vec<String>,
    },
}

#[derive(Args, Debug)]
pub struct SandboxExecArgs {
    #[arg(long)]
    pub root: PathBuf,
    #[arg(long, default_value_t = 8192)]
    pub memory_mb: u64,
    #[arg(long, default_value_t = 512)]
    pub processes: u32,
    #[arg(long)]
    pub home_writable: Vec<String>,
    /// TCP 연결을 이 포트(이그레스 프록시)로만 허용
    #[arg(long)]
    pub egress_port: Option<u16>,
    #[arg(long)]
    pub allow_tcp_port: Vec<u16>,
    #[arg(trailing_var_arg = true, required = true)]
    pub command: Vec<String>,
}

#[derive(Subcommand, Debug)]
pub enum ServerCmd {
    /// 서버를 이 프로세스에서 실행(보통 자동으로 뜬다)
    Run,
    /// 서버와 모든 pane 종료
    Stop,
    /// config.toml·policy.toml·매니페스트 다시 읽기
    ReloadConfig,
    /// 표준 입출력을 로컬 서버 소켓에 잇는다(SSH 원격 접속용)
    Stdio,
    /// 상태 감지 매니페스트 목록
    AgentManifests,
}

#[derive(Subcommand, Debug)]
pub enum SessionCmd {
    /// 이름 있는 세션에 붙는다
    Attach { name: String },
    /// 세션 서버를 멈춘다
    Stop { name: String },
}

#[derive(Subcommand, Debug)]
pub enum WorkspaceCmd {
    List,
    Create {
        #[arg(long)]
        cwd: Option<String>,
        #[arg(long)]
        label: Option<String>,
        #[arg(long)]
        no_focus: bool,
        /// none, restricted, container, wsl
        #[arg(long)]
        profile: Option<String>,
    },
    Rename {
        id: String,
        label: String,
    },
    Focus {
        id: String,
    },
    Close {
        id: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum TabCmd {
    List {
        #[arg(long)]
        workspace: Option<String>,
    },
    Create {
        #[arg(long)]
        workspace: Option<String>,
        #[arg(long)]
        label: Option<String>,
        #[arg(long)]
        cwd: Option<String>,
        #[arg(long)]
        no_focus: bool,
    },
    Rename {
        id: String,
        label: String,
    },
    Focus {
        id: String,
    },
    Close {
        id: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum PaneCmd {
    List,
    /// pane 나누기(새 pane ID를 돌려줌)
    Split {
        /// 나눌 pane(기본: 초점 pane)
        target: Option<String>,
        #[arg(long, default_value = "right")]
        direction: String,
        #[arg(long)]
        cwd: Option<String>,
        /// 새 셸에 바로 입력할 명령
        #[arg(long)]
        command: Option<String>,
        #[arg(long)]
        profile: Option<String>,
        #[arg(long)]
        no_focus: bool,
    },
    /// 명령을 입력하고 Enter
    Run {
        pane: String,
        command: String,
    },
    /// 원문 텍스트 보내기
    Send {
        pane: String,
        text: String,
        #[arg(long)]
        enter: bool,
    },
    /// pane 텍스트 읽기(항상 마스킹)
    Read {
        pane: String,
        /// recent(줄 단위 기록) 또는 screen(현재 화면)
        #[arg(long, default_value = "recent")]
        source: String,
        #[arg(long, default_value_t = 50)]
        lines: u64,
    },
    /// 정규식이 나올 때까지 기다리기
    WaitOutput {
        pane: String,
        #[arg(long)]
        regex: String,
        #[arg(long, default_value_t = 30_000)]
        timeout: u64,
    },
    /// 전면 프로세스
    ProcessInfo {
        pane: Option<String>,
    },
    Focus {
        pane: String,
    },
    Zoom {
        pane: Option<String>,
    },
    Close {
        pane: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum AgentCmd {
    List,
    /// 셸만 떠 있는 pane에서 에이전트 시작
    Start {
        name: String,
        #[arg(long)]
        kind: String,
        #[arg(long)]
        pane: Option<String>,
    },
    /// 프롬프트 보내기
    Prompt {
        target: String,
        text: String,
        /// 에이전트가 멈출 때까지 기다린다
        #[arg(long)]
        wait: bool,
        #[arg(long, default_value_t = 600_000)]
        timeout: u64,
        /// 에이전트가 질문을 띄운 상태(blocked)여도 보낸다(입력이 그 질문의 답이 될 수 있다)
        #[arg(long)]
        force: bool,
    },
    Read {
        target: String,
        #[arg(long, default_value_t = 50)]
        lines: u64,
    },
    /// 상태가 될 때까지 기다리기
    Wait {
        target: String,
        /// working, blocked, done, idle (쉼표로 여러 개)
        #[arg(long, default_value = "blocked,idle,done")]
        until: String,
        #[arg(long, default_value_t = 600_000)]
        timeout: u64,
    },
    /// 상태 감지 근거
    Explain {
        target: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum WorktreeCmd {
    Create {
        #[arg(long)]
        cwd: Option<String>,
        #[arg(long)]
        branch: String,
        #[arg(long)]
        base: Option<String>,
        #[arg(long)]
        label: Option<String>,
        #[arg(long)]
        path: Option<String>,
        #[arg(long)]
        no_focus: bool,
    },
    Remove {
        #[arg(long)]
        workspace: String,
        #[arg(long)]
        force: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum ApprovalCmd {
    List,
    Allow {
        id: u64,
        /// 같은 호출자·동작·대상은 창이 닫히거나 서버가 꺼질 때까지 묻지 않고 허용
        #[arg(long = "for-session")]
        for_session: bool,
    },
    Deny {
        id: u64,
    },
    /// 이 세션 동안 허용한 목록
    Grants,
    /// 세션 동안 허용을 하나(--key) 또는 전부 해제
    Revoke {
        #[arg(long)]
        key: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum AuditCmd {
    /// 해시 체인·서명 검증(서버 없이 파일만으로)
    Verify {
        #[arg(long)]
        dir: Option<PathBuf>,
    },
    /// 검증 결과 + 모든 기록을 JSON으로(Securidge Trace 연동)
    Export {
        #[arg(long)]
        dir: Option<PathBuf>,
        #[arg(long, short = 'o')]
        output: Option<PathBuf>,
        /// 암호화된 본문을 풀어서
        #[arg(long)]
        decrypt: bool,
    },
    /// 서버가 보는 무결성 상태
    Status,
    /// 기록 폴더 경로
    Path,
}

#[derive(Subcommand, Debug)]
pub enum PolicyCmd {
    /// 적용 중인 정책
    Show,
    /// policy.toml 경로
    Path,
    /// 주석이 달린 기본 policy.toml을 만든다(이미 있으면 그대로)
    Init,
    /// policy.toml 문법 검사
    Check,
}

#[derive(Subcommand, Debug)]
pub enum IntegrationCmd {
    /// claude(훅), codex(notify), claude-usage(사용량 표시용 상태 줄)
    Install {
        agent: String,
        /// codex/claude-usage: 기존 notify/상태 줄을 보관하고 교체한다(해제 시 복원)
        #[arg(long, conflicts_with = "chain")]
        replace: bool,
        /// codex: 기존 notify를 KMttt 보고 뒤에 실행한다(3초 제한, 해제 시 원래 설정 복원)
        #[arg(long, conflicts_with = "replace")]
        chain: bool,
    },
    Uninstall {
        agent: String,
        /// 이 kmttt 실행 파일이 등록한 것만 지운다(설치 프로그램의 제거 단계에서 씀)
        #[arg(long)]
        if_this_exe: bool,
    },
    Status {
        agent: Option<String>,
    },
    /// 연동된 에이전트(claude, codex)의 KMttt 스킬을 이 버전으로 갱신
    SyncSkills,
}

#[derive(Subcommand, Debug)]
pub enum PluginCmd {
    /// 폴더 또는 git 주소(owner/repo/plugin-name은 GitHub에서)
    Install {
        source: String,
        #[arg(long)]
        yes: bool,
    },
    List,
    Remove {
        name: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum MachineCmd {
    Add {
        target: String,
        #[arg(long)]
        label: Option<String>,
        #[arg(long, default_value = "default")]
        remote_session: String,
        #[arg(long, default_value = "kmttt")]
        bin: String,
        /// 연결 확인을 건너뛴다
        #[arg(long)]
        no_check: bool,
    },
    List,
    Rename {
        id: String,
        #[arg(long)]
        label: String,
    },
    Enable {
        id: String,
    },
    Disable {
        id: String,
    },
    Remove {
        id: String,
    },
}
