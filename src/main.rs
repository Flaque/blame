use chrono::{DateTime, TimeZone, Utc};
use clap::Parser;
use glob::glob;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Parser)]
#[command(name = "blame")]
#[command(about = "Find out who is responsible for a file or folder")]
#[command(after_help = "\
EXAMPLES:
    blame foo.rs              Blame a single file
    blame foo.rs bar.rs       Blame multiple files
    blame src/                Blame all files in a directory
    blame \"**/*.rs\"           Blame all Rust files (glob pattern)
    blame -v src/             Show all contributors with percentages
    blame --gh src/           Output GitHub usernames (for PR reviewers)
    blame --gh --only-name src/   Output just the username (for scripts)
")]
struct Args {
    /// Files, folders, or glob patterns to analyze (e.g., foo.rs bar.rs "**/*.rs")
    #[arg(required_unless_present = "upgrade")]
    patterns: Vec<String>,

    /// Show detailed breakdown by contributor
    #[arg(short, long)]
    verbose: bool,

    /// Output GitHub usernames instead of git author names
    #[arg(long)]
    gh: bool,

    /// Only output the name (for use in scripts)
    #[arg(long)]
    only_name: bool,

    /// Upgrade blame to the latest version
    #[arg(long)]
    upgrade: bool,
}

#[derive(Default)]
struct AuthorStats {
    lines: usize,
    last_commit_time: i64,
    commits: HashSet<String>,
}

fn main() {
    let args = Args::parse();

    if args.upgrade {
        upgrade();
        return;
    }

    // Expand all patterns and collect unique files
    let mut all_files: HashSet<String> = HashSet::new();
    let mut git_root: Option<PathBuf> = None;

    for pattern in &args.patterns {
        let expanded = expand_pattern(pattern);

        if expanded.is_empty() {
            eprintln!("Warning: No files matched '{}'", pattern);
            continue;
        }

        for path in expanded {
            // Get git root from first valid path
            if git_root.is_none() {
                git_root = get_git_root(&path);
            }

            let root = match &git_root {
                Some(r) => r,
                None => {
                    eprintln!("Error: '{}' is not in a git repository", path.display());
                    std::process::exit(1);
                }
            };

            if path.is_dir() {
                for f in get_git_files_in_dir(&path, root) {
                    all_files.insert(f);
                }
            } else if is_git_tracked(&path, root) {
                all_files.insert(path.to_string_lossy().to_string());
            }
        }
    }

    let files: Vec<String> = all_files.into_iter().collect();

    if files.is_empty() {
        eprintln!("Error: No git-tracked files found");
        std::process::exit(1);
    }

    let git_root = git_root.unwrap();

    let mut stats: HashMap<String, AuthorStats> = HashMap::new();

    for file in &files {
        if let Err(e) = collect_blame_stats(file, &git_root, &mut stats) {
            eprintln!("Warning: Could not process '{}': {}", file, e);
        }
    }

    if stats.is_empty() {
        eprintln!("Error: No blame data found");
        std::process::exit(1);
    }

    // Resolve GitHub usernames if --gh flag is set
    let stats = if args.gh {
        match get_github_repo(&git_root) {
            Some((owner, repo)) => resolve_github_usernames(stats, &owner, &repo),
            None => {
                eprintln!("Error: Could not determine GitHub repository from remote");
                std::process::exit(1);
            }
        }
    } else {
        stats
    };

    let mut authors: Vec<_> = stats.into_iter().collect();
    authors.sort_by(|a, b| b.1.lines.cmp(&a.1.lines));

    let total_lines: usize = authors.iter().map(|(_, s)| s.lines).sum();

    if args.only_name {
        if args.verbose {
            for (author, _) in &authors {
                println!("{}", author);
            }
        } else {
            println!("{}", authors[0].0);
        }
    } else if args.verbose {
        println!();
        for (author, author_stats) in &authors {
            let percentage = (author_stats.lines as f64 / total_lines as f64) * 100.0;
            let last_touch = format_relative_time(author_stats.last_commit_time);
            println!(
                "\x1b[38;5;208m{}\x1b[0m  {:>5.1}%  \x1b[2m(last touched {})\x1b[0m",
                author, percentage, last_touch
            );
        }
        println!();
    } else {
        let (author, author_stats) = &authors[0];
        let percentage = (author_stats.lines as f64 / total_lines as f64) * 100.0;
        let last_touch = format_relative_time(author_stats.last_commit_time);
        println!(
            "\x1b[38;5;208m{}\x1b[0m  {:>5.1}%  \x1b[2m(last touched {})\x1b[0m",
            author, percentage, last_touch
        );
    }
}

