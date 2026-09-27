/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Play counts, kept in `library/playcounts.tsv` (`id  count` per line).
//!
//! Song Summoner reads `MPMediaItemPropertyPlayCount` to award listening
//! points, so a song counts as played once our player gets through half of
//! it or four minutes, whichever comes first, like iTunes does.

use std::collections::HashMap;
use std::sync::Mutex;

static COUNTS: Mutex<Option<HashMap<u64, u32>>> = Mutex::new(None);

/// Seconds after which a play counts, for a song of `duration` seconds.
pub fn threshold(duration: f64) -> f64 {
    if duration > 0.0 {
        (duration * 0.5).min(240.0)
    } else {
        240.0
    }
}

pub fn should_count(position: f64, duration: f64) -> bool {
    position >= threshold(duration)
}

pub fn parse(text: &str) -> HashMap<u64, u32> {
    text.lines()
        .filter_map(|line| {
            let (id, count) = line.split_once('\t')?;
            Some((id.trim().parse().ok()?, count.trim().parse().ok()?))
        })
        .collect()
}

pub fn serialize(counts: &HashMap<u64, u32>) -> String {
    let mut ids: Vec<&u64> = counts.keys().collect();
    ids.sort();
    ids.into_iter()
        .map(|id| format!("{}\t{}\n", id, counts[id]))
        .collect()
}

fn with_counts<R>(f: impl FnOnce(&mut HashMap<u64, u32>) -> R) -> R {
    let mut guard = COUNTS.lock().unwrap();
    let counts = guard.get_or_insert_with(|| {
        std::fs::read_to_string(super::playcounts_path())
            .map(|text| parse(&text))
            .unwrap_or_default()
    });
    f(counts)
}

pub fn get(id: u64) -> u32 {
    with_counts(|counts| counts.get(&id).copied().unwrap_or(0))
}

/// Add one play and save the file straight away, so a crash or a killed
/// app doesn't lose it.
pub fn increment(id: u64) {
    let text = with_counts(|counts| {
        *counts.entry(id).or_insert(0) += 1;
        log!("media: play counted for {:016X}, now {}", id, counts[&id]);
        serialize(counts)
    });
    let path = super::playcounts_path();
    let tmp = path.with_extension("tsv.tmp");
    let result = std::fs::create_dir_all(super::library_dir())
        .and_then(|_| std::fs::write(&tmp, text))
        .and_then(|_| std::fs::rename(&tmp, &path));
    if let Err(e) = result {
        log!("media: couldn't save {}: {}", path.display(), e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_the_song_or_four_minutes() {
        assert_eq!(threshold(200.0), 100.0);
        assert_eq!(threshold(600.0), 240.0);
        assert_eq!(threshold(0.0), 240.0);
        assert!(!should_count(99.9, 200.0));
        assert!(should_count(100.0, 200.0));
        assert!(!should_count(239.0, 900.0));
        assert!(should_count(240.0, 900.0));
    }

    #[test]
    fn file_round_trips() {
        let mut counts = HashMap::new();
        counts.insert(18_446_744_073_709_551_615u64, 3);
        counts.insert(1, 12);
        let text = serialize(&counts);
        assert_eq!(text, "1\t12\n18446744073709551615\t3\n");
        assert_eq!(parse(&text), counts);
        // Junk lines are ignored.
        assert_eq!(parse("junk\n5\tx\n7\t2\r\n").get(&7), Some(&2));
        assert_eq!(parse("junk\n5\tx\n7\t2\r\n").len(), 1);
    }
}
