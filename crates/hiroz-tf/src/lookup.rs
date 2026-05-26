use hiroz::time::ZTime;
use hiroz_msgs::geometry_msgs::{Transform, TransformStamped};

use crate::LookupError;
use crate::buffer::BufferInner;
use crate::math;

impl BufferInner {
    /// Perform the full transform lookup from `source` to `target` at `time`.
    /// `ZTime::zero()` means "latest available".
    pub(crate) fn lookup(
        &self,
        target: &str,
        source: &str,
        time: ZTime,
    ) -> Result<TransformStamped, LookupError> {
        // Trivial case: same frame
        if target == source {
            return Ok(identity_stamped(target));
        }

        let source_path = self.path_to_root(source);
        let target_path = self.path_to_root(target);

        // Verify both frames exist somewhere in the known transform tree
        if !self.frame_exists_anywhere(source) {
            return Err(LookupError::UnknownFrame(source.to_string()));
        }
        if !self.frame_exists_anywhere(target) {
            return Err(LookupError::UnknownFrame(target.to_string()));
        }

        // Find the lowest common ancestor
        let lca_idx_in_source = source_path
            .iter()
            .position(|f| target_path.contains(f))
            .ok_or_else(|| LookupError::NoCommonAncestor {
                source: source.to_string(),
                target: target.to_string(),
            })?;

        let lca = &source_path[lca_idx_in_source];
        let lca_idx_in_target = target_path.iter().position(|f| f == lca).unwrap();

        // Build T_{LCA←source} by composing edges source→p1→...→LCA
        let source_to_lca = &source_path[..=lca_idx_in_source];
        let mut t_lca_from_source = math::identity_transform();
        for edge in source_to_lca.windows(2) {
            let child = &edge[0];
            let edge_tf = self.interpolate_edge(child, time)?;
            t_lca_from_source = math::compose_transforms(&t_lca_from_source, &edge_tf);
        }

        // Build T_{LCA←target} by composing edges target→q1→...→LCA
        let target_to_lca = &target_path[..=lca_idx_in_target];
        let mut t_lca_from_target = math::identity_transform();
        for edge in target_to_lca.windows(2) {
            let child = &edge[0];
            let edge_tf = self.interpolate_edge(child, time)?;
            t_lca_from_target = math::compose_transforms(&t_lca_from_target, &edge_tf);
        }

        // T_{target←source} = T_{target←LCA} * T_{LCA←source}
        //                    = inv(T_{LCA←target}) ∘ T_{LCA←source}
        let t_target_from_lca = math::invert_transform(&t_lca_from_target);
        let result = math::compose_transforms(&t_lca_from_source, &t_target_from_lca);

        Ok(TransformStamped {
            header: hiroz_msgs::std_msgs::Header {
                frame_id: target.to_string(),
                stamp: ztime_to_stamp(time),
            },
            child_frame_id: source.to_string(),
            transform: result,
        })
    }

    /// Return true iff `frame` appears anywhere in the stored transform tree
    /// (as a child frame OR as a parent/header frame of some stored transform).
    pub(crate) fn frame_exists_anywhere(&self, frame: &str) -> bool {
        if self.dynamic.contains_key(frame) || self.static_.contains_key(frame) {
            return true;
        }
        // Check if it appears as a parent in any stored transform
        self.dynamic.values().any(|entries| {
            entries
                .values()
                .next_back()
                .is_some_and(|tf| tf.header.frame_id == frame)
        }) || self.static_.values().any(|tf| tf.header.frame_id == frame)
    }

