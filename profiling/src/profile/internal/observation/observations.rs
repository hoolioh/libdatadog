// Unless explicitly stated otherwise all files in this repository are licensed under the Apache License Version 2.0.
// This product includes software developed at Datadog (https://www.datadoghq.com/). Copyright 2021-Present Datadog, Inc.

//! See the mod.rs file comment for why this module and file exists.

use super::super::SampleId;
use super::trimmed_observation::{ObservationLength, TrimmedObservation};
use crate::profile::Timestamp;
use std::collections::HashMap;

struct NonEmptyObservations {
    aggregated_data: HashMap<SampleId, TrimmedObservation>,
    timestamped_data: Vec<TrimmedTimestampedObservation>,
    obs_len: ObservationLength,
}

type TrimmedTimestampedObservation = (SampleId, Timestamp, TrimmedObservation);

#[derive(Default)]
pub struct Observations {
    inner: Option<NonEmptyObservations>,
}

/// Public API
impl Observations {
    pub fn add(&mut self, sample_id: SampleId, timestamp: Option<Timestamp>, values: Vec<i64>) {
        if let Some(inner) = &self.inner {
            inner.obs_len.assert_eq(values.len());
        } else {
            self.inner = Some(NonEmptyObservations {
                aggregated_data: Default::default(),
                timestamped_data: vec![],
                obs_len: ObservationLength::new(values.len()),
            });
        };

        // SAFETY: we just ensured it has an item above.
        let observations = unsafe { self.inner.as_mut().unwrap_unchecked() };
        let obs_len = observations.obs_len;

        if let Some(ts) = timestamp {
            let trimmed = TrimmedObservation::new(values, obs_len);
            observations.timestamped_data.push((sample_id, ts, trimmed));
        } else if let Some(v) = observations.aggregated_data.get_mut(&sample_id) {
            // SAFETY: This method is only way to build one of these, and at
            // the top we already checked the length matches.
            unsafe { v.as_mut_slice(obs_len) }
                .iter_mut()
                .zip(values)
                .for_each(|(a, b)| *a += b);
        } else {
            let trimmed = TrimmedObservation::new(values, obs_len);
            observations.aggregated_data.insert(sample_id, trimmed);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_none()
    }

    pub fn iter(&self) -> impl Iterator<Item = (SampleId, Option<Timestamp>, &[i64])> {
        self.inner.iter().flat_map(|observations| {
            let obs_len = observations.obs_len;
            let aggregated_data = observations
                .aggregated_data
                .iter()
                .map(move |(id, obs)| (*id, None, obs));
            let timestamped_data = observations
                .timestamped_data
                .iter()
                .map(move |(id, ts, obs)| (*id, Some(*ts), obs));
            aggregated_data
                .chain(timestamped_data)
                .map(move |(id, ts, obs)| {
                    // SAFETY: The only way to build one of these is through
                    // [Self::add], which already checked that the length was correct.
                    (id, ts, unsafe { obs.as_slice(obs_len) })
                })
        })
    }
}

impl Drop for NonEmptyObservations {
    fn drop(&mut self) {
        let o = self.obs_len;
        self.aggregated_data.drain().for_each(|(_, v)| {
            // SAFETY: The only way to build one of these is through
            // [Self::add], which already checked that the length was correct.
            unsafe { v.consume(o) };
        });
        self.timestamped_data.drain(..).for_each(|(_, _, v)| {
            // SAFETY: The only way to build one of these is through
            // [Self::add], which already checked that the length was correct.
            unsafe { v.consume(o) };
        });
    }
}

#[cfg(test)]
mod test {
    use std::num::NonZeroI64;

    use crate::profile::internal::Id;

    use super::*;

