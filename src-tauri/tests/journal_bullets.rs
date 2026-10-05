use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use twapp_lib::cli::transcript::TranscriptRoots;
use twapp_lib::journal::{digest::Digest, period, render, store};
use twapp_lib::summary::{RunOutput, Runner};

struct NoRewrite;
impl Runner for NoRewrite {
    fn run(&self, _: &str, _: &str, _: &str) -> Result<RunOutput, String> {
        panic!("changing presentation must not regenerate a saved summary")
    }
}

#[test]
fn legacy_entries_get_bullets_without_writes_or_cache_invalidation() {
    let root = std::env::temp_dir().join(format!("twapp-journal-bullets-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(root.join("days")).unwrap();
    std::fs::create_dir_all(root.join("periods")).unwrap();
    let day_path = root.join("days/2026-09-28.json");
    let day_bytes = include_bytes!("fixtures/journal/day-legacy.json");
    std::fs::write(&day_path, day_bytes).unwrap();
    let ctx = store::Context {
        root: root.clone(), sources: vec![], efforts: HashMap::new(),
        transcripts: TranscriptRoots { claude_projects: root.join("projects"), codex_history: root.join("history.jsonl") },
    };
    let day = store::build_day(&ctx, "2026-09-28".parse().unwrap(), Some(&NoRewrite), false).unwrap().unwrap();
    let expected: Vec<String> = serde_json::from_str(include_str!("fixtures/journal/legacy-bullets.json")).unwrap();
    assert_eq!(day.digest.as_ref().unwrap().bullets, expected);
    assert_eq!(std::fs::read(&day_path).unwrap(), day_bytes);

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    include_str!("fixtures/journal/period-input-legacy.json").trim().hash(&mut hasher);
    let mut saved: serde_json::Value = serde_json::from_str(include_str!("fixtures/journal/period-legacy.json")).unwrap();
    saved["inputs_hash"] = format!("{:016x}", hasher.finish()).into();
    let period_path = root.join("periods/2026-W40.json");
    let period_bytes = serde_json::to_vec(&saved).unwrap();
    std::fs::write(&period_path, &period_bytes).unwrap();
    let target = period::Period::parse("2026-W40", "2026-10-05".parse().unwrap()).unwrap();
    let record = period::build_period(&ctx, &target, Some(&NoRewrite), false).unwrap();
    assert_eq!(record.generated_at, "2026-10-05T10:00:00Z");
    assert_eq!(record.digest.as_ref().unwrap().bullets, expected);
    assert_eq!(std::fs::read(&period_path).unwrap(), period_bytes);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn default_cli_rendering_is_bullets_and_exports_keep_both_forms() {
    let record: store::DayRecord = serde_json::from_str(include_str!("fixtures/journal/day-bullets.json")).unwrap();
    let paragraph = &record.digest.as_ref().unwrap().overview;
    let bullets = render::day_markdown_with_style(&record, render::SummaryStyle::Bullets);
    assert!(bullets.contains("- Fixed duplicate imports; review pending, not deployed."));
    assert!(!bullets.contains(paragraph));
    assert!(!bullets.contains("## Import reliability") && !bullets.contains("## Sessions"));
    let prose = render::day_markdown_with_style(&record, render::SummaryStyle::Paragraph);
    assert!(prose.contains(paragraph));
    assert!(prose.contains("## Import reliability") && prose.contains("## Sessions"));
    assert!(!prose.contains("- Fixed duplicate imports; review pending, not deployed."));
    let exported = render::day_markdown(&record);
    assert!(exported.contains(paragraph) && exported.contains("## Paragraph"));
    assert!(exported.contains("## Import reliability"));
    assert!(exported.contains("## Sessions"));
    let mut period: period::PeriodRecord = serde_json::from_str(include_str!("fixtures/journal/period-legacy.json")).unwrap();
    period.digest = record.digest.clone();
    assert!(render::period_markdown(&period).contains("## Paragraph"));
    assert!(!render::period_markdown_with_style(&period, render::SummaryStyle::Bullets).contains(paragraph));
}

#[test]
fn a_legacy_digest_without_efforts_falls_back_to_its_headline() {
    let mut digest: Digest = serde_json::from_str(include_str!("fixtures/journal/digest-legacy.json")).unwrap();
    digest.efforts.clear();
    digest.ensure_bullets();
    assert_eq!(digest.bullets, [digest.headline]);
}
