//! 일반 화면 그리기: 사이드바, 탭 줄, pane들, 상태 줄, 도움말·워크스페이스 선택 창.

use crate::App;
use kmttt_core::i18n::t;
use kmttt_core::layout::{self, Rect as LRect};
use kmttt_proto::*;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

pub fn state_color(s: AgentState) -> Color {
    match s {
        AgentState::Working => Color::Cyan,
        AgentState::Blocked => Color::Yellow,
        AgentState::Done => Color::Green,
        AgentState::Idle => Color::Gray,
        AgentState::Unknown => Color::DarkGray,
    }
}

pub fn sev_color(s: Severity) -> Color {
    match s {
        Severity::Info => Color::Gray,
        Severity::Notice => Color::Yellow,
        Severity::Critical => Color::Red,
    }
}

fn vt_color(c: vt100::Color) -> Color {
    match c {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(i) => Color::Indexed(i),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

/// 화면 영역 나누기: (사이드바, 탭 줄, pane 영역, 상태 줄)
pub fn areas(app: &App, full: Rect) -> (Rect, Rect, Rect, Rect) {
    let sw = app.sidebar_width.min(full.width / 2);
    let sidebar = Rect {
        x: 0,
        y: 0,
        width: sw,
        height: full.height.saturating_sub(1),
    };
    let mx = if sw > 0 { sw + 1 } else { 0 };
    let mw = full.width.saturating_sub(mx);
    let tabbar = Rect {
        x: mx,
        y: 0,
        width: mw,
        height: 1,
    };
    let main = Rect {
        x: mx,
        y: 1,
        width: mw,
        height: full.height.saturating_sub(2),
    };
    let status = Rect {
        x: 0,
        y: full.height.saturating_sub(1),
        width: full.width,
        height: 1,
    };
    (sidebar, tabbar, main, status)
}

pub fn draw(f: &mut Frame, app: &App) {
    let full = f.area();
    let (sidebar, tabbar, main, status) = areas(app, full);
    draw_sidebar(f, app, sidebar);
    if sidebar.width > 0 {
        let buf = f.buffer_mut();
        for y in 0..sidebar.height {
            if let Some(c) = buf.cell_mut((sidebar.width, y)) {
                c.set_symbol("│")
                    .set_style(Style::default().fg(Color::DarkGray));
            }
        }
    }
    draw_tabbar(f, app, tabbar);
    draw_panes(f, app, main);
    draw_status(f, app, status);
    match app.overlay {
        crate::Overlay::Help => draw_help(f, app, full),
        crate::Overlay::Workspaces(sel) => draw_workspaces(f, app, full, sel),
        crate::Overlay::None => {}
    }
}

fn draw_sidebar(f: &mut Frame, app: &App, area: Rect) {
    if area.width < 4 {
        return;
    }
    let lang = &app.lang;
    let snap = &app.snap;
    let mut lines: Vec<Line> = vec![
        Line::from(Span::styled(
            " Securidge KMttt",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            format!(
                " {}{}",
                app.session_label,
                if app.mask_screen { " · MASK" } else { "" }
            ),
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(""),
        Line::from(Span::styled(
            format!(" {}", t(lang, "sidebar.workspaces")),
            Style::default().add_modifier(Modifier::BOLD),
        )),
    ];
    for w in &snap.workspaces {
        let focused = w.id == snap.focus_workspace;
        // 워크스페이스 안 에이전트 상태 요약(가장 급한 것)
        let worst = snap
            .panes
            .iter()
            .filter(|p| p.workspace == w.id)
            .filter_map(|p| p.agent.as_ref().map(|a| a.state))
            .min_by_key(|s| s.priority());
        let mut spans = vec![Span::styled(
            if focused { " ▸ " } else { "   " },
            Style::default().fg(Color::Cyan),
        )];
        if let Some(s) = worst {
            spans.push(Span::styled(
                format!("{} ", s.icon()),
                Style::default().fg(state_color(s)),
            ));
        }
        let name = if w.worktree.is_some() {
            format!("⎇ {}", w.label)
        } else {
            w.label.clone()
        };
        spans.push(Span::styled(
            trunc(&name, area.width.saturating_sub(6) as usize),
            if focused {
                Style::default().add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            },
        ));
        lines.push(Line::from(spans));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!(" {}", t(lang, "sidebar.agents")),
        Style::default().add_modifier(Modifier::BOLD),
    )));
    let mut agents: Vec<&PaneInfo> = snap.panes.iter().filter(|p| p.agent.is_some()).collect();
    agents.sort_by_key(|p| p.agent.as_ref().map(|a| a.state.priority()).unwrap_or(9));
    if agents.is_empty() {
        lines.push(Line::from(Span::styled(
            format!("   {}", t(lang, "sidebar.no_agents")),
            Style::default().fg(Color::DarkGray),
        )));
    }
    for p in agents {
        let a = p.agent.as_ref().expect("filtered");
        let name = a.name.clone().unwrap_or_else(|| a.kind.clone());
        let mut spans = vec![
            Span::styled(
                format!(" {} ", a.state.icon()),
                Style::default().fg(state_color(a.state)),
            ),
            Span::raw(trunc(&name, 10)),
            Span::styled(format!(" {}", p.id), Style::default().fg(Color::DarkGray)),
        ];
        // 보안 배지: 격리 강도, 차단 수, 훅 여부
        let badge = match p.sandbox_strength.as_str() {
            "high" => "▣",
            "medium" => "▤",
            "low" => "▥",
            _ => "",
        };
        if !badge.is_empty() {
            spans.push(Span::styled(
                format!(" {badge}"),
                Style::default().fg(Color::Green),
            ));
        }
        if p.blocked_calls > 0 {
            spans.push(Span::styled(
                format!(" ⊘{}", p.blocked_calls),
                Style::default().fg(Color::Red),
            ));
        }
        if !a.hooked {
            spans.push(Span::styled(" ◌", Style::default().fg(Color::DarkGray)));
        }
        lines.push(Line::from(spans));
        lines.push(Line::from(Span::styled(
            format!("    {}", t(lang, &format!("state.{}", a.state.as_str()))),
            Style::default().fg(state_color(a.state)),
        )));
    }
    if !app.pending_approvals.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!(
                " ⚑ {} {}",
                app.pending_approvals.len(),
                t(lang, "control.summary_pending")
            ),
            Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )));
    }
    let body = Rect {
        height: area.height.saturating_sub(1),
        ..area
    };
    f.render_widget(Paragraph::new(lines), body);
    // 왼쪽 아래 제작자 표시
    let credit = Rect {
        y: area.y + area.height.saturating_sub(1),
        height: 1,
        ..area
    };
    f.render_widget(
        Paragraph::new(Span::styled(
            format!(" {}", t(lang, "credit")),
            Style::default().fg(Color::DarkGray),
        )),
        credit,
    );
}

