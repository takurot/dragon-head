//! Transition model: learns action/state-transition frequencies from session
//! history and blends them with portable domain-pack hints to predict the
//! AI's next action and the resulting state hash.
use std::collections::HashMap;

/// Newtype wrapper around an action name (e.g. `"click:search_button"`).
///
/// Prevents accidental mix-ups with raw state-hash strings at call sites.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ActionSignature(String);

impl ActionSignature {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for ActionSignature {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

impl From<String> for ActionSignature {
    fn from(value: String) -> Self {
        Self(value)
    }
}

/// A portable, cross-session prior describing "after `from_action`, the next
/// action is usually `predicted_next_action`".
///
/// Domain hints inform **action** prediction only — state hashes are
/// page-instance specific and cannot be predicted for unseen pages.
#[derive(Debug, Clone, PartialEq)]
pub struct DomainHint {
    pub from_action: ActionSignature,
    pub predicted_next_action: ActionSignature,
    pub weight: f64,
}

/// Learned frequency tables for action and state-hash prediction.
///
/// `state_transitions` is session-local (state hashes are page-instance
/// specific and not portable across sessions). `action_sequences` describes
/// action-to-action transitions and is portable — it is the table exported
/// via FlatBuffers in [`super::codec`].
#[derive(Debug, Clone, Default)]
pub struct TransitionModel {
    state_transitions: HashMap<(String, ActionSignature), HashMap<String, u32>>,
    action_sequences: HashMap<ActionSignature, HashMap<ActionSignature, u32>>,
}

impl TransitionModel {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record an observed transition: from `from_state_hash`, taking `action`
    /// led to `to_state_hash`. If `prior_action` is known, also records the
    /// action-to-action sequence `prior_action -> action`.
    pub fn record(
        &mut self,
        from_state_hash: &str,
        action: &ActionSignature,
        to_state_hash: &str,
        prior_action: Option<&ActionSignature>,
    ) {
        *self
            .state_transitions
            .entry((from_state_hash.to_string(), action.clone()))
            .or_default()
            .entry(to_state_hash.to_string())
            .or_insert(0) += 1;

        if let Some(prior) = prior_action {
            *self
                .action_sequences
                .entry(prior.clone())
                .or_default()
                .entry(action.clone())
                .or_insert(0) += 1;
        }
    }

    /// Predict the next action given the last action taken, blending observed
    /// session data with portable `domain_hints`.
    ///
    /// Observed data dominates once present; hints provide a prior for cold
    /// start (no session data yet for `last_action`). Returns the predicted
    /// action with a frequency-ratio confidence in `0.0..=1.0`.
    pub fn predict_action(
        &self,
        last_action: Option<&ActionSignature>,
        domain_hints: &[DomainHint],
    ) -> Option<(ActionSignature, f64)> {
        let last_action = last_action?;

        if let Some(observed) = self.action_sequences.get(last_action) {
            if !observed.is_empty() {
                return Some(most_frequent(observed));
            }
        }

        domain_hints
            .iter()
            .filter(|hint| &hint.from_action == last_action)
            .max_by(|a, b| a.weight.total_cmp(&b.weight))
            .map(|hint| {
                (
                    hint.predicted_next_action.clone(),
                    hint.weight.clamp(0.0, 1.0),
                )
            })
    }

    /// Predict the resulting state hash given the current state hash and the
    /// action about to be taken. Session-local only — state hashes cannot be
    /// predicted for transitions never observed in this session.
    pub fn predict_state(
        &self,
        current_state_hash: &str,
        action: &ActionSignature,
    ) -> Option<(String, f64)> {
        let observed = self
            .state_transitions
            .get(&(current_state_hash.to_string(), action.clone()))?;
        if observed.is_empty() {
            return None;
        }
        Some(most_frequent(observed))
    }

    /// Snapshot the portable action-sequence table as `(from, to, count)`
    /// records, suitable for FlatBuffers export via [`super::codec`].
    pub fn action_sequence_records(&self) -> Vec<(ActionSignature, ActionSignature, u32)> {
        let mut records: Vec<_> = self
            .action_sequences
            .iter()
            .flat_map(|(from, tos)| {
                tos.iter()
                    .map(move |(to, count)| (from.clone(), to.clone(), *count))
            })
            .collect();
        records.sort();
        records
    }

