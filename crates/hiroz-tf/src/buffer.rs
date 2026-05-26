use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use hiroz::time::ZTime;
use hiroz_msgs::geometry_msgs::TransformStamped;
use hiroz_msgs::tf2_msgs::TFMessage;
use tokio::sync::Notify;

/// Default maximum age of dynamic transforms to retain (10 seconds, matching tf2).
pub(crate) const DEFAULT_MAX_HISTORY: Duration = Duration::from_secs(10);

/// Maximum depth when walking up the frame tree, to guard against cycles.
const MAX_TREE_DEPTH: usize = 100;

pub(crate) struct BufferInner {
    /// Dynamic transforms keyed by child_frame_id → time-sorted entries.
    pub(crate) dynamic: HashMap<String, BTreeMap<ZTime, TransformStamped>>,
    /// Static transforms keyed by child_frame_id → latest entry (no time history needed).
    pub(crate) static_: HashMap<String, TransformStamped>,
    pub(crate) max_history: Duration,
    /// Notified on every `add_message` call so `wait_for_transform` can wake up.
    pub(crate) notify: Arc<Notify>,
}

impl Default for BufferInner {
    fn default() -> Self {
        Self {
            dynamic: HashMap::new(),
            static_: HashMap::new(),
            max_history: DEFAULT_MAX_HISTORY,
            notify: Arc::new(Notify::new()),
        }
    }
}

impl BufferInner {
    pub(crate) fn add_message(&mut self, msg: TFMessage, is_static: bool) {
        for tf in msg.transforms {
            self.add_transform(tf, is_static);
        }
        self.notify.notify_waiters();
    }

    fn add_transform(&mut self, tf: TransformStamped, is_static: bool) {
        if is_static {
            self.static_.insert(tf.child_frame_id.clone(), tf);
        } else {
            let stamp = stamp_to_ztime(&tf.header.stamp);
            let entries = self.dynamic.entry(tf.child_frame_id.clone()).or_default();
            entries.insert(stamp, tf);
            self.prune_old_entries(stamp);
        }
    }

    fn prune_old_entries(&mut self, now: ZTime) {
        let cutoff_nanos = now
            .as_unix_nanos()
            .saturating_sub(self.max_history.as_nanos() as i64);
        let cutoff = ZTime::from_unix_nanos(cutoff_nanos);

        for entries in self.dynamic.values_mut() {
            let old_keys: Vec<ZTime> = entries.range(..cutoff).map(|(k, _)| *k).collect();
            for k in old_keys {
                entries.remove(&k);
            }
        }
    }

    /// Return all known frame IDs (both dynamic and static children).
    pub(crate) fn all_frames(&self) -> Vec<String> {
        let mut frames: std::collections::HashSet<String> = std::collections::HashSet::new();
        for key in self.dynamic.keys() {
            frames.insert(key.clone());
        }
        for key in self.static_.keys() {
            frames.insert(key.clone());
        }
        frames.into_iter().collect()
    }

    /// Walk from `frame` toward the tree root, returning the path
    /// `[frame, parent(frame), parent(parent(frame)), ..., root]`.
    pub(crate) fn path_to_root(&self, frame: &str) -> Vec<String> {
        let mut path = vec![frame.to_string()];
        let mut current = frame.to_string();

        while path.len() < MAX_TREE_DEPTH {
            let parent = self
                .static_
                .get(&current)
                .map(|tf| tf.header.frame_id.clone())
                .or_else(|| {
                    self.dynamic
                        .get(&current)
                        .and_then(|entries| entries.values().next_back())
                        .map(|tf| tf.header.frame_id.clone())
                });

            match parent {
                None => break,
                Some(p) if path.contains(&p) => break, // cycle guard
                Some(p) => {
                    path.push(p.clone());
                    current = p;
                }
            }
        }

        path
    }
}