fn trunc(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut o: String = s.chars().take(n.saturating_sub(1)).collect();
        o.push('…');
        o
    }
}

fn draw_tabbar(f: &mut Frame, app: &App, area: Rect) {
    let snap = &app.snap;
    let mut spans = Vec::new();
    if let Some(ws) = snap.workspace(&snap.focus_workspace) {
        spans.push(Span::styled(
            format!(" {} ", ws.label),
            Style::default().fg(Color::Black).bg(Color::Cyan),
        ));
        for (i, tab) in ws.tabs.iter().enumerate() {
            let focused = tab.id == ws.focus_tab;
            let st = if focused {
                Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)
            } else {
                Style::default().fg(Color::Gray)
            };
            spans.push(Span::raw(" "));
            spans.push(Span::styled(
                format!(
                    " {}:{}{} ",
                    i + 1,
                    tab.label,
                    if tab.zoomed { " ⤢" } else { "" }
                ),
                st,
            ));
        }
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_panes(f: &mut Frame, app: &App, area: Rect) {
    let Some(tab) = app.snap.focused_tab() else {
        f.render_widget(
            Paragraph::new("  no workspace — prefix + shift+n to create one"),
            area,
        );
        return;
    };
    let la = LRect {
        x: area.x,
        y: area.y,
        w: area.width,
        h: area.height,
    };
    let rects: Vec<(String, LRect)> = if tab.zoomed {
        vec![(tab.focus.clone(), la)]
    } else {
        layout::compute(&tab.layout, la)
    };
    if !tab.zoomed {
        let buf = f.buffer_mut();
        for (x, y, len, vertical) in layout::dividers(&tab.layout, la) {
            for i in 0..len {
                let (cx, cy) = if vertical { (x, y + i) } else { (x + i, y) };
                if let Some(c) = buf.cell_mut((cx, cy)) {
                    c.set_symbol(if vertical { "│" } else { "─" })
                        .set_style(Style::default().fg(Color::DarkGray));
                }
            }
        }
    }
    for (id, r) in &rects {
        let rect = Rect {
            x: r.x,
            y: r.y,
            width: r.w,
            height: r.h,
        };
        let focused = *id == tab.focus;
        if let Some(parser) = app.parsers.get(id) {
            render_screen(
                f.buffer_mut(),
                parser.screen(),
                rect,
                app.mask_screen.then_some(&app.redactor),
            );
            if focused && !parser.screen().hide_cursor() {
                let (cy, cx) = parser.screen().cursor_position();
                if cx < rect.width && cy < rect.height {
                    f.set_cursor_position((rect.x + cx, rect.y + cy));
                }
            }
        }
        // 초점 pane 표시: 오른쪽 위 작은 표식과 상태
        if rects.len() > 1 || tab.zoomed {
            if let Some(p) = app.snap.pane(id) {
                let mut label = p.id.clone();
                if let Some(a) = &p.agent {
                    label = format!(
                        "{} {} {}",
                        a.state.icon(),
                        a.name.clone().unwrap_or_else(|| a.kind.clone()),
                        p.id
                    );
                }
                if p.sandbox != "none" {
                    label.push_str(&format!(" [{}]", p.sandbox));
                }
                let w = (label.chars().count() as u16 + 2).min(rect.width);
                let lx = rect.x + rect.width.saturating_sub(w);
                let style = if focused {
                    Style::default().fg(Color::Black).bg(Color::Cyan)
                } else {
                    Style::default().fg(Color::DarkGray)
                };
                f.render_widget(
                    Paragraph::new(Span::styled(format!(" {label} "), style)),
                    Rect {
                        x: lx,
                        y: rect.y,
                        width: w,
                        height: 1,
                    },
                );
            }
        }
    }
}

/// vt100 화면을 버퍼에 옮긴다. `mask`가 있으면 각 줄에서 찾은 비밀을 '*'로 가린다.
pub fn render_screen(
    buf: &mut Buffer,
    screen: &vt100::Screen,
    area: Rect,
    mask: Option<&kmttt_redact::Redactor>,
) {
    let (rows, cols) = screen.size();
    let (rows, cols) = (rows.get(), cols.get());
    for row in 0..rows.min(area.height) {
        let mut masked_cols: Vec<bool> = Vec::new();
        if let Some(r) = mask {
            // 줄 텍스트와 열 위치 대응표를 만들어 탐지 구간을 열로 옮긴다.
            let mut text = String::new();
            let mut col_of_byte: Vec<u16> = Vec::new();
            for col in 0..cols {
                if let Some(c) = screen.cell(row, col) {
                    if c.is_wide_continuation() {
                        continue;
                    }
                    let s = if c.has_contents() { c.contents() } else { " " };
                    for _ in 0..s.len() {
                        col_of_byte.push(col);
                    }
                    text.push_str(s);
                }
            }
            masked_cols = vec![false; cols as usize];
            for fnd in r.find(&text) {
                for b in fnd.start..fnd.end.min(col_of_byte.len()) {
                    masked_cols[col_of_byte[b] as usize] = true;
                }
            }
        }
        for col in 0..cols.min(area.width) {
            let Some(c) = screen.cell(row, col) else {
                continue;
            };
            if c.is_wide_continuation() {
                continue;
            }
            let Some(out) = buf.cell_mut((area.x + col, area.y + row)) else {
                continue;
            };
            let masked = masked_cols.get(col as usize).copied().unwrap_or(false);
            let sym = if masked {
                "*"
            } else if c.has_contents() {
                c.contents()
            } else {
                " "
            };
            let mut style = Style::default()
                .fg(vt_color(c.fgcolor()))
                .bg(vt_color(c.bgcolor()));
            let mut m = Modifier::empty();
            if c.bold() {
                m |= Modifier::BOLD;
            }
            if c.dim() {
                m |= Modifier::DIM;
            }
            if c.italic() {
                m |= Modifier::ITALIC;
            }
            if c.underline() {
                m |= Modifier::UNDERLINED;
            }
            if c.inverse() {
                m |= Modifier::REVERSED;
            }
            style = style.add_modifier(m);
            out.set_symbol(sym).set_style(style);
            if c.is_wide() && !masked {
                // 넓은 문자 뒤 칸은 비워 둔다(겹침 방지).
                if let Some(next) = buf.cell_mut((area.x + col + 1, area.y + row)) {
                    next.set_symbol("");
                }
            }
        }
    }
}

fn draw_status(f: &mut Frame, app: &App, area: Rect) {
    let lang = &app.lang;
    let mut spans = Vec::new();
    if app.prefix_pending {
        spans.push(Span::styled(
            format!(" {} ", t(lang, "status.prefix")),
            Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ));
    }
    if let Some(p) = app.snap.focused_tab().and_then(|t| app.snap.pane(&t.focus)) {
        spans.push(Span::styled(
            format!(" {} ", p.id),
            Style::default().fg(Color::Black).bg(Color::Gray),
        ));
        if p.sandbox != "none" {
            spans.push(Span::styled(
                format!(" {}:{} ", p.sandbox, p.sandbox_strength),
                Style::default().fg(Color::Green),
            ));
        }
    }
    if let Some((msg, level, _)) = &app.toast {
        let color = match level.as_str() {
            "critical" => Color::Red,
            "notice" => Color::Yellow,
            _ => Color::Cyan,
        };
        spans.push(Span::styled(format!(" {msg} "), Style::default().fg(color)));
    } else {
        spans.push(Span::styled(
            format!(" {}", t(lang, "status.detach_hint")),
            Style::default().fg(Color::DarkGray),
        ));
    }
    f.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(Color::Black)),
        area,
    );
}

