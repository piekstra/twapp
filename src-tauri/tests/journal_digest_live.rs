use twapp_lib::journal::{digest::{day_digest, period_digest, PeriodInput}, store::DayRecord};
use twapp_lib::summary::{HarnessRunner, RunOutput, Runner, SummaryHarness};

struct CheckedWriter(HarnessRunner);
impl Runner for CheckedWriter {
    fn run(&self, system: &str, prompt: &str, input: &str) -> Result<RunOutput, String> {
        let output = self.0.run(system, prompt, input)?;
        let raw = twapp_lib::summary::runner::extract_json_object(&output.text).expect("writer JSON");
        let bullets = raw["bullets"].as_array().expect("writer supplies bullets before fallback");
        assert!(!bullets.is_empty() && bullets.len() <= 5);
        for bullet in bullets {
            let text = bullet.as_str().expect("writer bullet is text");
            assert!(!text.trim().is_empty() && text.chars().count() <= 140);
        }
        assert!(raw["overview"].as_str().is_some_and(|text| !text.trim().is_empty()));
        println!("raw writer: {}", raw);
        Ok(output)
    }
}

#[test]
#[ignore = "requires Claude authentication and consumes usage; uses only synthetic journal facts"]
fn real_journal_writer_produces_bullets_and_paragraph_for_days_and_periods() {
    let facts = serde_json::from_str::<DayRecord>(include_str!("fixtures/journal/day-legacy.json")).unwrap().facts;
    let runner = CheckedWriter(HarnessRunner::new(SummaryHarness::Claude, None, None));
    let day = day_digest(&facts, &runner).unwrap();
    let period = period_digest(&[PeriodInput { label: "Mon Sep 28".into(), digest: &day }], &runner).unwrap();
    for (label, digest) in [("day", day), ("period", period)] {
        assert!(!digest.overview.is_empty());
        assert!(!digest.bullets.is_empty() && digest.bullets.len() <= 5);
        assert!(digest.bullets.iter().all(|b| b.chars().count() <= 140));
        assert!(digest.bullets.join(" ").to_lowercase().contains("review"));
        println!("{}: {}", label, serde_json::to_string(&digest).unwrap());
    }
}
