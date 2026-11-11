//! Persistent state: a single JSON file plus one audio file per version.
//!
//! The data set is tiny (a kid's tracks), so a JSON file rewritten
//! atomically after each change is simpler and easier to back up than a
//! database.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use musik_shared::*;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Db {
    pub settings: Settings,
    pub admin_password_hash: Option<String>,
    pub api_key: Option<String>,
    pub tracks: Vec<Track>,
    pub ledger: Vec<Ledger>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Track {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub emoji: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub versions: Vec<Version>,
    /// Next branch letter index (0 = "a").
    #[serde(default)]
    pub next_branch: u32,
    #[serde(default)]
    pub last_error: Option<GenError>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Version {
    pub id: String,
    pub parent_id: Option<String>,
    pub label: String,
    pub depth: u32,
    /// "" = main line, otherwise "a", "b", ...
    pub branch: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub favorite: bool,
    pub created_at: DateTime<Utc>,
    pub status: VersionStatus,
    pub params: GenParams,
    pub duration_ms: u32,
    pub estimated_cost: f64,
    pub charged_cost: Option<f64>,
    pub measured_credits: Option<i64>,
    pub song_id: Option<String>,
    /// Raw composition plan returned by ElevenLabs.
    pub el_plan: Option<serde_json::Value>,
    pub song_metadata: Option<SongMetadata>,
    /// File name inside `<data>/audio/`.
    pub audio_file: Option<String>,
    pub content_type: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Ledger {
    pub id: String,
    pub at: DateTime<Utc>,
    pub kind: LedgerKind,
    pub track_id: Option<String>,
    pub version_id: Option<String>,
    pub track_title: String,
    pub version_label: String,
    pub seconds: f64,
    pub estimated_cost: f64,
    pub measured_credits: Option<i64>,
    pub cost: f64,
    pub note: String,
    /// Reserved for a song that is still recording (not yet final).
    #[serde(default)]
    pub pending: bool,
}

pub fn new_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..12].to_string()
}

fn branch_name(mut n: u32) -> String {
    // 0 -> a, 25 -> z, 26 -> aa, ...
    let mut s = Vec::new();
    loop {
        s.push((b'a' + (n % 26) as u8) as char);
        if n < 26 {
            break;
        }
        n = n / 26 - 1;
    }
    s.iter().rev().collect()
}

pub fn make_label(depth: u32, branch: &str) -> String {
    if branch.is_empty() {
        format!("v{depth}")
    } else {
        format!("v{depth}.{branch}")
    }
}

impl Track {
    pub fn version(&self, id: &str) -> Option<&Version> {
        self.versions.iter().find(|v| v.id == id)
    }

    /// Work out depth/branch/label for a new child of `parent_id`.
    ///
    /// The first child of a version continues its line (v0 -> v1); any further
    /// child opens a new branch (v0 -> v1.a, v1.b, ...). Labels stay unique
    /// because each branch is a straight line that only grows at its tip.
    pub fn allocate_label(&mut self, parent_id: Option<&str>) -> (u32, String, String) {
        let (depth, inherit) = match parent_id.and_then(|p| self.version(p)) {
            Some(p) => (p.depth + 1, Some(p.branch.clone())),
            None => (0, None),
        };
        let has_siblings = self.versions.iter().any(|v| v.parent_id.as_deref() == parent_id);
        let branch = match (inherit, has_siblings) {
            (Some(b), false) => b,
            (None, false) => String::new(),
            _ => {
                let b = branch_name(self.next_branch);
                self.next_branch += 1;
                b
            }
        };
        let label = make_label(depth, &branch);
        (depth, branch, label)
    }

    /// The version and all its descendants.
    pub fn subtree_ids(&self, root: &str) -> Vec<String> {
        let mut out = vec![root.to_string()];
        let mut i = 0;
        while i < out.len() {
            let cur = out[i].clone();
            for v in &self.versions {
                if v.parent_id.as_deref() == Some(cur.as_str()) {
                    out.push(v.id.clone());
                }
            }
            i += 1;
        }
        out
    }

    pub fn summary(&self) -> TrackSummary {
        let ready: Vec<&Version> = self.versions.iter().filter(|v| v.status == VersionStatus::Ready).collect();
        TrackSummary {
            id: self.id.clone(),
            title: self.title.clone(),
            emoji: self.emoji.clone(),
            created_at: self.created_at.to_rfc3339(),
            updated_at: self.updated_at.to_rfc3339(),
            version_count: ready.len(),
            latest_label: ready.iter().max_by_key(|v| v.created_at).map(|v| v.label.clone()),
            favorite_label: ready.iter().find(|v| v.favorite).map(|v| v.label.clone()),
            generating: self.versions.iter().any(|v| v.status == VersionStatus::Generating),
        }
    }

    pub fn detail(&self) -> TrackDetail {
        TrackDetail {
            id: self.id.clone(),
            title: self.title.clone(),
            emoji: self.emoji.clone(),
            created_at: self.created_at.to_rfc3339(),
            updated_at: self.updated_at.to_rfc3339(),
            versions: self.versions.iter().map(|v| v.info()).collect(),
            last_error: self.last_error.clone(),
        }
    }
}

