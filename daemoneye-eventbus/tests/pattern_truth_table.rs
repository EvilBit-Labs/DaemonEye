use daemoneye_eventbus::topic::{Topic, TopicPattern};
use insta::assert_yaml_snapshot;
use serde::Serialize;

#[derive(Debug, Serialize)]
struct Case {
    pattern: String,
    topic: String,
    matches: Option<bool>,
    pattern_error: Option<String>,
    topic_error: Option<String>,
}

fn eval_case(pattern: &str, topic: &str) -> Case {
    // Try to compile the pattern
    match TopicPattern::new(pattern) {
        Err(e) => Case {
            pattern: pattern.to_string(),
            topic: topic.to_string(),
            matches: None,
            pattern_error: Some(e.to_string()),
            topic_error: None,
        },
        Ok(pat) => match Topic::new(topic) {
            Err(e) => Case {
                pattern: pattern.to_string(),
                topic: topic.to_string(),
                matches: None,
                pattern_error: None,
                topic_error: Some(e.to_string()),
            },
            Ok(top) => Case {
                pattern: pattern.to_string(),
                topic: topic.to_string(),
                matches: Some(pat.matches(&top)),
                pattern_error: None,
                topic_error: None,
            },
        },
    }
}

#[test]
fn truth_table() {
    let cases = vec![
        // Single-level wildcard (+) should match exactly one segment
        eval_case("events.process.+", "events.process.new"),
        eval_case("events.process.+", "events.process"),
        // Multi-level wildcard (#) should match zero or more segments, only allowed at end
        eval_case("events.#", "events.process.new"),
        eval_case("events.#", "control.collector.status"),
        // Control plane triggers
        eval_case("control.trigger.+", "control.trigger.request"),
        eval_case("control.trigger.+", "control.trigger"),
        // Exact literal match
        eval_case("events.process.new", "events.process.new"),
        eval_case("events.process.new", "events.process.old"),
        // Another valid single-level wildcard
        eval_case("events.+.new", "events.process.new"),
        // Invalid pattern: # not at end
        eval_case("events.#.process", "events.process.new"),
        // Reserved topic prefix (system) should error on topic creation
        eval_case("events.process.+", "system.health.status"),
        // Matrix to distinguish + vs multiple segments
        eval_case("events.+.+", "events.process.new"),
        eval_case("events.+.+", "events.process.lifecycle"),
        eval_case("events.+.+", "events.process.lifecycle.new"),
        // Multi-wildcard at end with prior + should match long tail
        eval_case("events.+.#", "events.process.lifecycle.new"),
        // Current implementation does not treat # as matching zero segments at end
        eval_case("events.#", "events"),
        eval_case("events.process.new.#", "events.process.new"),
        // Literal '*' inside a segment does not wildcard-match
        eval_case("events.*.new", "events.process.new"),
        // Invalid character in topic (uppercase)
        eval_case("events.process.+", "events.Process.new"),
    ];

    assert_yaml_snapshot!("topic_pattern_matrix_v1", cases);
}
