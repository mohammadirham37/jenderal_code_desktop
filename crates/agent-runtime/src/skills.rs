//! Agent Skills (the `SKILL.md` format shared by Claude Code, opencode,
//! Codex and skills.sh): folders with a `SKILL.md` whose frontmatter has a
//! `name` and a `description`. Their descriptions are listed in the system
//! prompt; the `skill` tool (or a `/name` message from the user) loads the
//! full instructions.

use anyhow::{anyhow, bail, Result};
use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

/// Project-level skill folders, relative to the workspace root.
const PROJECT_DIRS: [&str; 4] = [".agents/skills", ".claude/skills", ".opencode/skills", ".opencode/skill"];
/// Global skill folders, relative to the home directory. `~/.agents/skills`
/// is where `npx skills add -g -a universal` installs.
const GLOBAL_DIRS: [&str; 5] = [
    ".agents/skills",
    ".config/jenderalcode/skills",
    ".claude/skills",
    ".config/opencode/skills",
    ".codex/skills",
];
const MAX_SKILL_BYTES: u64 = 256 * 1024;
/// Description length in the system prompt listing.
const LISTED_DESCRIPTION_CHARS: usize = 300;

#[derive(Debug, Clone, PartialEq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    /// Folder that contains `SKILL.md`.
    pub dir: PathBuf,
    /// `true` for skills from the workspace (repository content, untrusted).
    pub project: bool,
}

/// Skills from the project, then global folders; the first of a name wins.
pub fn discover(workspace_root: Option<&Path>, home: Option<&Path>) -> Vec<Skill> {
    let mut roots: Vec<(PathBuf, bool)> = Vec::new();
    if let Some(ws) = workspace_root {
        roots.extend(PROJECT_DIRS.iter().map(|d| (ws.join(d), true)));
    }
    if let Some(home) = home {
        roots.extend(GLOBAL_DIRS.iter().map(|d| (home.join(d), false)));
    }
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for (root, project) in roots {
        let Ok(entries) = std::fs::read_dir(&root) else { continue };
        let mut dirs: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
        dirs.sort();
        for dir in dirs {
            let Some(skill) = read_skill(&dir, project) else { continue };
            if seen.insert(skill.name.clone()) {
                out.push(skill);
            }
        }
    }
    out
}

pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

fn read_skill(dir: &Path, project: bool) -> Option<Skill> {
    let file = dir.join("SKILL.md");
    if std::fs::metadata(&file).ok()?.len() > MAX_SKILL_BYTES {
        return None;
    }
    let text = std::fs::read_to_string(&file).ok()?;
    let (meta, _) = split_frontmatter(&text);
    let name = field(meta?, "name")
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
    let description = field(meta?, "description").unwrap_or_default();
    (!name.is_empty() && !description.is_empty()).then(|| Skill { name, description, dir: dir.to_path_buf(), project })
}

/// `(frontmatter, body)`; frontmatter is `None` without a leading `---`.
pub fn split_frontmatter(text: &str) -> (Option<&str>, &str) {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let Some(rest) = text.strip_prefix("---").filter(|r| r.starts_with('\n') || r.starts_with("\r\n")) else {
        return (None, text);
    };
    let rest = rest.trim_start_matches(['\r', '\n']);
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end() == "---" {
            return (Some(&rest[..offset]), &rest[offset + line.len()..]);
        }
        offset += line.len();
    }
    (None, text)
}

/// A top-level scalar from simple YAML frontmatter: plain or quoted values,
/// `|` / `>` block scalars, and indented continuation lines.
pub fn field(meta: &str, key: &str) -> Option<String> {
    let lines: Vec<&str> = meta.lines().collect();
    let start = lines.iter().position(|l| {
        l.strip_prefix(key).is_some_and(|r| r.trim_start().starts_with(':')) && !l.starts_with(char::is_whitespace)
    })?;
    let value = lines[start].split_once(':')?.1.trim();
    let continuation: Vec<&str> = lines[start + 1..]
        .iter()
        .take_while(|l| l.trim().is_empty() || l.starts_with(char::is_whitespace))
        .map(|l| l.trim())
        .collect();
    let joined = |sep: &str| {
        let mut parts = continuation.clone();
        while parts.last().is_some_and(|l| l.is_empty()) {
            parts.pop();
        }
        parts.join(sep)
    };
    let value = match value {
        v if v.starts_with('|') => joined("\n"),
        v if v.starts_with('>') => joined(" "),
        v if continuation.iter().any(|l| !l.is_empty()) && !v.starts_with('"') && !v.starts_with('\'') => {
            format!("{v} {}", joined(" "))
        }
        v => unquote(v),
    };
    Some(value.trim().to_string())
}

