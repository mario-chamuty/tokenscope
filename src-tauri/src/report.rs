use crate::db::{build_time_filter, Database};
use crate::settings::{build_cost_case, build_cost_expr, PricingConfig};
use std::path::PathBuf;

/// Anthropic's rough ~4 chars-per-token heuristic, mirrored from the
/// frontend so the markdown/PDF estimates line up with the Advanced tab.
fn est_tokens(chars: i64) -> i64 {
    (chars as f64 / 4.0).round() as i64
}

fn pct(part: i64, whole: i64) -> String {
    if whole <= 0 {
        return "0%".into();
    }
    format!("{:.1}%", (part as f64 / whole as f64) * 100.0)
}

/// Generate markdown report. Now includes the full Advanced breakdown
/// (Output/Input composition, interaction counters, tool/MCP/skill/
/// subagent/denied tables) in addition to the headline totals.
pub fn generate_markdown(
    db: &Database,
    output_dir: &PathBuf,
    since: Option<&str>,
    pricing: &PricingConfig,
) -> Result<PathBuf, String> {
    std::fs::create_dir_all(output_dir).map_err(|e| e.to_string())?;
    let report_path = output_dir.join("token_report.md");

    let adv = db.get_advanced_stats(since, &[], None, pricing)?;

    let conn = db.read_connection()?;
    let tf = build_time_filter(since, "t")?;
    let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let range_label = since.unwrap_or("All time");

    let mut out = Vec::new();
    out.push(format!(
        "# TokenScope Token Usage Report\n\nGenerated: {} | Range: {}\n",
        now, range_label
    ));

    // Grand totals
    let (sessions, turns, inp, cc, cr, outp) = query_totals(&conn, &tf)?;
    let total = inp + cc + cr + outp;
    out.push("## Grand Totals\n".into());
    out.push(format!(
        "- **Sessions**: {}\n- **Turns**: {}\n- **Total tokens**: {}",
        fnum(sessions),
        fnum(turns),
        fnum(total)
    ));
    out.push(format!(
        "  - Input: {}\n  - Cache creation: {}\n  - Cache read: {}\n  - Output: {}\n",
        fnum(inp),
        fnum(cc),
        fnum(cr),
        fnum(outp)
    ));

    // ── Advanced: Output composition ────────────────────────────────
    let text_est = est_tokens(adv.text_chars);
    let tool_est = est_tokens(adv.tool_input_chars);
    let think_est = (adv.total_output_tokens - text_est - tool_est).max(0);
    out.push("## Output Composition\n".into());
    out.push(format!(
        "Total output tokens: **{}** across **{}** turns ({} with thinking, {} with tool use).\n",
        fnum(adv.total_output_tokens),
        fnum(adv.total_turns),
        fnum(adv.turns_with_thinking),
        fnum(adv.turns_with_tools)
    ));
    out.push("| Component | Est. tokens | Share |".into());
    out.push("|-----------|-------------|-------|".into());
    out.push(format!(
        "| Text | {} | {} |",
        fnum(text_est),
        pct(text_est, adv.total_output_tokens)
    ));
    out.push(format!(
        "| Tool use | {} | {} |",
        fnum(tool_est),
        pct(tool_est, adv.total_output_tokens)
    ));
    out.push(format!(
        "| Thinking\\* | {} | {} |",
        fnum(think_est),
        pct(think_est, adv.total_output_tokens)
    ));
    out.push("\n\\* Thinking is a residual estimate: Claude Code logs encrypted signatures but not plaintext, so we count turns that *contain* thinking blocks and infer token share from `output_total − text − tool_input`.\n".into());

    // ── Input & Cache composition ───────────────────────────────────
    let input_total = adv.total_input_tokens + adv.total_cache_read + adv.total_cache_creation;
    out.push("## Input & Cache Composition\n".into());
    out.push(format!(
        "Total input-side tokens: **{}**.\n",
        fnum(input_total)
    ));
    out.push("| Component | Tokens | Share |".into());
    out.push("|-----------|--------|-------|".into());
    out.push(format!(
        "| Fresh input | {} | {} |",
        fnum(adv.total_input_tokens),
        pct(adv.total_input_tokens, input_total)
    ));
    out.push(format!(
        "| Cache write | {} | {} |",
        fnum(adv.total_cache_creation),
        pct(adv.total_cache_creation, input_total)
    ));
    out.push(format!(
        "| Cache read | {} | {} |\n",
        fnum(adv.total_cache_read),
        pct(adv.total_cache_read, input_total)
    ));

    // ── Interaction counters ────────────────────────────────────────
    out.push("## Interactions\n".into());
    out.push(format!(
        "- **Questions asked** (AskUserQuestion): {}",
        fnum(adv.ask_user_count)
    ));
    out.push(format!(
        "- **Plan mode invocations** (ExitPlanMode): {}",
        fnum(adv.plan_mode_count)
    ));
    out.push(format!(
        "- **Denied tool calls** (rejected by auto-mode): {}",
        fnum(adv.denied_count)
    ));
    out.push(format!(
        "- **Subagents spawned**: {} (turns: {}, cost: ${:.2})\n",
        fnum(adv.subagent_stats.spawn_count),
        fnum(adv.subagent_stats.subagent_turns),
        adv.subagent_stats.subagent_cost
    ));

    // ── Denied breakdown ────────────────────────────────────────────
    if !adv.denied_breakdown.is_empty() {
        out.push("## Denied Tool Calls\n".into());
        out.push("| Tool | Category | Denials |".into());
        out.push("|------|----------|---------|".into());
        for d in &adv.denied_breakdown {
            out.push(format!(
                "| {} | {} | {} |",
                d.tool_name,
                d.category,
                fnum(d.call_count)
            ));
        }
        out.push(String::new());
    }

    // ── Category totals ─────────────────────────────────────────────
    if !adv.category_totals.is_empty() {
        out.push("## Category Totals\n".into());
        out.push("| Category | Calls | Turns | Input chars |".into());
        out.push("|----------|-------|-------|-------------|".into());
        for c in &adv.category_totals {
            out.push(format!(
                "| {} | {} | {} | {} |",
                c.category,
                fnum(c.call_count),
                fnum(c.turn_count),
                fnum(c.input_chars)
            ));
        }
        out.push(String::new());
    }

    // ── Top tools ───────────────────────────────────────────────────
    if !adv.tool_breakdown.is_empty() {
        out.push("## Top Tools\n".into());
        out.push("| Tool | Category | Calls | Input chars |".into());
        out.push("|------|----------|-------|-------------|".into());
        for t in adv.tool_breakdown.iter().take(30) {
            out.push(format!(
                "| {} | {} | {} | {} |",
                t.tool_name,
                t.category,
                fnum(t.call_count),
                fnum(t.input_chars)
            ));
        }
        out.push(String::new());
    }

    // ── MCP servers ─────────────────────────────────────────────────
    if !adv.mcp_breakdown.is_empty() {
        out.push("## MCP Servers\n".into());
        out.push("| Server | Distinct tools | Calls | Input chars |".into());
        out.push("|--------|----------------|-------|-------------|".into());
        for m in &adv.mcp_breakdown {
            out.push(format!(
                "| {} | {} | {} | {} |",
                m.server,
                fnum(m.tool_count),
                fnum(m.call_count),
                fnum(m.input_chars)
            ));
        }
        out.push(String::new());
    }

    // ── Skills ──────────────────────────────────────────────────────
    if !adv.skill_breakdown.is_empty() {
        out.push("## Skills Invoked\n".into());
        out.push("| Skill | Calls |".into());
        out.push("|-------|-------|".into());
        for s in &adv.skill_breakdown {
            out.push(format!("| {} | {} |", s.skill_name, fnum(s.call_count)));
        }
        out.push(String::new());
    }

    // ── Subagent types ──────────────────────────────────────────────
    if !adv.subagent_types.is_empty() {
        out.push("## Subagent Types\n".into());
        out.push("| Subagent | Spawns |".into());
        out.push("|----------|--------|".into());
        for s in &adv.subagent_types {
            out.push(format!("| {} | {} |", s.subagent_type, fnum(s.call_count)));
        }
        out.push(String::new());
    }

    // ── Top subjects (bash / files / domains) ───────────────────────
    if !adv.top_bash.is_empty() {
        out.push("## Top Bash Commands\n".into());
        out.push("| Command | Calls |".into());
        out.push("|---------|-------|".into());
        for s in adv.top_bash.iter().take(20) {
            out.push(format!(
                "| `{}` | {} |",
                md_escape(&s.subject),
                fnum(s.call_count)
            ));
        }
        out.push(String::new());
    }
    if !adv.top_files.is_empty() {
        out.push("## Top Files Touched\n".into());
        out.push("| File | Accesses |".into());
        out.push("|------|----------|".into());
        for s in adv.top_files.iter().take(20) {
            out.push(format!(
                "| `{}` | {} |",
                md_escape(&s.subject),
                fnum(s.call_count)
            ));
        }
        out.push(String::new());
    }
    if !adv.top_domains.is_empty() {
        out.push("## Top Web Domains\n".into());
        out.push("| Domain | Hits |".into());
        out.push("|--------|------|".into());
        for s in adv.top_domains.iter().take(20) {
            out.push(format!(
                "| {} | {} |",
                md_escape(&s.subject),
                fnum(s.call_count)
            ));
        }
        out.push(String::new());
    }

    // ── By model ────────────────────────────────────────────────────
    out.push("## By Model\n".into());
    out.push("| Model | Turns | Input | Cache Create | Cache Read | Output | Total |".into());
    out.push("|-------|-------|-------|--------------|------------|--------|-------|".into());
    for (model, turns, inp, cc, cr, outp) in query_by_model(&conn, &tf)? {
        let t = inp + cc + cr + outp;
        out.push(format!(
            "| {} | {} | {} | {} | {} | {} | {} |",
            model,
            fnum(turns),
            fnum(inp),
            fnum(cc),
            fnum(cr),
            fnum(outp),
            fnum(t)
        ));
    }
    out.push(String::new());

    // ── By project ──────────────────────────────────────────────────
    out.push("## By Project\n".into());
    out.push(
        "| Project | Sessions | Turns | Input | Output | Cache Read | Cache Create | Total |"
            .into(),
    );
    out.push(
        "|---------|----------|-------|-------|--------|------------|--------------|-------|"
            .into(),
    );
    for (proj, sess, turns, inp, outp, cr, cc) in query_by_project(&conn, &tf)? {
        let t = inp + outp + cr + cc;
        out.push(format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} |",
            proj,
            sess,
            fnum(turns),
            fnum(inp),
            fnum(outp),
            fnum(cr),
            fnum(cc),
            fnum(t)
        ));
    }
    out.push(String::new());

    // ── Top sessions ────────────────────────────────────────────────
    out.push("## Top 25 Costliest Sessions\n".into());
    for (i, (sid, proj, model, start, end, turns, inp, outp, cr, cc, cost)) in
        query_top_sessions(&conn, &tf, pricing)?.iter().enumerate()
    {
        let total = inp + outp + cr + cc;
        let s = if start.len() >= 19 {
            &start[..19]
        } else {
            start.as_str()
        };
        let e = if end.len() >= 19 {
            &end[..19]
        } else {
            end.as_str()
        };
        out.push(format!(
            "### {}. {} – ${:.2} ({} tokens)",
            i + 1,
            proj,
            cost,
            fnum(total)
        ));
        out.push(format!(
            "- Session: `{}`\n- Model: {}\n- Period: {} to {}\n- Turns: {}",
            sid, model, s, e, turns
        ));
        out.push(format!(
            "- Tokens: input={}, output={}, cache_read={}, cache_create={}\n",
            fnum(*inp),
            fnum(*outp),
            fnum(*cr),
            fnum(*cc)
        ));
    }

    std::fs::write(&report_path, out.join("\n")).map_err(|e| e.to_string())?;
    Ok(report_path)
}

