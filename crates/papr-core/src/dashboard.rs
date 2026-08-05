#![allow(missing_docs)]
use std::collections::HashMap;
use sha2::{Digest, Sha256};
use crate::models::RemotePaper;

/// A SHA-256 rank gives every paper an equal chance of every position and
/// includes the date, so the selection changes each day. This avoids any
/// dependence on arXiv response order, database iteration, or process-local
/// RNG state. The paper ID only resolves the cryptographically negligible case
/// of two equal ranks.
pub fn shuffle_daily_bucket(papers: &mut [RemotePaper], feed_date: &str, keyword: &str) {
    papers.sort_by_cached_key(|paper| (daily_paper_rank(feed_date, keyword, &paper.id), paper.id.clone()));
}

pub fn keyword_terms(keyword: &str) -> Vec<String> {
    keyword
        .split(|character: char| !character.is_alphanumeric())
        .filter(|term| !term.is_empty())
        .map(str::to_lowercase)
        .collect()
}

pub fn title_match_strength(title: &str, terms: &[String]) -> usize {
    let title = title.to_lowercase();
    terms.iter().filter(|term| title.contains(term.as_str())).count()
}

#[derive(Debug)]
pub struct DashboardCandidate {
    paper: RemotePaper,
    matches: Vec<KeywordMatch>,
    daily_rank: [u8; 32],
}

#[derive(Debug)]
pub struct KeywordMatch {
    keyword_index: usize,
    title_term_matches: usize,
    full_title_match: bool,
}

/// Select a balanced, relevance-ranked daily dashboard feed.
///
/// Each keyword receives a gentle, position-weighted representation target.
/// Greedy selection maximizes coverage of still-unmet targets, then uses a
/// deterministic daily rank to rotate papers. A selected paper counts toward
/// every keyword it matches, so cross-keyword papers are boosted rather than
/// attributed to whichever bucket happened to be visited first. Title matches
/// remain a tie-breaker after daily rotation.
pub fn select_dashboard_papers(
    buckets: Vec<(String, Vec<RemotePaper>)>,
    limit: usize,
    feed_date: &str,
) -> Vec<RemotePaper> {
    let keywords: Vec<_> = buckets
        .iter()
        .map(|(keyword, _)| keyword.clone())
        .collect();
    let terms: Vec<_> = keywords.iter().map(|keyword| keyword_terms(keyword)).collect();
    let mut candidates = Vec::<DashboardCandidate>::new();
    let mut candidate_indexes = HashMap::<String, usize>::new();
    let mut available_by_keyword = vec![0_usize; buckets.len()];

    for (keyword_index, (_, papers)) in buckets.into_iter().enumerate() {
        for paper in papers {
            let title_term_matches = title_match_strength(&paper.title, &terms[keyword_index]);
            let full_title_match = !terms[keyword_index].is_empty()
                && title_term_matches == terms[keyword_index].len();
            let keyword_match = KeywordMatch {
                keyword_index,
                title_term_matches,
                full_title_match,
            };
            if let Some(&candidate_index) = candidate_indexes.get(&paper.id) {
                if !candidates[candidate_index]
                    .matches
                    .iter()
                    .any(|matched| matched.keyword_index == keyword_index)
                {
                    available_by_keyword[keyword_index] += 1;
                    candidates[candidate_index].matches.push(keyword_match);
                }
            } else {
                let candidate_index = candidates.len();
                candidate_indexes.insert(paper.id.clone(), candidate_index);
                available_by_keyword[keyword_index] += 1;
                candidates.push(DashboardCandidate {
                    daily_rank: daily_paper_rank(feed_date, "dashboard", &paper.id),
                    paper,
                    matches: vec![keyword_match],
                });
            }
        }
    }

    let targets = keyword_representation_targets(&available_by_keyword, &keywords, limit, feed_date);
    let keyword_weights = keyword_priority_weights(&available_by_keyword);
    let mut represented = vec![0_usize; targets.len()];
    let mut selected = Vec::with_capacity(limit.min(candidates.len()));

    while selected.len() < limit && !candidates.is_empty() {
        let best_index = candidates
            .iter()
            .enumerate()
            .max_by_key(|(_, candidate)| {
                let coverage_keywords = candidate
                    .matches
                    .iter()
                    .filter(|matched| represented[matched.keyword_index] < targets[matched.keyword_index])
                    .count();
                let weighted_coverage = candidate
                    .matches
                    .iter()
                    .filter_map(|matched| {
                        let deficit = targets[matched.keyword_index]
                            .saturating_sub(represented[matched.keyword_index]);
                        (deficit > 0).then_some(deficit * keyword_weights[matched.keyword_index])
                    })
                    .sum::<usize>();
                let full_title_matches = candidate
                    .matches
                    .iter()
                    .filter(|matched| matched.full_title_match)
                    .count();
                let title_term_matches = candidate
                    .matches
                    .iter()
                    .map(|matched| matched.title_term_matches)
                    .sum::<usize>();
                (
                    coverage_keywords,
                    candidate.matches.len(),
                    weighted_coverage,
                    candidate.daily_rank,
                    full_title_matches,
                    title_term_matches,
                    candidate.paper.id.as_str(),
                )
            })
            .map(|(index, _)| index);
        let Some(best_index) = best_index else {
            break;
        };
        let candidate = candidates.swap_remove(best_index);
        for matched in &candidate.matches {
            represented[matched.keyword_index] += 1;
        }
        selected.push(candidate.paper);
    }
    selected
}