impl Version {
    pub fn info(&self) -> VersionInfo {
        // Plan-mode takes: the plan the child built is exact (v2 models always
        // keep section lengths) and still knows which parts were kept/imitated
        // and their lyrics. Prompt-mode takes: use the plan ElevenLabs made.
        let plan = match (self.params.mode, &self.el_plan) {
            (Mode::Plan, _) if !self.params.chunks.is_empty() => Some(self.params.chunks.clone()),
            (_, Some(p)) => chunks_from_el_plan(p),
            _ => None,
        };
        VersionInfo {
            id: self.id.clone(),
            parent_id: self.parent_id.clone(),
            label: self.label.clone(),
            depth: self.depth,
            branch: self.branch.clone(),
            name: self.name.clone(),
            notes: self.notes.clone(),
            favorite: self.favorite,
            created_at: self.created_at.to_rfc3339(),
            status: self.status,
            params: self.params.clone(),
            duration_ms: self.duration_ms,
            estimated_cost: self.estimated_cost,
            charged_cost: self.charged_cost,
            measured_credits: self.measured_credits,
            song_id: self.song_id.clone(),
            plan,
            song_metadata: self.song_metadata.clone(),
            audio_url: self.audio_file.as_ref().map(|_| format!("/api/versions/{}/audio", self.id)),
        }
    }
}

impl Ledger {
    pub fn entry(&self) -> LedgerEntry {
        LedgerEntry {
            id: self.id.clone(),
            at: self.at.to_rfc3339(),
            kind: self.kind,
            track_title: self.track_title.clone(),
            version_label: self.version_label.clone(),
            seconds: self.seconds,
            estimated_cost: self.estimated_cost,
            measured_credits: self.measured_credits,
            cost: self.cost,
            note: self.note.clone(),
            pending: self.pending,
        }
    }
}

pub struct Store {
    pub dir: PathBuf,
    file: PathBuf,
}

impl Store {
    pub fn open(dir: &Path) -> Result<(Store, Db)> {
        std::fs::create_dir_all(dir.join("audio")).with_context(|| format!("creating {}", dir.display()))?;
        let file = dir.join("db.json");
        let db = if file.exists() {
            let raw = std::fs::read_to_string(&file).with_context(|| format!("reading {}", file.display()))?;
            serde_json::from_str(&raw).with_context(|| format!("parsing {}", file.display()))?
        } else {
            Db::default()
        };
        Ok((Store { dir: dir.to_path_buf(), file }, db))
    }

    pub fn audio_path(&self, file: &str) -> PathBuf {
        self.dir.join("audio").join(file)
    }

    /// Atomically write the whole database.
    pub fn save(&self, db: &Db) -> Result<()> {
        let tmp = self.file.with_extension("json.tmp");
        let raw = serde_json::to_vec_pretty(db)?;
        std::fs::write(&tmp, raw)?;
        std::fs::rename(&tmp, &self.file)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track() -> Track {
        Track {
            id: "t".into(),
            title: "t".into(),
            emoji: String::new(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            versions: vec![],
            next_branch: 0,
            last_error: None,
        }
    }

    fn add(t: &mut Track, parent: Option<&str>) -> String {
        let (depth, branch, label) = t.allocate_label(parent);
        let id = format!("id-{label}");
        t.versions.push(Version {
            id: id.clone(),
            parent_id: parent.map(|s| s.to_string()),
            label: label.clone(),
            depth,
            branch,
            name: String::new(),
            notes: String::new(),
            favorite: false,
            created_at: Utc::now(),
            status: VersionStatus::Ready,
            params: GenParams::default(),
            duration_ms: 0,
            estimated_cost: 0.0,
            charged_cost: None,
            measured_credits: None,
            song_id: None,
            el_plan: None,
            song_metadata: None,
            audio_file: None,
            content_type: None,
        });
        label
    }

    #[test]
    fn labels_follow_branching_rules() {
        let mut t = track();
        assert_eq!(add(&mut t, None), "v0");
        assert_eq!(add(&mut t, Some("id-v0")), "v1");
        // Back to v0 and change something else: a new branch.
        assert_eq!(add(&mut t, Some("id-v0")), "v1.a");
        assert_eq!(add(&mut t, Some("id-v1.a")), "v2.a");
        assert_eq!(add(&mut t, Some("id-v1")), "v2");
        assert_eq!(add(&mut t, Some("id-v1")), "v2.b");
        assert_eq!(add(&mut t, Some("id-v0")), "v1.c");
        // A second fresh start inside the same track.
        assert_eq!(add(&mut t, None), "v0.d");
        let labels: std::collections::HashSet<_> = t.versions.iter().map(|v| v.label.clone()).collect();
        assert_eq!(labels.len(), t.versions.len());
    }

    #[test]
    fn subtree_collects_descendants() {
        let mut t = track();
        add(&mut t, None);
        add(&mut t, Some("id-v0"));
        add(&mut t, Some("id-v1"));
        add(&mut t, Some("id-v0"));
        let mut s = t.subtree_ids("id-v1");
        s.sort();
        assert_eq!(s, vec!["id-v1", "id-v2"]);
    }

    #[test]
    fn branch_names() {
        assert_eq!(branch_name(0), "a");
        assert_eq!(branch_name(25), "z");
        assert_eq!(branch_name(26), "aa");
        assert_eq!(branch_name(27), "ab");
    }
}