/// Removes the throw-away browser profile on every exit path.
struct TempProfile(PathBuf);

impl Drop for TempProfile {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

const PDF_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Runs the browser with its own profile directory: without one, Edge/Chrome
/// hand the job to an already running instance and exit without printing.
fn run_print_to_pdf(
    browser: &std::path::Path,
    headless_flag: &str,
    profile: &std::path::Path,
    pdf_path: &std::path::Path,
    file_url: &str,
) -> Result<std::process::ExitStatus, String> {
    let name = browser.to_string_lossy();
    let mut child = std::process::Command::new(browser)
        .args([
            headless_flag,
            "--disable-gpu",
            "--no-first-run",
            "--no-pdf-header-footer",
            &format!("--user-data-dir={}", profile.to_string_lossy()),
            &format!("--print-to-pdf={}", pdf_path.to_string_lossy()),
            file_url,
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("Failed to launch {name}: {e}"))?;
    let started = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) if started.elapsed() > PDF_TIMEOUT => {
                child.kill().ok();
                child.wait().ok();
                return Err(format!("{name} did not finish rendering the PDF in time"));
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(100)),
            Err(e) => return Err(format!("Failed to wait for {name}: {e}")),
        }
    }
}

/// Generate a real PDF file. Builds HTML with all advanced-stats sections,
/// then renders it with a headless browser (Edge first, Chrome second) via
/// `--print-to-pdf`. Without a browser this fails; the HTML stays in the
/// reports folder.
pub fn generate_pdf(
    db: &Database,
    output_dir: &PathBuf,
    since: Option<&str>,
    pricing: &PricingConfig,
) -> Result<PathBuf, String> {
    std::fs::create_dir_all(output_dir).map_err(|e| e.to_string())?;
    let ts = chrono::Utc::now().format("%Y%m%d_%H%M%S").to_string();
    let pdf_path = output_dir.join(format!("tokscope_report_{}.pdf", ts));
    let html_path = output_dir.join(format!("tokscope_report_{}.html", ts));

    let html = build_report_html(db, since, pricing)?;
    std::fs::write(&html_path, &html).map_err(|e| e.to_string())?;

    let browser = find_headless_browser().ok_or_else(|| {
        format!(
            "Could not find Microsoft Edge or Google Chrome for PDF rendering. \
             Install one of them and try again. The HTML report was saved to {}.",
            html_path.display()
        )
    })?;

    let profile = TempProfile(output_dir.join(format!("browser-profile-{ts}")));
    let file_url = format!("file:///{}", html_path.to_string_lossy().replace('\\', "/"));
    let status = run_print_to_pdf(&browser, "--headless=new", &profile.0, &pdf_path, &file_url)?;
    if !status.success() {
        return Err(format!(
            "{} exited with {:?} while generating the PDF",
            browser.to_string_lossy(),
            status.code()
        ));
    }
    if !pdf_path.exists() {
        return Err("Headless browser returned success but did not produce a PDF".to_string());
    }

    Ok(pdf_path)
}