pub fn keyword_priority_weights(available_by_keyword: &[usize]) -> Vec<usize> {
    let active = available_by_keyword.iter().filter(|&&count| count > 0).count();
    let mut active_rank = 0_usize;
    available_by_keyword
        .iter()
        .map(|&available| {
            if available == 0 {
                0
            } else {
                // The range is intentionally narrow (at most ~10%) so keyword
                // order is a preference, not a monopoly.
                let weight = active * 10 + active.saturating_sub(active_rank + 1);
                active_rank += 1;
                weight
            }
        })
        .collect()
}

pub fn keyword_representation_targets(
    available_by_keyword: &[usize],
    keywords: &[String],
    limit: usize,
    feed_date: &str,
) -> Vec<usize> {
    let active: Vec<_> = available_by_keyword
        .iter()
        .enumerate()
        .filter_map(|(index, &available)| (available > 0).then_some(index))
        .collect();
    let mut targets = vec![0_usize; available_by_keyword.len()];
    if active.len() > limit {
        let weights = keyword_priority_weights(available_by_keyword);
        let mut weighted_window: Vec<_> = active
            .iter()
            .map(|&index| {
                let rank = daily_paper_rank(feed_date, "keyword-window", &keywords[index]);
                let mut bytes = [0_u8; 8];
                bytes.copy_from_slice(&rank[..8]);
                let uniform = (u64::from_be_bytes(bytes) as f64 / u64::MAX as f64)
                    .max(f64::MIN_POSITIVE);
                // Efraimidis-Spirakis weighted sampling: higher-priority
                // keywords have a slightly better daily chance of appearing.
                (uniform.powf(1.0 / weights[index] as f64), index)
            })
            .collect();
        weighted_window.sort_by(|left, right| right.0.total_cmp(&left.0));
        for (_, index) in weighted_window.into_iter().take(limit) {
            targets[index] = 1;
        }
        return targets;
    }
    for &index in active.iter().take(limit) {
        targets[index] = 1;
    }
    let remaining = limit.saturating_sub(active.len().min(limit));
    if remaining == 0 || active.is_empty() {
        return targets;
    }

    let weights = keyword_priority_weights(available_by_keyword);
    let weight_total: usize = active.iter().map(|&index| weights[index]).sum();
    let mut remainders = Vec::with_capacity(active.len());
    let mut assigned_extra = 0_usize;
    for &index in &active {
        let numerator = remaining * weights[index];
        let allocation = numerator / weight_total;
        targets[index] += allocation;
        assigned_extra += allocation;
        remainders.push((numerator % weight_total, index));
    }
    remainders.sort_by(|(left_remainder, left_index), (right_remainder, right_index)| {
        right_remainder
            .cmp(left_remainder)
            .then_with(|| left_index.cmp(right_index))
    });
    for (_, index) in remainders
        .into_iter()
        .take(remaining.saturating_sub(assigned_extra))
    {
        targets[index] += 1;
    }
    targets
}

