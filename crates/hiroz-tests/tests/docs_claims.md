Claims table for the buffer section of `docs/user-guide/interop.md` (criterion E7). The test `docs_claims_table_has_no_untested_sentence` reads this table. A sentence marked `not tested` rests on code reading or on the upstream project, and the note says which.

| sentence | tests | note |
|---|---|---|
| A ROS 2 Lyrical `rmw_zenoh` publisher treats an unbounded `uint8[]` field as a buffer, also inside a nested message | not tested | Behavior of the upstream publisher. The interop cell sends `UInt8MultiArray`, so the nested case is not exercised |
| Examples are `sensor_msgs/Image` and `sensor_msgs/PointCloud2` | not tested | Names of upstream messages. No cell sends them |
| If a matched subscriber announces buffer support, the publisher writes the message only on `<topic key>/_buf_cpu` | joined_subscribers_receive_from_a_released_publisher, mixed_matched_set_delivers_each_sample_once | Both cells read the group key as N and the base key as 0 from a Lyrical publisher |
| A subscriber that reads only `<topic key>` receives nothing, also when it is the one matched subscriber that does not announce buffer support (measured with `rmw_zenoh_cpp` 0.10.6) | mixed_matched_set_delivers_each_sample_once | The legacy subscriber is a hiroz subscriber with `BufferGroup::Off`. It reads the base key only and received 0 of 30 in two matched sets |
| Humble, Jazzy and Kilted have no buffer-aware `rmw_zenoh_cpp`, so the cells that need one skip there | not tested | Upstream releases. The nix cells printed the skip line on every distro, with `rmw_zenoh_cpp` 0.10.4 |
| A hiroz subscriber that joins the group announces buffer support, with `backends:cpu:` at the end of its liveliness token, and also reads `<topic key>/_buf_cpu` | typed_subscriber_with_buffer_fields_joins, joined_subscriber_reads_the_group_key_with_its_locality | |
| The payload is plain CDR | joined_subscriber_receives_on_the_key_a_real_publisher_writes | |
| A typed subscriber, a dynamic subscriber and a cache join this way | typed_subscriber_with_buffer_fields_joins, dynamic_subscriber_joins_for_nested_and_unbounded_byte_fields, cache_joins_the_group | |
| A joined subscriber has one subscription, on `<topic key>/**`, and drops every other key under the topic key in process | other_groups_of_the_topic_never_reach_a_joined_subscriber | |
| By default (`BufferGroup::Auto`) a subscriber joins when its message has an unbounded `uint8[]`, directly or in a nested message, or when hiroz has no message definition to ask | typed_subscriber_with_buffer_fields_joins, dynamic_subscriber_joins_for_nested_and_unbounded_byte_fields, typed_subscriber_without_schema_joins | |
| A message with only a bounded sequence or a fixed array of `uint8`, or with no `uint8[]`, does not join | dynamic_subscriber_with_bounded_or_fixed_bytes_does_not_join, typed_subscriber_without_byte_fields_does_not_join | |
| A schema cannot tell `byte[]` from `uint8[]`, so a message with an unbounded `byte[]` also joins | not tested | Code reading of the schema type. No test builds a schema with a `byte` field |
| A joined subscriber that is not needed adds no second subscription to the session | not tested | Code reading. A probe measured the declaration time, and no test counts the subscriptions |
| Call `with_buffer_group` on the subscriber builder to decide for yourself: `BufferGroup::Cpu` joins whatever the message is, and `BufferGroup::Off` does not join | cpu_joins_a_type_without_byte_fields, opt_out_announces_no_backends, opt_out_stays_out_without_a_schema | |
| The cache builder and the action client have no `with_buffer_group`, so they always follow `Auto` | not tested | Code reading. `ZCacheBuilder` has no such method, and the action client builds its subscribers without one |
| These subscribers never join, whatever you ask: | transient_local_stays_out_even_when_forced, custom_key_format_stays_out_even_when_forced | Lead-in of the list below it |
| Transient-local subscribers | transient_local_stays_out_even_when_forced | |
| `rmw_zenoh` serves them on the base key only | not tested | Behavior of the upstream publisher |
| Subscribers on a custom key expression format | custom_key_format_stays_out_even_when_forced | |
| These subscribers do not join: | not tested | Lead-in of the list below it |
| Raw subscribers | not tested | No test at this commit asserts that a default raw subscriber stays out. The raw tests belong to the PR that adds the opt-in |
| There is no way to opt them in | not tested | Code reading. This sentence changes with the raw opt-in PR |
| `rmw-zenoh-rs` subscriptions | not tested | Code reading of the one call site in `rmw-zenoh-rs`. No test runs it |
| They pass `BufferGroup::Off` | not tested | Code reading of the same call site |
| The feedback and status subscribers of an action client have no message definition, so they join under `Auto` | no_schema_callers_follow_the_rule | Tested at the token level for an action type with no schema. The claim that a generated action has no message definition is code reading. action_feedback_arrives_once_each_with_the_group_joined runs a real action and reads delivery, not the token |
| The Python binding has no message definition either, so it joins under `Auto` | not tested | Code reading of the Python node. The general rule is tested by typed_subscriber_without_schema_joins. No test runs the Python binding |
| The Python binding has no way to opt out | not tested | Code reading. `hiroz-py` has no call to `with_buffer_group` |
| hiroz writes one `info` log line for each group key in a process, after it subscribes to the key | joining_logs_the_group_key_once_per_key | |
| hiroz keeps one string for each group key it logged, for the life of the process | not tested | Code reading of the set in `log_once`. No test counts the set |
| If you use access control, allow `<topic key>/**`, which includes `<topic key>/_buf_cpu`, wherever you allow `<topic key>` | not tested | Advice. No test runs an allow rule. denied_wide_key_starves_a_joined_subscriber shows the effect of the opposite rule |
| A router rule that denies the group key makes a joined subscriber on a buffer topic receive nothing | denied_group_key_starves_silently_while_the_base_key_flows | |
| No error is raised, and the base key still delivers | denied_group_key_starves_silently_while_the_base_key_flows | |
| A topic that is not a buffer topic is not affected | non_buffer_topic_still_delivers_under_a_group_key_deny_rule | |
| A rule that denies `<topic key>/**` makes a joined subscriber receive nothing on both keys | denied_wide_key_starves_a_joined_subscriber | Measured on one in-process router, n=1 run: 0 received on the group key and on the base key, against some on an open router |
| `<topic key>/**` includes `<topic key>`, so an `Off` subscriber of the topic receives nothing either | denied_wide_key_starves_a_joined_subscriber | The `Off` subscriber received 0 on the base key under the rule and some on an open router. A joined subscriber on a second topic still received under the same rule |
| `BufferGroup::Auto` is the default, and it changes the liveliness token and the routing of existing subscribers | typed_subscriber_with_buffer_fields_joins, opt_out_announces_no_backends |  |
| A subscriber set to `Off` has a token with no `backends:` field and reads only `<topic key>` | opt_out_announces_no_backends, mixed_matched_set_delivers_each_sample_once | The token is read in the first test. The second shows an `Off` subscriber receiving 0 of 30 from a publisher that writes the group key |
| This is what every subscriber did before `Auto` existed | not tested | History of the code, not a property of the current tree |
| With `Auto`, a subscriber of a message with an unbounded `uint8[]` field, or of a message with no definition, has a token that ends with `backends:cpu:`, and it also reads `<topic key>/_buf_cpu` | typed_subscriber_with_buffer_fields_joins, typed_subscriber_without_schema_joins, joined_subscriber_reads_the_group_key_with_its_locality |  |
| This affects typed subscribers, dynamic subscribers and caches of such messages, the feedback and status subscribers of an action client, and the Python binding | typed_subscriber_with_buffer_fields_joins, dynamic_subscriber_joins_for_nested_and_unbounded_byte_fields, cache_joins_the_group, no_schema_callers_follow_the_rule | The Python binding is code reading. No test runs it |
| To keep the old token and routing on a subscriber, call `with_buffer_group(BufferGroup::Off)` on its builder | opt_out_announces_no_backends, opt_out_stays_out_without_a_schema |  |
| The cache builder, the action client and the Python binding have no way to keep them | not tested | Code reading. `ZCacheBuilder` has no `with_buffer_group`, and `hiroz-py` has no call to it |
| A subscriber whose liveliness token has no `backends:` field receives nothing from a buffer-aware publisher, until <https://github.com/ros2/rmw_zenoh/pull/1078> lands | not tested | Measured on 0.10.6 only, by hand outside the test suite. mixed_matched_set_delivers_each_sample_once covers a hiroz `Off` subscriber against 0.10.6 |
| Measured on `rmw_zenoh_cpp` 0.10.6 (n=5) | not tested | Same manual measurement, 5 runs |
| The routing code is the same in 0.10.5 and 0.10.7 by source reading, not measured | not tested | Code reading. 0.10.5 and 0.10.7 were not run |
| 0.10.4 and 0.11.0 have no buffer support | not tested | 0.10.4 was measured as a publisher in the same manual measurement. 0.11.0 was not measured here |
| This holds for a buffer topic | not tested | The measurement used `UInt8MultiArray`, which has an unbounded `uint8[]` |
| A hiroz subscriber set to `Off` and every `rmw-zenoh-rs` subscription have no `backends:` field, so both receive nothing in this case | not tested | The `Off` token is tested by opt_out_announces_no_backends and the `Off` delivery by mixed_matched_set_delivers_each_sample_once. The `rmw-zenoh-rs` call site passes `Off` by code reading. No test runs `rmw-zenoh-rs` against a buffer-aware publisher |
| Measured with a publisher on 0.10.6, a buffer-aware subscriber on 0.10.6 and a legacy subscriber on 0.10.4 shared one topic | not tested | Manual measurement outside the test suite. rclpy publisher and subscribers, a router on one host |
| The buffer-aware subscriber received 30 of 30 messages and the legacy subscriber received 0 of 30 | not tested | Same manual measurement, 5 runs, 30 of 30 and 0 of 30 in every run |
| A legacy subscriber alone received 0 of 30 | not tested | Same manual measurement, 5 runs |
| Each figure is the same in 5 runs, on one host, with 30 `UInt8MultiArray` messages of 64 bytes at 5 Hz | not tested | Conditions of the manual measurement. The 5 Hz rate is the publisher's spin timeout, not a timer |
| A publisher on 0.10.4 delivered 30 of 30 messages to a legacy subscriber, and to a buffer-aware subscriber, in the same 5 runs | not tested | Same manual measurement |
| Versions other than 0.10.4 and 0.10.6 are not measured | not tested | Scope statement |