/// Convert a `builtin_interfaces::Time` stamp to `ZTime`.
pub(crate) fn stamp_to_ztime(stamp: &hiroz_msgs::builtin_interfaces::Time) -> ZTime {
    let total_nanos = (stamp.sec as i64)
        .saturating_mul(1_000_000_000)
        .saturating_add(stamp.nanosec as i64);
    ZTime::from_unix_nanos(total_nanos)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hiroz_msgs::builtin_interfaces::Time;
    use hiroz_msgs::geometry_msgs::{Quaternion, Transform, Vector3};
    use hiroz_msgs::std_msgs::Header;

    fn make_tf(parent: &str, child: &str, sec: i32) -> TransformStamped {
        TransformStamped {
            header: Header {
                frame_id: parent.to_string(),
                stamp: Time { sec, nanosec: 0 },
            },
            child_frame_id: child.to_string(),
            transform: Transform {
                translation: Vector3 {
                    x: 1.0,
                    y: 0.0,
                    z: 0.0,
                },
                rotation: Quaternion {
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                    w: 1.0,
                },
            },
        }
    }

    #[test]
    fn add_dynamic_transform_inserts_entry() {
        let mut buf = BufferInner::default();
        let tf = make_tf("map", "odom", 100);
        buf.add_message(
            TFMessage {
                transforms: vec![tf],
            },
            false,
        );
        assert!(buf.dynamic.contains_key("odom"));
    }

    #[test]
    fn add_static_transform_overwrites_previous() {
        let mut buf = BufferInner::default();
        let tf1 = make_tf("map", "sensor", 1);
        let tf2 = make_tf("world", "sensor", 2);
        buf.add_message(
            TFMessage {
                transforms: vec![tf1],
            },
            true,
        );
        buf.add_message(
            TFMessage {
                transforms: vec![tf2],
            },
            true,
        );
        assert_eq!(buf.static_["sensor"].header.frame_id, "world");
    }

    #[test]
    fn prune_removes_old_entries() {
        let mut buf = BufferInner {
            max_history: Duration::from_secs(5),
            ..Default::default()
        };
        // Insert old entry at t=0
        buf.add_message(
            TFMessage {
                transforms: vec![make_tf("map", "odom", 0)],
            },
            false,
        );
        // Insert recent entry at t=100 — triggers pruning of t=0 (100-0 > 5s)
        buf.add_message(
            TFMessage {
                transforms: vec![make_tf("map", "odom", 100)],
            },
            false,
        );
        let entries = &buf.dynamic["odom"];
        let oldest_sec = entries
            .values()
            .next()
            .map(|tf| tf.header.stamp.sec)
            .unwrap();
        assert!(oldest_sec > 0, "old entry at t=0 should have been pruned");
    }

    #[test]
    fn path_to_root_single_hop() {
        let mut buf = BufferInner::default();
        buf.add_message(
            TFMessage {
                transforms: vec![make_tf("map", "odom", 1)],
            },
            false,
        );
        let path = buf.path_to_root("odom");
        assert_eq!(path, vec!["odom", "map"]);
    }

    #[test]
    fn path_to_root_two_hops() {
        let mut buf = BufferInner::default();
        buf.add_message(
            TFMessage {
                transforms: vec![make_tf("map", "odom", 1)],
            },
            false,
        );
        buf.add_message(
            TFMessage {
                transforms: vec![make_tf("odom", "base_link", 1)],
            },
            false,
        );
        let path = buf.path_to_root("base_link");
        assert_eq!(path, vec!["base_link", "odom", "map"]);
    }

    #[test]
    fn path_to_root_unknown_frame_is_just_itself() {
        let buf = BufferInner::default();
        let path = buf.path_to_root("unknown");
        assert_eq!(path, vec!["unknown"]);
    }

    #[test]
    fn all_frames_includes_both_static_and_dynamic() {
        let mut buf = BufferInner::default();
        buf.add_message(
            TFMessage {
                transforms: vec![make_tf("map", "odom", 1)],
            },
            false,
        );
        buf.add_message(
            TFMessage {
                transforms: vec![make_tf("map", "sensor", 1)],
            },
            true,
        );
        let frames = buf.all_frames();
        assert!(frames.contains(&"odom".to_string()));
        assert!(frames.contains(&"sensor".to_string()));
    }
}