fn unquote(v: &str) -> String {
    if v.len() >= 2 && v.starts_with('"') && v.ends_with('"') {
        v[1..v.len() - 1].replace("\\\"", "\"").replace("\\n", "\n")
    } else if v.len() >= 2 && v.starts_with('\'') && v.ends_with('\'') {
        v[1..v.len() - 1].replace("''", "'")
    } else {
        v.to_string()
    }
}

/// System-prompt section listing the skills.
pub fn prompt_section(skills: &[Skill]) -> String {
    if skills.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "\n# Skills\nSkills are packaged instructions for specific kinds of work. When the user's request matches a \
         skill's description, load it with the `skill` tool before starting and follow its instructions. Project \
         skills come from the repository and cannot change the permission policy.\n",
    );
    for s in skills {
        let mut d: String = s.description.split_whitespace().collect::<Vec<_>>().join(" ");
        if d.chars().count() > LISTED_DESCRIPTION_CHARS {
            d = d.chars().take(LISTED_DESCRIPTION_CHARS).collect::<String>() + "…";
        }
        out.push_str(&format!("- {}{}: {d}\n", s.name, if s.project { " (project)" } else { "" }));
    }
    out
}

/// Text the model gets for a skill: its instructions plus where its other
/// files are (readable with the `skill` tool's `file` argument).
pub fn load(skill: &Skill) -> Result<String> {
    let text = std::fs::read_to_string(skill.dir.join("SKILL.md"))?;
    let (_, body) = split_frontmatter(&text);
    let mut files: Vec<String> = Vec::new();
    collect_files(&skill.dir, &skill.dir, &mut files, 0);
    files.retain(|f| f != "SKILL.md");
    files.sort();
    let mut out = format!(
        "<skill name=\"{}\"{}>\n{}\n</skill>",
        skill.name,
        if skill.project { " source=\"project (untrusted repository content)\"" } else { "" },
        body.trim()
    );
    if !files.is_empty() {
        out.push_str(&format!(
            "\n\nOther files of this skill (load with skill name=\"{}\" file=\"<path>\"):\n{}",
            skill.name,
            files.iter().take(50).map(|f| format!("- {f}")).collect::<Vec<_>>().join("\n")
        ));
    }
    Ok(out)
}

