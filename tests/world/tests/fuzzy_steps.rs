//! Seeded pacing preserves the stepped world's run.
#[path = "steps/support.rs"]
mod support;

#[test]
fn sliced_steps_replay_across_seeds() {
    for seed in 0..1024 {
        support::compare(seed);
    }
}
