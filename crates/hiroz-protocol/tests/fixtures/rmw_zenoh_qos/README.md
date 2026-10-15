# rmw_zenoh QoS chunk fixtures

Golden output of the real `rmw_zenoh_cpp` C++ code (`liveliness::Entity::make`, `qos_to_keyexpr`, `keyexpr_to_qos`), built from rmw_zenoh commit `dda52d6` (`origin/rolling`) by `oracle/build.sh` and run with `oracle/oracle.cpp`. Tests read these files and never recompute an expected value.

- `qos_grid.tsv`: 576 profiles (`oracle` with no argument, columns `rel dur hist depth deadline lifespan liveliness lease`, then the chunk `qos_to_keyexpr` writes). The depth is 1, 5, 10, 42 or 100 for keep-last and 42 for keep-all.
- `qos_depths.tsv`: 408 rows, depth 0 to 100 for keep-last and keep-all with depth 0, for each reliability and durability. Produced with `oracle emit` on a row per profile; the chunk is the last path component of the token.
- `qos_decode.tsv`: 8 chunks and the fields `keyexpr_to_qos` returns, `NULL` for a chunk it rejects. Produced with `oracle decode`, one chunk per line on stdin.

Values are the rmw integers: reliability 1 reliable and 2 best effort, durability 1 transient local and 2 volatile, history 1 keep-last and 2 keep-all, liveliness 1 automatic and 3 manual by topic.