fn centered(full: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(full.width);
    let h = h.min(full.height);
    Rect {
        x: full.x + (full.width - w) / 2,
        y: full.y + (full.height - h) / 2,
        width: w,
        height: h,
    }
}

fn draw_help(f: &mut Frame, app: &App, full: Rect) {
    let rows = app.help_rows.len() as u16 + 2;
    let area = centered(full, 56, rows + 2);
    f.render_widget(Clear, area);
    let lines: Vec<Line> = app
        .help_rows
        .iter()
        .map(|(a, k)| {
            Line::from(vec![
                Span::styled(format!(" {k:<22}"), Style::default().fg(Color::Cyan)),
                Span::raw(a.clone()),
            ])
        })
        .collect();
    f.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" {} — Esc ", t(&app.lang, "help.title"))),
        ),
        area,
    );
}

fn draw_workspaces(f: &mut Frame, app: &App, full: Rect, sel: usize) {
    let n = app.snap.workspaces.len() as u16;
    let area = centered(full, 50, n + 4);
    f.render_widget(Clear, area);
    let mut lines: Vec<Line> = app
        .snap
        .workspaces
        .iter()
        .enumerate()
        .map(|(i, w)| {
            let st = if i == sel {
                Style::default().add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            };
            Line::from(Span::styled(
                format!(" {} {}  ({} tabs) ", w.id, w.label, w.tabs.len()),
                st,
            ))
        })
        .collect();
    lines.push(Line::from(Span::styled(
        " Enter focus · n new · x close · Esc",
        Style::default().fg(Color::DarkGray),
    )));
    f.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" {} ", t(&app.lang, "sidebar.workspaces"))),
        ),
        area,
    );
}