    /// Interpolate the stored transform for `child` at `time`.
    ///
    /// Checks static first (always valid, no time interpolation).
    /// Falls back to dynamic with bracketed linear/slerp interpolation.
    pub(crate) fn interpolate_edge(
        &self,
        child: &str,
        time: ZTime,
    ) -> Result<Transform, LookupError> {
        // Static transforms are always valid
        if let Some(tf) = self.static_.get(child) {
            return Ok(tf.transform.clone());
        }

        let entries = self
            .dynamic
            .get(child)
            .ok_or_else(|| LookupError::UnknownFrame(child.to_string()))?;

        if entries.is_empty() {
            return Err(LookupError::UnknownFrame(child.to_string()));
        }

        // Latest-available sentinel
        if time == ZTime::zero() {
            let (_, tf) = entries.iter().next_back().unwrap();
            return Ok(tf.transform.clone());
        }

        let oldest = *entries.keys().next().unwrap();
        let newest = *entries.keys().next_back().unwrap();

        if time < oldest || time > newest {
            return Err(LookupError::ExtrapolationError {
                frame: child.to_string(),
                requested: time,
                oldest,
                newest,
            });
        }

        // Exact match
        if let Some(tf) = entries.get(&time) {
            return Ok(tf.transform.clone());
        }

        // Interpolate between surrounding entries
        let before = entries
            .range(..time)
            .next_back()
            .map(|(t, tf)| (*t, tf.transform.clone()));
        let after = entries
            .range(time..)
            .next()
            .map(|(t, tf)| (*t, tf.transform.clone()));

        match (before, after) {
            (Some((t0, tf0)), Some((t1, tf1))) => {
                let t0_ns = t0.as_unix_nanos();
                let t1_ns = t1.as_unix_nanos();
                let req_ns = time.as_unix_nanos();
                let alpha = (req_ns - t0_ns) as f64 / (t1_ns - t0_ns) as f64;
                Ok(math::interpolate_transforms(&tf0, &tf1, alpha))
            }
            _ => Err(LookupError::ExtrapolationError {
                frame: child.to_string(),
                requested: time,
                oldest,
                newest,
            }),
        }
    }
}

fn identity_stamped(frame: &str) -> TransformStamped {
    TransformStamped {
        header: hiroz_msgs::std_msgs::Header {
            frame_id: frame.to_string(),
            stamp: hiroz_msgs::builtin_interfaces::Time { sec: 0, nanosec: 0 },
        },
        child_frame_id: frame.to_string(),
        transform: math::identity_transform(),
    }
}