/// Locate a Chromium-family browser capable of `--print-to-pdf`. Order:
/// Edge (pre-installed on all Windows 10/11), then Chrome. On non-Windows
/// falls through to the common Linux/mac binary names so the code at
/// least stays portable even though this ships as a Windows app.
fn find_headless_browser() -> Option<PathBuf> {
    let candidates: Vec<PathBuf> = if cfg!(windows) {
        let pf = std::env::var("ProgramFiles").unwrap_or_else(|_| "C:/Program Files".into());
        let pf86 =
            std::env::var("ProgramFiles(x86)").unwrap_or_else(|_| "C:/Program Files (x86)".into());
        let local_appdata = std::env::var("LOCALAPPDATA").unwrap_or_default();
        vec![
            PathBuf::from(format!("{}/Microsoft/Edge/Application/msedge.exe", pf86)),
            PathBuf::from(format!("{}/Microsoft/Edge/Application/msedge.exe", pf)),
            PathBuf::from(format!("{}/Google/Chrome/Application/chrome.exe", pf)),
            PathBuf::from(format!("{}/Google/Chrome/Application/chrome.exe", pf86)),
            PathBuf::from(format!(
                "{}/Google/Chrome/Application/chrome.exe",
                local_appdata
            )),
        ]
    } else {
        vec![
            PathBuf::from("/usr/bin/microsoft-edge"),
            PathBuf::from("/usr/bin/google-chrome"),
            PathBuf::from("/usr/bin/chromium"),
            PathBuf::from("/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge"),
            PathBuf::from("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"),
        ]
    };
    candidates.into_iter().find(|p| p.exists())
}