    #[test]
    fn add_and_iter_test() {
        let mut o = Observations::default();
        let s1 = SampleId::from_offset(1);
        let s2 = SampleId::from_offset(2);
        let s3 = SampleId::from_offset(3);
        let t1 = Some(Timestamp::new(1).unwrap());
        let t2 = Some(Timestamp::new(2).unwrap());

        o.add(s1, None, vec![1, 2, 3]);
        o.add(s1, None, vec![4, 5, 6]);
        o.add(s2, None, vec![7, 8, 9]);
        o.iter().for_each(|(k, ts, v)| {
            assert!(ts.is_none());
            if k == s1 {
                assert_eq!(v, vec![5, 7, 9]);
            } else if k == s2 {
                assert_eq!(v, vec![7, 8, 9]);
            } else {
                panic!("Unexpected key");
            }
        });
        // Iter twice to make sure there are no issues doing that
        o.iter().for_each(|(k, ts, v)| {
            assert!(ts.is_none());
            if k == s1 {
                assert_eq!(v, vec![5, 7, 9]);
            } else if k == s2 {
                assert_eq!(v, vec![7, 8, 9]);
            } else {
                panic!("Unexpected key");
            }
        });
        o.add(s3, t1, vec![10, 11, 12]);

        o.iter().for_each(|(k, ts, v)| {
            if k == s1 {
                assert_eq!(v, vec![5, 7, 9]);
                assert!(ts.is_none());
            } else if k == s2 {
                assert_eq!(v, vec![7, 8, 9]);
                assert!(ts.is_none());
            } else if k == s3 {
                assert_eq!(v, vec![10, 11, 12]);
                assert_eq!(ts, t1);
            } else {
                panic!("Unexpected key");
            }
        });

        o.add(s2, t2, vec![13, 14, 15]);
        o.iter().for_each(|(k, ts, v)| {
            if k == s1 {
                assert_eq!(v, vec![5, 7, 9]);
                assert!(ts.is_none());
            } else if k == s2 {
                if ts.is_some() {
                    assert_eq!(v, vec![13, 14, 15]);
                    assert_eq!(ts, t2);
                } else {
                    assert_eq!(v, vec![7, 8, 9]);
                    assert!(ts.is_none());
                }
            } else if k == s3 {
                assert_eq!(v, vec![10, 11, 12]);
                assert_eq!(ts, t1);
            } else {
                panic!("Unexpected key");
            }
        });
    }

    #[test]
    #[should_panic]
    fn different_lengths_panic_different_key_no_ts() {
        let mut o = Observations::default();
        o.add(SampleId::from_offset(1), None, vec![1, 2, 3]);
        // This should panic
        o.add(SampleId::from_offset(2), None, vec![4, 5]);
    }

    #[test]
    #[should_panic]
    fn different_lengths_panic_same_key_no_ts() {
        let mut o = Observations::default();
        o.add(SampleId::from_offset(1), None, vec![1, 2, 3]);
        // This should panic
        o.add(SampleId::from_offset(1), None, vec![4, 5]);
    }

    #[test]
    #[should_panic]
    fn different_lengths_panic_different_key_ts() {
        let mut o = Observations::default();
        let ts = NonZeroI64::new(1).unwrap();
        o.add(SampleId::from_offset(1), Some(ts), vec![1, 2, 3]);
        // This should panic
        o.add(SampleId::from_offset(2), Some(ts), vec![4, 5]);
    }

    #[test]
    #[should_panic]
    fn different_lengths_panic_same_key_ts() {
        let mut o = Observations::default();
        let ts = NonZeroI64::new(1).unwrap();
        o.add(SampleId::from_offset(1), Some(ts), vec![1, 2, 3]);
        // This should panic
        o.add(SampleId::from_offset(1), Some(ts), vec![4, 5]);
    }

    #[test]
    #[should_panic]
    fn different_lengths_panic_different_key_mixed() {
        let mut o = Observations::default();
        let ts = NonZeroI64::new(1).unwrap();
        o.add(SampleId::from_offset(1), None, vec![1, 2, 3]);
        // This should panic
        o.add(SampleId::from_offset(2), Some(ts), vec![4, 5]);
    }

    #[test]
    #[should_panic]
    fn different_lengths_panic_same_key_mixed() {
        let mut o = Observations::default();
        let ts = NonZeroI64::new(1).unwrap();
        o.add(SampleId::from_offset(1), Some(ts), vec![1, 2, 3]);
        // This should panic
        o.add(SampleId::from_offset(1), None, vec![4, 5]);
    }
}