fn ztime_to_stamp(t: ZTime) -> hiroz_msgs::builtin_interfaces::Time {
    let nanos = t.as_unix_nanos().max(0) as u64;
    hiroz_msgs::builtin_interfaces::Time {
        sec: (nanos / 1_000_000_000) as i32,
        nanosec: (nanos % 1_000_000_000) as u32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::BufferInner;
    use hiroz_msgs::builtin_interfaces::Time;
    use hiroz_msgs::geometry_msgs::{Quaternion, Transform, Vector3};
    use hiroz_msgs::std_msgs::Header;
    use hiroz_msgs::tf2_msgs::TFMessage;

    fn make_tf_at(parent: &str, child: &str, sec: i32, x: f64) -> TransformStamped {
        TransformStamped {
            header: Header {
                frame_id: parent.to_string(),
                stamp: Time { sec, nanosec: 0 },
            },
            child_frame_id: child.to_string(),
            transform: Transform {
                translation: Vector3 { x, y: 0.0, z: 0.0 },
                rotation: Quaternion {
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                    w: 1.0,
                },
            },
        }
    }

    fn t(sec: i32) -> ZTime {
        ZTime::from_unix_nanos(sec as i64 * 1_000_000_000)
    }

    #[test]
    fn lookup_unknown_frame_errors() {
        let buf = BufferInner::default();
        assert!(matches!(
            buf.lookup("map", "base_link", ZTime::zero()),
            Err(LookupError::UnknownFrame(_))
        ));
    }

    #[test]
    fn lookup_same_frame_is_identity() {
        let buf = BufferInner::default();
        let tf = buf.lookup("map", "map", ZTime::zero()).unwrap();
        assert!((tf.transform.translation.x).abs() < 1e-10);
        assert!((tf.transform.rotation.w - 1.0).abs() < 1e-10);
    }

    #[test]
    fn lookup_direct_edge_returns_latest() {
        let mut buf = BufferInner::default();
        buf.add_message(
            TFMessage {
                transforms: vec![make_tf_at("map", "odom", 10, 5.0)],
            },
            false,
        );
        let tf = buf.lookup("map", "odom", ZTime::zero()).unwrap();
        assert!((tf.transform.translation.x - 5.0).abs() < 1e-10);
    }

    #[test]
    fn lookup_interpolates_at_midpoint() {
        let mut buf = BufferInner::default();
        buf.add_message(
            TFMessage {
                transforms: vec![make_tf_at("map", "odom", 10, 0.0)],
            },
            false,
        );
        buf.add_message(
            TFMessage {
                transforms: vec![make_tf_at("map", "odom", 20, 10.0)],
            },
            false,
        );
        // At t=15 (midpoint), x should be ~5.0
        let tf = buf.lookup("map", "odom", t(15)).unwrap();
        assert!((tf.transform.translation.x - 5.0).abs() < 1e-6);
    }

    #[test]
    fn lookup_extrapolation_error_outside_window() {
        let mut buf = BufferInner::default();
        buf.add_message(
            TFMessage {
                transforms: vec![make_tf_at("map", "odom", 10, 0.0)],
            },
            false,
        );
        assert!(matches!(
            buf.lookup("map", "odom", t(5)),
            Err(LookupError::ExtrapolationError { .. })
        ));
    }

    #[test]
    fn lookup_two_hop_chain() {
        let mut buf = BufferInner::default();
        // map→odom: translate x=1
        buf.add_message(
            TFMessage {
                transforms: vec![make_tf_at("map", "odom", 10, 1.0)],
            },
            false,
        );
        // odom→base_link: translate x=2
        buf.add_message(
            TFMessage {
                transforms: vec![make_tf_at("odom", "base_link", 10, 2.0)],
            },
            false,
        );
        // Expected: base_link in map = x=3
        let tf = buf.lookup("map", "base_link", ZTime::zero()).unwrap();
        assert!((tf.transform.translation.x - 3.0).abs() < 1e-6);
    }

    #[test]
    fn lookup_static_transform_at_any_time() {
        let mut buf = BufferInner::default();
        buf.add_message(
            TFMessage {
                transforms: vec![make_tf_at("map", "sensor", 0, 0.5)],
            },
            true,
        );
        // Static transforms should be returned regardless of requested time
        let tf = buf.lookup("map", "sensor", t(9999)).unwrap();
        assert!((tf.transform.translation.x - 0.5).abs() < 1e-10);
    }

    #[test]
    fn lookup_no_common_ancestor_errors() {
        let mut buf = BufferInner::default();
        // Two disconnected trees
        buf.add_message(
            TFMessage {
                transforms: vec![make_tf_at("world_a", "odom_a", 10, 1.0)],
            },
            false,
        );
        buf.add_message(
            TFMessage {
                transforms: vec![make_tf_at("world_b", "odom_b", 10, 1.0)],
            },
            false,
        );
        assert!(matches!(
            buf.lookup("odom_a", "odom_b", ZTime::zero()),
            Err(LookupError::NoCommonAncestor { .. })
        ));
    }

    #[test]
    fn lookup_inverse_direction() {
        let mut buf = BufferInner::default();
        // map→odom: translate x=5
        buf.add_message(
            TFMessage {
                transforms: vec![make_tf_at("map", "odom", 10, 5.0)],
            },
            false,
        );
        // Lookup odom←map should be inverse: x=-5
        let tf = buf.lookup("odom", "map", ZTime::zero()).unwrap();
        assert!((tf.transform.translation.x - (-5.0)).abs() < 1e-6);
    }

    #[test]
    fn lookup_full_via_fixed_frame() {
        // Set up: map→odom (x=1) and map→camera (x=3).
        // lookup_full("camera", "odom", t, "map", t) should give x=2 (camera relative to odom).
        let mut buf = BufferInner::default();
        buf.add_message(
            TFMessage {
                transforms: vec![make_tf_at("map", "odom", 10, 1.0)],
            },
            false,
        );
        buf.add_message(
            TFMessage {
                transforms: vec![make_tf_at("map", "camera", 10, 3.0)],
            },
            false,
        );

        let t = ZTime::zero();
        // T(map ← odom) at t, then T(camera ← map) at t
        let t1 = buf.lookup("map", "odom", t).unwrap();
        let t2 = buf.lookup("camera", "map", t).unwrap();
        let result = crate::math::compose_stamped(t2, t1, "camera", "odom");
        // camera is at x=3, odom is at x=1 in map frame.
        // odom expressed in camera frame = 1 - 3 = -2 (odom is behind camera).
        assert!(
            (result.transform.translation.x - (-2.0)).abs() < 1e-5,
            "expected x=-2.0, got {}",
            result.transform.translation.x
        );
        assert_eq!(result.header.frame_id, "camera");
        assert_eq!(result.child_frame_id, "odom");
    }
}