/// Build the HTML document that is either saved as-is or rendered to PDF.
/// Uses a print-oriented stylesheet (no onclick buttons, no web fonts) so
/// Chromium's `--print-to-pdf` produces a clean, paginated document.
fn build_report_html(
    db: &Database,
    since: Option<&str>,
    pricing: &PricingConfig,
) -> Result<String, String> {
    let adv = db.get_advanced_stats(since, &[], None, pricing)?;

    let conn = db.read_connection()?;
    let tf = build_time_filter(since, "t")?;
    let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let range_label = since.unwrap_or("All time");

    let (sessions, turns, inp, cc, cr, outp) = query_totals(&conn, &tf)?;
    let models = query_by_model(&conn, &tf)?;
    let projects = query_by_project(&conn, &tf)?;
    let top_sessions = query_top_sessions(&conn, &tf, pricing)?;

    let cost_expr = build_cost_expr(pricing);
    let cost: f64 = conn
        .query_row(
            &format!("SELECT COALESCE({cost_expr},0) FROM turns t WHERE 1=1 {tf}"),
            [],
            |r| r.get(0),
        )
        .map_err(|e| format!("Database error: {e}"))?;

    let text_est = est_tokens(adv.text_chars);
    let tool_est = est_tokens(adv.tool_input_chars);
    let think_est = (adv.total_output_tokens - text_est - tool_est).max(0);
    let input_total = adv.total_input_tokens + adv.total_cache_read + adv.total_cache_creation;

    let mut h = String::new();
    h.push_str(&format!(r#"<!DOCTYPE html>
<html><head><meta charset="UTF-8"><title>TokenScope Report</title>
<style>
*{{margin:0;padding:0;box-sizing:border-box}}
@page{{size:A4;margin:18mm 14mm}}
body{{font-family:-apple-system,Segoe UI,Arial,sans-serif;color:#111;font-size:10pt;line-height:1.4}}
h1{{font-size:20pt;color:#c2410c;margin-bottom:4px}}
h2{{font-size:12pt;text-transform:uppercase;letter-spacing:1px;color:#333;margin:18pt 0 6pt;border-bottom:1px solid #888;padding-bottom:3pt;page-break-after:avoid}}
h3{{font-size:10pt;margin:10pt 0 4pt;page-break-after:avoid}}
.subtitle{{font-family:Consolas,monospace;font-size:9pt;color:#555;margin-bottom:14pt}}
.stat-row{{display:flex;flex-wrap:wrap;gap:10pt;margin:10pt 0 14pt}}
.stat-box{{border:1px solid #aaa;border-radius:4pt;padding:6pt 10pt;min-width:90pt}}
.sl{{font-family:Consolas,monospace;font-size:7.5pt;letter-spacing:1px;color:#666;text-transform:uppercase}}
.sv{{font-family:Consolas,monospace;font-size:14pt;font-weight:700;color:#111}}
.sv.green{{color:#047857}}
table{{width:100%;border-collapse:collapse;font-family:Consolas,monospace;font-size:8.5pt;margin:4pt 0 10pt;page-break-inside:auto}}
tr{{page-break-inside:avoid;page-break-after:auto}}
th{{text-align:left;padding:4pt 6pt;font-size:8pt;letter-spacing:1px;text-transform:uppercase;color:#555;border-bottom:1.5pt solid #333;background:#f2f2f2}}
td{{padding:3pt 6pt;border-bottom:1px solid #ddd;color:#222}}
.num{{text-align:right}}
.cost{{color:#047857;font-weight:600}}
.note{{font-size:8pt;color:#555;font-style:italic;margin:2pt 0 6pt}}
.bar-row{{display:flex;align-items:center;gap:8pt;margin:2pt 0;font-family:Consolas,monospace;font-size:9pt}}
.bar-label{{width:80pt;color:#555}}
.bar-track{{flex:1;height:10pt;background:#eee;border:1px solid #ccc;border-radius:2pt;overflow:hidden}}
.bar-fill{{height:100%;background:#c2410c}}
.bar-fill.blue{{background:#1e40af}}
.bar-fill.green{{background:#047857}}
.bar-fill.purple{{background:#6d28d9}}
.bar-fill.orange{{background:#c2410c}}
.bar-val{{width:130pt;color:#111;font-weight:600}}
.section-break{{page-break-before:always}}
</style></head><body>
<h1>TokenScope Report</h1>
<div class="subtitle">Generated: {now} UTC · Range: {range_label} · Est. Cost: ${cost:.2}</div>
<div class="stat-row">
  <div class="stat-box"><div class="sl">Sessions</div><div class="sv">{sessions}</div></div>
  <div class="stat-box"><div class="sl">Turns</div><div class="sv">{turns}</div></div>
  <div class="stat-box"><div class="sl">Input</div><div class="sv">{inp}</div></div>
  <div class="stat-box"><div class="sl">Output</div><div class="sv">{outp}</div></div>
  <div class="stat-box"><div class="sl">Cache Read</div><div class="sv">{cr}</div></div>
  <div class="stat-box"><div class="sl">Cache Create</div><div class="sv">{cc}</div></div>
  <div class="stat-box"><div class="sl">Est. Cost</div><div class="sv green">${cost:.2}</div></div>
</div>
"#, now=now, range_label=range_label, cost=cost,
    sessions=fnum(sessions), turns=fnum(turns), inp=fnum(inp), outp=fnum(outp), cr=fnum(cr), cc=fnum(cc)));

    // ── Output composition ──
    h.push_str("<h2>Output Composition</h2>");
    h.push_str(&format!(
        "<div class='note'>Total output tokens: <b>{}</b> across <b>{}</b> turns ({} with thinking, {} with tool use).</div>",
        fnum(adv.total_output_tokens), fnum(adv.total_turns),
        fnum(adv.turns_with_thinking), fnum(adv.turns_with_tools)));
    h.push_str(&bar_row(
        "Text",
        "orange",
        text_est,
        adv.total_output_tokens,
    ));
    h.push_str(&bar_row(
        "Tool use",
        "blue",
        tool_est,
        adv.total_output_tokens,
    ));
    h.push_str(&bar_row(
        "Thinking*",
        "purple",
        think_est,
        adv.total_output_tokens,
    ));
    h.push_str("<div class='note'>* Thinking is a residual estimate: Claude Code logs encrypted signatures but not plaintext. Turns with thinking blocks are counted; the token share is inferred as output_total − text − tool_input.</div>");

    // ── Input & Cache composition ──
    h.push_str("<h2>Input &amp; Cache Composition</h2>");
    h.push_str(&format!(
        "<div class='note'>Total input-side tokens: <b>{}</b>.</div>",
        fnum(input_total)
    ));
    h.push_str(&bar_row(
        "Fresh input",
        "orange",
        adv.total_input_tokens,
        input_total,
    ));
    h.push_str(&bar_row(
        "Cache write",
        "purple",
        adv.total_cache_creation,
        input_total,
    ));
    h.push_str(&bar_row(
        "Cache read",
        "green",
        adv.total_cache_read,
        input_total,
    ));

    // ── Interactions ──
    h.push_str("<h2>Interactions</h2>");
    h.push_str(&format!(r#"<div class="stat-row">
        <div class="stat-box"><div class="sl">Questions Asked</div><div class="sv">{}</div></div>
        <div class="stat-box"><div class="sl">Plan Mode</div><div class="sv">{}</div></div>
        <div class="stat-box"><div class="sl">Denied Calls</div><div class="sv">{}</div></div>
        <div class="stat-box"><div class="sl">Subagents Spawned</div><div class="sv">{}</div></div>
        <div class="stat-box"><div class="sl">Subagent Turns</div><div class="sv">{}</div></div>
        <div class="stat-box"><div class="sl">Subagent Cost</div><div class="sv green">${:.2}</div></div>
        </div>"#,
        fnum(adv.ask_user_count), fnum(adv.plan_mode_count), fnum(adv.denied_count),
        fnum(adv.subagent_stats.spawn_count), fnum(adv.subagent_stats.subagent_turns),
        adv.subagent_stats.subagent_cost));

    // ── Denied breakdown ──
    if !adv.denied_breakdown.is_empty() {
        h.push_str("<h2>Denied Tool Calls</h2><table><tr><th>Tool</th><th>Category</th><th class='num'>Denials</th></tr>");
        for d in &adv.denied_breakdown {
            h.push_str(&format!(
                "<tr><td>{}</td><td>{}</td><td class='num'>{}</td></tr>",
                html_escape(&d.tool_name),
                html_escape(&d.category),
                fnum(d.call_count)
            ));
        }
        h.push_str("</table>");
    }

    // ── Category totals ──
    if !adv.category_totals.is_empty() {
        h.push_str("<h2>Category Totals</h2><table><tr><th>Category</th><th class='num'>Calls</th><th class='num'>Turns</th><th class='num'>Input chars</th></tr>");
        for c in &adv.category_totals {
            h.push_str(&format!("<tr><td>{}</td><td class='num'>{}</td><td class='num'>{}</td><td class='num'>{}</td></tr>",
                html_escape(&c.category), fnum(c.call_count), fnum(c.turn_count), fnum(c.input_chars)));
        }
        h.push_str("</table>");
    }

    // ── Top tools ──
    if !adv.tool_breakdown.is_empty() {
        h.push_str("<h2>Top Tools</h2><table><tr><th>Tool</th><th>Category</th><th class='num'>Calls</th><th class='num'>Input chars</th></tr>");
        for t in adv.tool_breakdown.iter().take(30) {
            h.push_str(&format!(
                "<tr><td>{}</td><td>{}</td><td class='num'>{}</td><td class='num'>{}</td></tr>",
                html_escape(&t.tool_name),
                html_escape(&t.category),
                fnum(t.call_count),
                fnum(t.input_chars)
            ));
        }
        h.push_str("</table>");
    }

    // ── MCP servers ──
    if !adv.mcp_breakdown.is_empty() {
        h.push_str("<h2>MCP Servers</h2><table><tr><th>Server</th><th class='num'>Distinct Tools</th><th class='num'>Calls</th><th class='num'>Input chars</th></tr>");
        for m in &adv.mcp_breakdown {
            h.push_str(&format!("<tr><td>{}</td><td class='num'>{}</td><td class='num'>{}</td><td class='num'>{}</td></tr>",
                html_escape(&m.server), fnum(m.tool_count), fnum(m.call_count), fnum(m.input_chars)));
        }
        h.push_str("</table>");
    }

    // ── Skills ──
    if !adv.skill_breakdown.is_empty() {
        h.push_str(
            "<h2>Skills Invoked</h2><table><tr><th>Skill</th><th class='num'>Calls</th></tr>",
        );
        for s in &adv.skill_breakdown {
            h.push_str(&format!(
                "<tr><td>{}</td><td class='num'>{}</td></tr>",
                html_escape(&s.skill_name),
                fnum(s.call_count)
            ));
        }
        h.push_str("</table>");
    }

    // ── Subagent types ──
    if !adv.subagent_types.is_empty() {
        h.push_str(
            "<h2>Subagent Types</h2><table><tr><th>Subagent</th><th class='num'>Spawns</th></tr>",
        );
        for s in &adv.subagent_types {
            h.push_str(&format!(
                "<tr><td>{}</td><td class='num'>{}</td></tr>",
                html_escape(&s.subagent_type),
                fnum(s.call_count)
            ));
        }
        h.push_str("</table>");
    }

    // ── Top subjects ──
    if !adv.top_bash.is_empty() {
        h.push_str(
            "<h2>Top Bash Commands</h2><table><tr><th>Command</th><th class='num'>Calls</th></tr>",
        );
        for s in adv.top_bash.iter().take(20) {
            h.push_str(&format!(
                "<tr><td>{}</td><td class='num'>{}</td></tr>",
                html_escape(&s.subject),
                fnum(s.call_count)
            ));
        }
        h.push_str("</table>");
    }
    if !adv.top_files.is_empty() {
        h.push_str(
            "<h2>Top Files Touched</h2><table><tr><th>File</th><th class='num'>Accesses</th></tr>",
        );
        for s in adv.top_files.iter().take(20) {
            h.push_str(&format!(
                "<tr><td>{}</td><td class='num'>{}</td></tr>",
                html_escape(&s.subject),
                fnum(s.call_count)
            ));
        }
        h.push_str("</table>");
    }
    if !adv.top_domains.is_empty() {
        h.push_str(
            "<h2>Top Web Domains</h2><table><tr><th>Domain</th><th class='num'>Hits</th></tr>",
        );
        for s in adv.top_domains.iter().take(20) {
            h.push_str(&format!(
                "<tr><td>{}</td><td class='num'>{}</td></tr>",
                html_escape(&s.subject),
                fnum(s.call_count)
            ));
        }
        h.push_str("</table>");
    }

    // ── By model ──
    h.push_str("<div class='section-break'></div><h2>By Model</h2><table><tr><th>Model</th><th class='num'>Turns</th><th class='num'>Input</th><th class='num'>Cache Create</th><th class='num'>Cache Read</th><th class='num'>Output</th><th class='num'>Total</th></tr>");
    for (model, turns, inp, cc, cr, outp) in &models {
        let t = inp + cc + cr + outp;
        h.push_str(&format!("<tr><td>{}</td><td class='num'>{}</td><td class='num'>{}</td><td class='num'>{}</td><td class='num'>{}</td><td class='num'>{}</td><td class='num'>{}</td></tr>",
            html_escape(model), fnum(*turns), fnum(*inp), fnum(*cc), fnum(*cr), fnum(*outp), fnum(t)));
    }
    h.push_str("</table>");

    // ── By project ──
    h.push_str("<h2>By Project</h2><table><tr><th>Project</th><th class='num'>Sessions</th><th class='num'>Turns</th><th class='num'>Input</th><th class='num'>Output</th><th class='num'>Cache Read</th><th class='num'>Cache Create</th></tr>");
    for (proj, sess, turns, inp, outp, cr, cc) in &projects {
        h.push_str(&format!("<tr><td>{}</td><td class='num'>{}</td><td class='num'>{}</td><td class='num'>{}</td><td class='num'>{}</td><td class='num'>{}</td><td class='num'>{}</td></tr>",
            html_escape(proj), sess, fnum(*turns), fnum(*inp), fnum(*outp), fnum(*cr), fnum(*cc)));
    }
    h.push_str("</table>");

    // ── Top sessions ──
    h.push_str("<h2>Top 25 Costliest Sessions</h2><table><tr><th>Project</th><th>Model</th><th>Period</th><th class='num'>Turns</th><th class='num'>Input</th><th class='num'>Output</th><th class='num'>Cost</th></tr>");
    for (sid, proj, model, start, end, turns, inp, outp, _cr, _cc, cost) in &top_sessions {
        let s = if start.len() >= 10 {
            &start[..10]
        } else {
            start.as_str()
        };
        let e = if end.len() >= 10 {
            &end[..10]
        } else {
            end.as_str()
        };
        let _ = sid;
        h.push_str(&format!("<tr><td>{}</td><td>{}</td><td>{} – {}</td><td class='num'>{}</td><td class='num'>{}</td><td class='num'>{}</td><td class='num cost'>${:.2}</td></tr>",
            html_escape(proj), html_escape(model), s, e, turns, fnum(*inp), fnum(*outp), cost));
    }
    h.push_str("</table></body></html>");

    Ok(h)
}

fn bar_row(label: &str, color: &str, value: i64, total: i64) -> String {
    let pct_val = if total > 0 {
        (value as f64 / total as f64 * 100.0).min(100.0)
    } else {
        0.0
    };
    format!(
        r#"<div class="bar-row"><div class="bar-label">{lbl}</div><div class="bar-track"><div class="bar-fill {col}" style="width:{w:.1}%"></div></div><div class="bar-val">{v} ({p:.1}%)</div></div>"#,
        lbl = html_escape(label),
        col = color,
        w = pct_val,
        v = fnum(value),
        p = pct_val,
    )
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn md_escape(s: &str) -> String {
    s.replace('|', "\\|").replace('`', "\\`")
}

fn db_err(e: rusqlite::Error) -> String {
    format!("Database error: {e}")
}

fn query_totals(
    conn: &rusqlite::Connection,
    tf: &str,
) -> Result<(i64, i64, i64, i64, i64, i64), String> {
    conn.query_row(
        &format!(
            "SELECT COUNT(DISTINCT t.session_id), COUNT(*),
            COALESCE(SUM(t.input_tokens),0), COALESCE(SUM(t.cache_creation),0),
            COALESCE(SUM(t.cache_read),0), COALESCE(SUM(t.output_tokens),0)
        FROM turns t WHERE 1=1 {}",
            tf
        ),
        [],
        |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
            ))
        },
    )
    .map_err(db_err)
}

fn query_by_model(
    conn: &rusqlite::Connection,
    tf: &str,
) -> Result<Vec<(String, i64, i64, i64, i64, i64)>, String> {
    let mut stmt = conn
        .prepare(&format!(
        "SELECT model, COUNT(*), COALESCE(SUM(input_tokens),0), COALESCE(SUM(cache_creation),0),
            COALESCE(SUM(cache_read),0), COALESCE(SUM(output_tokens),0)
        FROM turns t WHERE 1=1 {} GROUP BY model ORDER BY 3 DESC", tf
    ))
        .map_err(db_err)?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
            ))
        })
        .map_err(db_err)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
}

fn query_by_project(
    conn: &rusqlite::Connection,
    tf: &str,
) -> Result<Vec<(String, i64, i64, i64, i64, i64, i64)>, String> {
    let mut stmt = conn.prepare(&format!(
        "SELECT s.project, COUNT(DISTINCT t.session_id), COUNT(*),
            COALESCE(SUM(t.input_tokens),0), COALESCE(SUM(t.output_tokens),0),
            COALESCE(SUM(t.cache_read),0), COALESCE(SUM(t.cache_creation),0)
        FROM turns t JOIN sessions s ON t.session_id = s.id
        WHERE 1=1 {} GROUP BY s.project ORDER BY SUM(t.input_tokens+t.output_tokens+t.cache_read+t.cache_creation) DESC", tf
    )).map_err(db_err)?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
            ))
        })
        .map_err(db_err)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
}

type TopSession = (
    String,
    String,
    String,
    String,
    String,
    i64,
    i64,
    i64,
    i64,
    i64,
    f64,
);

fn query_top_sessions(
    conn: &rusqlite::Connection,
    tf: &str,
    pricing: &PricingConfig,
) -> Result<Vec<TopSession>, String> {
    let cost_expr = build_cost_case(pricing, Some("t"));
    let mut stmt = conn
        .prepare(&format!(
            "SELECT t.session_id, s.project,
            (SELECT model FROM turns WHERE session_id = t.session_id ORDER BY timestamp DESC LIMIT 1),
            MIN(t.timestamp), MAX(t.timestamp),
            COUNT(*), COALESCE(SUM(t.input_tokens),0), COALESCE(SUM(t.output_tokens),0),
            COALESCE(SUM(t.cache_read),0), COALESCE(SUM(t.cache_creation),0),
            COALESCE(SUM({}),0) as cost
        FROM turns t JOIN sessions s ON t.session_id = s.id
        WHERE 1=1 {} GROUP BY t.session_id ORDER BY cost DESC LIMIT 25",
            cost_expr, tf
        ))
        .map_err(db_err)?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
                r.get(7)?,
                r.get(8)?,
                r.get(9)?,
                r.get(10)?,
            ))
        })
        .map_err(db_err)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
}

fn fnum(n: i64) -> String {
    if n >= 1_000_000_000 {
        format!("{:.2}B", n as f64 / 1e9)
    } else if n >= 1_000_000 {
        format!("{:.2}M", n as f64 / 1e6)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1e3)
    } else {
        format!("{}", n)
    }
}