    /// Merge imported `(from, to, count)` records into the action-sequence
    /// table, summing counts for records that already exist.
    pub fn merge_action_sequences(
        &mut self,
        records: Vec<(ActionSignature, ActionSignature, u32)>,
    ) {
        for (from, to, count) in records {
            *self
                .action_sequences
                .entry(from)
                .or_default()
                .entry(to)
                .or_insert(0) += count;
        }
    }
}

/// Pick the entry with the highest count, returning it alongside its
/// frequency ratio (count / total) as a confidence value. Ties broken by key
/// for determinism.
fn most_frequent<K: Clone + Ord>(counts: &HashMap<K, u32>) -> (K, f64) {
    let total: u32 = counts.values().sum();
    let (key, count) = counts
        .iter()
        .max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0)))
        .expect("counts is non-empty");
    (key.clone(), f64::from(*count) / f64::from(total))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn action(name: &str) -> ActionSignature {
        ActionSignature::from(name)
    }

    #[test]
    fn predicts_action_returns_none_when_nothing_recorded_and_no_hints() {
        let model = TransitionModel::new();
        assert_eq!(
            model.predict_action(Some(&action("click:search")), &[]),
            None
        );
    }

    #[test]
    fn predicts_action_returns_none_without_a_last_action() {
        let model = TransitionModel::new();
        assert_eq!(model.predict_action(None, &[]), None);
    }

    #[test]
    fn predicts_most_frequent_observed_action_with_confidence_ratio() {
        let mut model = TransitionModel::new();
        let click = action("click:search_button");
        let type_input = action("type:search_input");
        let submit = action("click:submit");

        // 3 occurrences of click -> type, 1 of click -> submit
        model.record("hash_a", &type_input, "hash_b", Some(&click));
        model.record("hash_a", &type_input, "hash_b", Some(&click));
        model.record("hash_a", &type_input, "hash_b", Some(&click));
        model.record("hash_a", &submit, "hash_c", Some(&click));

        let (predicted, confidence) = model.predict_action(Some(&click), &[]).unwrap();
        assert_eq!(predicted, type_input);
        assert_eq!(confidence, 0.75);
    }

    #[test]
    fn domain_hint_provides_prior_when_no_session_data_recorded() {
        let model = TransitionModel::new();
        let click = action("click:search_button");
        let type_input = action("type:search_input");
        let hints = vec![DomainHint {
            from_action: click.clone(),
            predicted_next_action: type_input.clone(),
            weight: 0.6,
        }];

        let (predicted, confidence) = model.predict_action(Some(&click), &hints).unwrap();
        assert_eq!(predicted, type_input);
        assert_eq!(confidence, 0.6);
    }

    #[test]
    fn observed_data_dominates_once_session_data_exists() {
        let mut model = TransitionModel::new();
        let click = action("click:search_button");
        let type_input = action("type:search_input");
        let submit = action("click:submit");

        // Hint says "submit" follows "click", but session observes "type" twice.
        model.record("hash_a", &type_input, "hash_b", Some(&click));
        model.record("hash_a", &type_input, "hash_b", Some(&click));

        let hints = vec![DomainHint {
            from_action: click.clone(),
            predicted_next_action: submit.clone(),
            weight: 0.9,
        }];

        let (predicted, confidence) = model.predict_action(Some(&click), &hints).unwrap();
        assert_eq!(predicted, type_input);
        assert_eq!(confidence, 1.0);
    }

    #[test]
    fn predicts_state_returns_none_for_unseen_transition() {
        let model = TransitionModel::new();
        assert_eq!(model.predict_state("hash_a", &action("click:search")), None);
    }

    #[test]
    fn predicts_most_frequent_observed_state_with_confidence_ratio() {
        let mut model = TransitionModel::new();
        let click = action("click:search_button");

        model.record("hash_a", &click, "hash_b", None);
        model.record("hash_a", &click, "hash_b", None);
        model.record("hash_a", &click, "hash_c", None);

        let (predicted, confidence) = model.predict_state("hash_a", &click).unwrap();
        assert_eq!(predicted, "hash_b");
        assert_eq!(confidence, 2.0 / 3.0);
    }

    #[test]
    fn action_sequence_records_round_trip_through_merge() {
        let mut model = TransitionModel::new();
        let click = action("click:search_button");
        let type_input = action("type:search_input");
        model.record("hash_a", &type_input, "hash_b", Some(&click));
        model.record("hash_a", &type_input, "hash_b", Some(&click));

        let records = model.action_sequence_records();
        assert_eq!(records, vec![(click.clone(), type_input.clone(), 2)]);

        let mut imported = TransitionModel::new();
        imported.merge_action_sequences(records);
        assert_eq!(
            imported.action_sequence_records(),
            vec![(click, type_input, 2)]
        );
    }

    #[test]
    fn merge_action_sequences_sums_counts_for_existing_records() {
        let click = action("click:search_button");
        let type_input = action("type:search_input");

        let mut model = TransitionModel::new();
        model.merge_action_sequences(vec![(click.clone(), type_input.clone(), 2)]);
        model.merge_action_sequences(vec![(click.clone(), type_input.clone(), 3)]);

        assert_eq!(
            model.action_sequence_records(),
            vec![(click, type_input, 5)]
        );
    }

    #[test]
    fn action_signature_converts_from_str_and_string() {
        let from_str: ActionSignature = "click:button".into();
        let from_string: ActionSignature = String::from("click:button").into();
        assert_eq!(from_str, from_string);
        assert_eq!(from_str.as_str(), "click:button");
    }
}