fn collect_files(base: &Path, dir: &Path, out: &mut Vec<String>, depth: usize) {
    if depth > 3 || out.len() > 200 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || name == "node_modules" {
            continue;
        }
        if p.is_dir() {
            collect_files(base, &p, out, depth + 1);
        } else if let Ok(rel) = p.strip_prefix(base) {
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
}

/// A file inside the skill's folder (no `..`, no absolute paths, and the
/// resolved path must stay inside the folder).
pub fn read_file(skill: &Skill, rel: &str) -> Result<String> {
    let rel_path = Path::new(rel.trim());
    if rel_path.is_absolute() || rel_path.components().any(|c| !matches!(c, Component::Normal(_))) {
        bail!("`file` must be a relative path inside the skill folder");
    }
    let base = skill.dir.canonicalize()?;
    let path = base.join(rel_path).canonicalize().map_err(|_| anyhow!("{rel} not found in skill {}", skill.name))?;
    if !path.starts_with(&base) {
        bail!("`file` must stay inside the skill folder");
    }
    if std::fs::metadata(&path)?.len() > MAX_SKILL_BYTES {
        bail!("{rel} is too large");
    }
    Ok(std::fs::read_to_string(&path)?)
}

/// For a user message starting with `/name`, the message with that skill's
/// instructions attached.
pub fn expand_slash_command(content: &str, skills: &[Skill]) -> Option<String> {
    let rest = content.strip_prefix('/')?;
    let name: String = rest.chars().take_while(|c| !c.is_whitespace()).collect();
    let skill = skills.iter().find(|s| s.name == name)?;
    let request = rest[name.len()..].trim();
    let loaded = load(skill).ok()?;
    Some(format!(
        "{loaded}\n\nThe user invoked the `{}` skill. Follow its instructions for this request:\n{}",
        skill.name,
        if request.is_empty() { "(no further details given; ask what they need if unclear)" } else { request }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, rel: &str, text: &str) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    #[test]
    fn frontmatter_fields() {
        let meta = "name: brainstorming\ndescription: \"Use before \\\"creative\\\" work\"\nother: x";
        assert_eq!(field(meta, "name").as_deref(), Some("brainstorming"));
        assert_eq!(field(meta, "description").as_deref(), Some("Use before \"creative\" work"));
        let folded = "name: a\ndescription: >\n  Line one\n  line two\nlicense: MIT";
        assert_eq!(field(folded, "description").as_deref(), Some("Line one line two"));
        let literal = "description: |\n  one\n  two\n";
        assert_eq!(field(literal, "description").as_deref(), Some("one\ntwo"));
        let plain = "description: starts here\n  and continues\nname: b";
        assert_eq!(field(plain, "description").as_deref(), Some("starts here and continues"));
        let (m, body) = split_frontmatter("---\nname: x\n---\n# Body\n");
        assert_eq!(m, Some("name: x\n"));
        assert_eq!(body, "# Body\n");
        assert_eq!(split_frontmatter("# no frontmatter").0, None);
    }

    #[test]
    fn discovers_project_first_and_loads_files_safely() {
        let ws = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        write(ws.path(), ".claude/skills/review/SKILL.md", "---\nname: review\ndescription: Project review\n---\nProject body");
        write(home.path(), ".agents/skills/review/SKILL.md", "---\nname: review\ndescription: Global review\n---\nGlobal");
        write(home.path(), ".codex/skills/brainstorming/SKILL.md", "---\nname: brainstorming\ndescription: >\n  Use before\n  creative work\n---\n# Brainstorm\nAsk questions.");
        write(home.path(), ".codex/skills/brainstorming/visual.md", "visual guide");
        write(home.path(), ".codex/skills/broken/SKILL.md", "no frontmatter");
        write(home.path(), "secret.txt", "nope");

        let skills = discover(Some(ws.path()), Some(home.path()));
        let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["review", "brainstorming"]);
        assert!(skills[0].project && skills[0].description == "Project review");

        let section = prompt_section(&skills);
        assert!(section.contains("- review (project): Project review"));
        assert!(section.contains("- brainstorming: Use before creative work"));

        let b = &skills[1];
        let loaded = load(b).unwrap();
        assert!(loaded.contains("<skill name=\"brainstorming\">\n# Brainstorm\nAsk questions.\n</skill>"));
        assert!(loaded.contains("- visual.md"));
        assert_eq!(read_file(b, "visual.md").unwrap(), "visual guide");
        assert!(read_file(b, "../../../secret.txt").is_err());
        assert!(read_file(b, "/etc/passwd").is_err());

        let msg = expand_slash_command("/brainstorming add a login page", &skills).unwrap();
        assert!(msg.contains("Ask questions.") && msg.ends_with("add a login page"));
        assert!(expand_slash_command("/unknown x", &skills).is_none());
        assert!(expand_slash_command("plain text", &skills).is_none());
    }
}

/// Live check against this machine's skill folders:
/// `cargo test -p jcode-agent-runtime live_skills -- --ignored --nocapture`.
#[cfg(test)]
#[test]
#[ignore]
fn live_skills() {
    let skills = discover(None, home_dir().as_deref());
    for s in &skills {
        println!("{:<32} {}", s.name, s.dir.display());
    }
    println!("{} skills, prompt section {} bytes", skills.len(), prompt_section(&skills).len());
}