pub fn daily_paper_rank(feed_date: &str, keyword: &str, paper_id: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"papr-dashboard-feed-v1\0");
    for value in [feed_date, keyword, paper_id] {
        hasher.update((value.len() as u64).to_be_bytes());
        hasher.update(value.as_bytes());
    }
    hasher.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn remote_paper(id: &str, title: &str) -> RemotePaper {
        let timestamp = Utc
            .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
            .single()
            .unwrap_or_else(Utc::now);
        RemotePaper {
            id: id.into(),
            title: title.into(),
            authors: vec!["Researcher".into()],
            abstract_text: String::new(),
            published: timestamp,
            updated: timestamp,
            categories: vec!["cs.DL".into()],
            pdf_url: None,
            doi: None,
            journal_ref: None,
        }
    }

    #[test]
    fn daily_dashboard_permutation_is_input_order_independent_and_changes_by_date() {
        let papers: Vec<_> = (0..30)
            .map(|index| remote_paper(&format!("https://arxiv.org/abs/{index}"), "Paper"))
            .collect();
        let mut reordered = papers.clone();
        reordered.reverse();

        shuffle_daily_bucket(&mut reordered, "2026-07-19", "quantum gravity");
        let mut same_day = papers.clone();
        shuffle_daily_bucket(&mut same_day, "2026-07-19", "quantum gravity");
        assert_eq!(reordered, same_day);

        let mut next_day = papers;
        shuffle_daily_bucket(&mut next_day, "2026-07-20", "quantum gravity");
        assert_ne!(
            same_day.iter().take(10).map(|paper| &paper.id).collect::<Vec<_>>(),
            next_day.iter().take(10).map(|paper| &paper.id).collect::<Vec<_>>(),
        );
    }
    #[test]
    fn keyword_dashboard_feed_rotates_selected_papers_each_day() {
        let papers = (0..30)
            .map(|index| remote_paper(&format!("https://arxiv.org/abs/{index}"), "Quantum result"))
            .collect::<Vec<_>>();

        let first_day = select_dashboard_papers(
            vec![("quantum".into(), papers.clone())],
            10,
            "2026-07-19",
        );
        let next_day = select_dashboard_papers(
            vec![("quantum".into(), papers)],
            10,
            "2026-07-20",
        );

        assert_ne!(
            first_day.iter().map(|paper| &paper.id).collect::<Vec<_>>(),
            next_day.iter().map(|paper| &paper.id).collect::<Vec<_>>(),
        );
    }
    #[test]
    fn dashboard_prioritizes_papers_with_all_keyword_terms_in_the_title() {
        let papers = vec![
            remote_paper("https://arxiv.org/abs/abstract", "A related result"),
            remote_paper("https://arxiv.org/abs/title-one", "Quantum gravity constraints"),
            remote_paper("https://arxiv.org/abs/title-two", "Gravity in quantum systems"),
        ];

        let selected = select_dashboard_papers(
            vec![("quantum gravity".into(), papers)],
            10,
            "2026-07-19",
        );

        assert!(selected[0].title.to_lowercase().contains("quantum"));
        assert!(selected[0].title.to_lowercase().contains("gravity"));
        assert!(selected[1].title.to_lowercase().contains("quantum"));
        assert!(selected[1].title.to_lowercase().contains("gravity"));
        assert_eq!(selected[2].id, "https://arxiv.org/abs/abstract");
    }
    #[test]
    fn dashboard_represents_keywords_even_when_only_one_has_a_title_match() {
        let selected = select_dashboard_papers(
            vec![
                (
                    "neural networks".into(),
                    vec![remote_paper("https://arxiv.org/abs/abstract", "A related result")],
                ),
                (
                    "quantum gravity".into(),
                    vec![remote_paper(
                        "https://arxiv.org/abs/title",
                        "Quantum gravity constraints",
                    )],
                ),
            ],
            10,
            "2026-07-19",
        );

        assert_eq!(selected.len(), 2);
        assert!(selected.iter().any(|paper| paper.id == "https://arxiv.org/abs/title"));
        assert!(selected
            .iter()
            .any(|paper| paper.id == "https://arxiv.org/abs/abstract"));
    }
    #[test]
    fn dashboard_balances_keyword_targets_before_title_quality() {
        let first_keyword: Vec<_> = (0..10)
            .map(|index| remote_paper(&format!("https://arxiv.org/abs/alpha-{index}"), "Alpha"))
            .collect();
        let second_keyword: Vec<_> = (0..10)
            .map(|index| remote_paper(&format!("https://arxiv.org/abs/beta-{index}"), "Related work"))
            .collect();
        let selected = select_dashboard_papers(
            vec![("alpha".into(), first_keyword), ("beta".into(), second_keyword)],
            10,
            "2026-07-19",
        );

        let first_count = selected
            .iter()
            .filter(|paper| paper.id.contains("alpha-"))
            .count();
        assert_eq!(first_count, 5);
        assert_eq!(selected.len() - first_count, 5);
    }
    #[test]
    fn dashboard_keyword_targets_are_balanced_with_a_gentle_earlier_preference() {
        let keywords = |count| (0..count).map(|index| format!("keyword {index}")).collect::<Vec<_>>();
        assert_eq!(
            keyword_representation_targets(&[50, 50], &keywords(2), 10, "2026-07-19"),
            vec![5, 5]
        );
        assert_eq!(
            keyword_representation_targets(&[50, 50, 50], &keywords(3), 10, "2026-07-19"),
            vec![4, 3, 3]
        );
        assert_eq!(
            keyword_representation_targets(&[50, 50, 50, 50], &keywords(4), 10, "2026-07-19"),
            vec![3, 3, 2, 2]
        );
        assert_eq!(
            keyword_representation_targets(&[50, 0, 50], &keywords(3), 10, "2026-07-19"),
            vec![5, 0, 5]
        );
    }
    #[test]
    fn dashboard_many_keywords_uses_a_daily_weighted_window() {
        let keywords = (0..15)
            .map(|index| format!("keyword {index}"))
            .collect::<Vec<_>>();
        let targets = keyword_representation_targets(&[50; 15], &keywords, 10, "2026-07-19");

        assert_eq!(targets.iter().sum::<usize>(), 10);
        assert_eq!(targets.iter().filter(|&&target| target == 1).count(), 10);
    }
    #[test]
    fn dashboard_reallocates_when_a_keyword_runs_out_of_candidates() {
        let second_keyword: Vec<_> = (0..20)
            .map(|index| remote_paper(&format!("https://arxiv.org/abs/beta-{index}"), "Beta"))
            .collect();
        let selected = select_dashboard_papers(
            vec![
                (
                    "alpha".into(),
                    vec![remote_paper("https://arxiv.org/abs/alpha-only", "Alpha")],
                ),
                ("beta".into(), second_keyword),
            ],
            10,
            "2026-07-19",
        );

        assert_eq!(selected.len(), 10);
        assert_eq!(
            selected
                .iter()
                .filter(|paper| paper.id == "https://arxiv.org/abs/alpha-only")
                .count(),
            1
        );
    }
    #[test]
    fn a_multi_keyword_paper_counts_once_toward_each_target() {
        let shared = remote_paper("https://arxiv.org/abs/shared", "Alpha beta gamma");
        let bucket = |keyword: &str, count: usize| {
            (0..count)
                .map(|index| {
                    remote_paper(
                        &format!("https://arxiv.org/abs/{keyword}-{index}"),
                        keyword,
                    )
                })
                .collect::<Vec<_>>()
        };
        let mut alpha = bucket("alpha", 4);
        let mut beta = bucket("beta", 3);
        let mut gamma = bucket("gamma", 3);
        alpha.push(shared.clone());
        beta.push(shared.clone());
        gamma.push(shared);

        let selected = select_dashboard_papers(
            vec![
                ("alpha".into(), alpha),
                ("beta".into(), beta),
                ("gamma".into(), gamma),
            ],
            10,
            "2026-07-19",
        );

        assert_eq!(selected[0].id, "https://arxiv.org/abs/shared");
        assert!(selected.iter().filter(|paper| paper.id.contains("beta-")).count() >= 2);
        assert!(selected.iter().filter(|paper| paper.id.contains("gamma-")).count() >= 2);
    }
    #[test]
    fn dashboard_boosts_and_deduplicates_multi_keyword_matches() {
        let shared = remote_paper("https://arxiv.org/abs/shared", "Alpha beta methods");
        let selected = select_dashboard_papers(
            vec![
                (
                    "alpha".into(),
                    vec![remote_paper("https://arxiv.org/abs/alpha", "Alpha result"), shared.clone()],
                ),
                (
                    "beta".into(),
                    vec![remote_paper("https://arxiv.org/abs/beta", "Beta result"), shared],
                ),
            ],
            2,
            "2026-07-19",
        );

        assert_eq!(selected[0].id, "https://arxiv.org/abs/shared");
        assert_eq!(
            selected
                .iter()
                .filter(|paper| paper.id == "https://arxiv.org/abs/shared")
                .count(),
            1
        );
    }
}
