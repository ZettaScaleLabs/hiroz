<!--
DO NOT EXPAND THIS README
Documentation source: book/ - Published: https://zettascalelabs.github.io/hiroz/
Keep this file minimal. Point readers to the book.
-->

# rmw_zenoh QoS chunk fixtures

Golden output of the real `rmw_zenoh_cpp` C++ code (`liveliness::Entity::make`, `qos_to_keyexpr`, `keyexpr_to_qos`), built from rmw_zenoh commit `dda52d6`. The tests read these files and never recompute an expected value. The regenerate recipe is the comment header of `oracle/build.sh`.

The grid file `qos_grid.tsv` keeps 2 of the 4 columns that `oracle qos` prints (the profile fields and the chunk). It drops the canonical token and the gid.

**📚 [Full Documentation](https://zettascalelabs.github.io/hiroz/)**