fn is_git_tracked(path: &Path, git_root: &Path) -> bool {
    let relative = path.strip_prefix(git_root).unwrap_or(path);
    let output = Command::new("git")
        .args(["ls-files", "--error-unmatch", &relative.to_string_lossy()])
        .current_dir(git_root)
        .output();

    matches!(output, Ok(o) if o.status.success())
}

fn expand_pattern(pattern: &str) -> Vec<PathBuf> {
    // First try as a literal path
    let literal_path = Path::new(pattern);
    if literal_path.exists() {
        return vec![literal_path.canonicalize().unwrap_or(literal_path.to_path_buf())];
    }

    // Try as a glob pattern
    match glob(pattern) {
        Ok(paths) => paths
            .filter_map(|p| p.ok())
            .filter_map(|p| p.canonicalize().ok())
            .collect(),
        Err(_) => vec![],
    }
}

fn get_git_root(path: &Path) -> Option<PathBuf> {
    let start_dir = if path.is_dir() { path } else { path.parent()? };

    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(start_dir)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Some(PathBuf::from(root))
}

fn get_git_files_in_dir(dir: &Path, git_root: &Path) -> Vec<String> {
    let relative_dir = dir.strip_prefix(git_root).unwrap_or(dir);

    let output = Command::new("git")
        .args(["ls-files", &relative_dir.to_string_lossy()])
        .current_dir(git_root)
        .output()
        .expect("Failed to run git ls-files");

    if !output.status.success() {
        return vec![];
    }

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|s| git_root.join(s).to_string_lossy().to_string())
        .collect()
}

fn collect_blame_stats(
    file: &str,
    git_root: &Path,
    stats: &mut HashMap<String, AuthorStats>,
) -> Result<(), String> {
    let file_path = Path::new(file);
    let relative_file = file_path
        .strip_prefix(git_root)
        .unwrap_or(file_path)
        .to_string_lossy();

    let output = Command::new("git")
        .args(["blame", "--line-porcelain", &*relative_file])
        .current_dir(git_root)
        .output()
        .map_err(|e| format!("Failed to run git blame: {}", e))?;

    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).to_string());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut current_author: Option<String> = None;
    let mut current_time: i64 = 0;
    let mut current_sha: Option<String> = None;

    for line in stdout.lines() {
        // First line of each block starts with 40-char SHA
        if line.len() >= 40 && line.chars().take(40).all(|c| c.is_ascii_hexdigit()) {
            current_sha = Some(line[..40].to_string());
        } else if let Some(author) = line.strip_prefix("author ") {
            current_author = Some(author.to_string());
        } else if let Some(time_str) = line.strip_prefix("author-time ") {
            if let Ok(time) = time_str.parse::<i64>() {
                current_time = time;
            }
        } else if line.starts_with('\t') {
            // This is the actual line content, meaning we've finished parsing this block
            if let Some(ref author) = current_author {
                let entry = stats.entry(author.clone()).or_default();
                entry.lines += 1;
                if current_time > entry.last_commit_time {
                    entry.last_commit_time = current_time;
                }
                if let Some(ref sha) = current_sha {
                    entry.commits.insert(sha.clone());
                }
            }
        }
    }

    Ok(())
}

