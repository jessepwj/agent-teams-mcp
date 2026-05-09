use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const WEB_DIR: &str = "web/team-mode";
const INDEX_PLACEHOLDER: &str = "__TEAM_MODE_WEB_BUNDLE_REVISION__";
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let web_dir = manifest_dir.join(WEB_DIR);
    let files = collect_files(&web_dir);

    println!("cargo:rerun-if-changed={}", web_dir.display());
    for file in &files {
        println!("cargo:rerun-if-changed={}", file.display());
    }

    let revision = bundle_revision(&web_dir, &files);
    println!("cargo:rustc-env=TEAM_MODE_WEB_BUNDLE_REVISION={revision}");

    // Embed a git commit identifier so binaries can self-report their build
    // version. Used by the version-mismatch check in relay/hook to warn
    // when the on-disk binary is newer than the running service.
    let git_rev = git_rev(&manifest_dir);
    println!("cargo:rustc-env=TEAM_MODE_GIT_REV={git_rev}");
    // Re-run build.rs when HEAD moves so the embedded value tracks commits
    // without requiring a clean rebuild. `index` covers staged-but-uncommitted
    // changes that influence the dirty marker.
    let git_dir = manifest_dir.join(".git");
    if git_dir.exists() {
        println!("cargo:rerun-if-changed={}", git_dir.join("HEAD").display());
        println!("cargo:rerun-if-changed={}", git_dir.join("index").display());
    }

    let index_path = web_dir.join("index.html");
    let index = fs::read_to_string(&index_path).expect("read web/team-mode/index.html");
    let processed = index.replace(INDEX_PLACEHOLDER, &revision);
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    fs::write(out_dir.join("index.processed.html"), processed).expect("write processed index");
}

/// Resolve a short commit identifier suitable for embedding in the binary.
/// Returns `unknown` when git is unavailable (e.g. cargo install from
/// crates.io) so callers never panic and the version-mismatch check
/// gracefully no-ops in that case.
fn git_rev(manifest_dir: &Path) -> String {
    let head = Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .current_dir(manifest_dir)
        .output();
    let head = match head {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).trim().to_string(),
        _ => return "unknown".to_string(),
    };
    let dirty = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(manifest_dir)
        .output()
        .ok()
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(false);
    if dirty {
        format!("{head}-dirty")
    } else {
        head
    }
}

fn collect_files(web_dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_files_recursive(web_dir, &mut files);
    files.sort_by_key(|path| relative_key(web_dir, path));
    files
}

fn collect_files_recursive(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries = fs::read_dir(dir)
        .unwrap_or_else(|err| panic!("read {}: {err}", dir.display()))
        .collect::<Result<Vec<_>, _>>()
        .unwrap_or_else(|err| panic!("read entry in {}: {err}", dir.display()));
    entries.sort_by_key(|entry| entry.path());
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            collect_files_recursive(&path, out);
        } else if path.is_file() && is_baked_static_asset(&path) {
            out.push(path);
        }
    }
}

fn is_baked_static_asset(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|ext| ext.to_str()),
        Some("html" | "css" | "js")
    )
}

fn bundle_revision(web_dir: &Path, files: &[PathBuf]) -> String {
    let mut hash = FNV_OFFSET;
    for file in files {
        let rel = relative_key(web_dir, file);
        hash = fnv_update(hash, rel.as_bytes());
        hash = fnv_update(hash, &[0]);
        let bytes = fs::read(file).unwrap_or_else(|err| panic!("read {}: {err}", file.display()));
        hash = fnv_update(hash, &bytes);
        hash = fnv_update(hash, &[0]);
    }
    format!("{hash:016x}")
}

fn relative_key(web_dir: &Path, path: &Path) -> String {
    path.strip_prefix(web_dir)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn fnv_update(mut hash: u64, bytes: &[u8]) -> u64 {
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}