fn format_relative_time(timestamp: i64) -> String {
    let dt: DateTime<Utc> = Utc.timestamp_opt(timestamp, 0).unwrap();
    let now = Utc::now();
    let duration = now.signed_duration_since(dt);

    let days = duration.num_days();
    if days == 0 {
        let hours = duration.num_hours();
        if hours == 0 {
            let minutes = duration.num_minutes();
            if minutes <= 1 {
                return "just now".to_string();
            }
            return format!("{} minutes ago", minutes);
        }
        return format!("{} hours ago", hours);
    }
    if days == 1 {
        return "yesterday".to_string();
    }
    if days < 7 {
        return format!("{} days ago", days);
    }
    if days < 30 {
        let weeks = days / 7;
        return format!("{} week{} ago", weeks, if weeks == 1 { "" } else { "s" });
    }
    if days < 365 {
        let months = days / 30;
        return format!("{} month{} ago", months, if months == 1 { "" } else { "s" });
    }
    let years = days / 365;
    format!("{} year{} ago", years, if years == 1 { "" } else { "s" })
}

fn get_github_repo(git_root: &Path) -> Option<(String, String)> {
    let output = Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(git_root)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let url = String::from_utf8_lossy(&output.stdout).trim().to_string();

    // Parse GitHub URL formats:
    // https://github.com/owner/repo.git
    // git@github.com:owner/repo.git
    // https://github.com/owner/repo
    if let Some(rest) = url.strip_prefix("https://github.com/") {
        let rest = rest.trim_end_matches(".git");
        let parts: Vec<&str> = rest.splitn(2, '/').collect();
        if parts.len() == 2 {
            return Some((parts[0].to_string(), parts[1].to_string()));
        }
    } else if let Some(rest) = url.strip_prefix("git@github.com:") {
        let rest = rest.trim_end_matches(".git");
        let parts: Vec<&str> = rest.splitn(2, '/').collect();
        if parts.len() == 2 {
            return Some((parts[0].to_string(), parts[1].to_string()));
        }
    }

    None
}

fn get_github_username(owner: &str, repo: &str, sha: &str) -> Option<String> {
    let output = Command::new("gh")
        .args([
            "api",
            &format!("repos/{}/{}/commits/{}", owner, repo, sha),
            "--jq",
            ".author.login",
        ])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let username = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if username.is_empty() || username == "null" {
        None
    } else {
        Some(username)
    }
}

fn resolve_github_usernames(
    stats: HashMap<String, AuthorStats>,
    owner: &str,
    repo: &str,
) -> HashMap<String, AuthorStats> {
    let mut author_to_gh: HashMap<String, Option<String>> = HashMap::new();

    // For each author, try to find their GitHub username from one of their commits
    for (author, author_stats) in &stats {
        if author_to_gh.contains_key(author) {
            continue;
        }
        // Try the first commit to get the GitHub username
        if let Some(sha) = author_stats.commits.iter().next() {
            let gh_user = get_github_username(owner, repo, sha);
            author_to_gh.insert(author.clone(), gh_user);
        }
    }

    // Rebuild stats keyed by GitHub username (or fall back to git author name)
    let mut new_stats: HashMap<String, AuthorStats> = HashMap::new();
    for (author, author_stats) in stats {
        let key = author_to_gh
            .get(&author)
            .and_then(|u| u.clone())
            .unwrap_or(author);
        let entry = new_stats.entry(key).or_default();
        entry.lines += author_stats.lines;
        if author_stats.last_commit_time > entry.last_commit_time {
            entry.last_commit_time = author_stats.last_commit_time;
        }
        entry.commits.extend(author_stats.commits);
    }

    new_stats
}

fn upgrade() {
    println!("Upgrading blame...");

    let status = Command::new("sh")
        .args(["-c", "git clone https://github.com/flaque/blame /tmp/blame-upgrade 2>/dev/null || git -C /tmp/blame-upgrade pull && cargo install --path /tmp/blame-upgrade"])
        .status();

    match status {
        Ok(s) if s.success() => println!("Upgrade complete!"),
        _ => {
            eprintln!("Upgrade failed");
            std::process::exit(1);
        }
    }
}
