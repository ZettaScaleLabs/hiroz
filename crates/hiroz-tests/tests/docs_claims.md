Claims table for the buffer section of `docs/user-guide/interop.md` (criterion E7). The test `docs_claims_table_has_no_untested_sentence` reads this table. A sentence marked `not tested` rests on code reading or on the upstream project, and the note says which.

| sentence | tests | note |
|---|---|---|
| A ROS 2 Lyrical `rmw_zenoh` publisher treats an unbounded `uint8[]` field as a buffer, also inside a nested message | not tested | Behavior of the upstream publisher. The interop cell sends `UInt8MultiArray`, so the nested case is not exercised |
| Examples are `sensor_msgs/Image` and `sensor_msgs/PointCloud2` | not tested | Names of upstream messages. No cell sends them |
| If every matched subscriber announces buffer support, the publisher writes the message only on `<topic key>/_buf_cpu`, and a subscriber that reads only `<topic key>` receives nothing | joined_subscribers_receive_from_a_released_publisher | The cell reads the group key as N and the base key as 0 from a Lyrical publisher. It does not run a subscriber that reads only the base key |
| A hiroz subscriber that joins the group announces buffer support, with `backends:cpu:` at the end of its liveliness token, and also reads `<topic key>/_buf_cpu` | typed_subscriber_with_buffer_fields_joins, joined_subscriber_reads_the_group_key_with_its_locality | |
| The payload is plain CDR | joined_subscriber_receives_on_the_key_a_real_publisher_writes | |
| A typed subscriber, a dynamic subscriber and a cache join this way | typed_subscriber_with_buffer_fields_joins, dynamic_subscriber_joins_for_nested_and_unbounded_byte_fields, cache_joins_the_group | |
| By default (`BufferGroup::Auto`) a subscriber joins when its message has an unbounded `uint8[]`, directly or in a nested message, or when hiroz has no message definition to ask | typed_subscriber_with_buffer_fields_joins, dynamic_subscriber_joins_for_nested_and_unbounded_byte_fields, typed_subscriber_without_schema_joins | |
| A message with only a bounded sequence or a fixed array of `uint8`, or with no `uint8[]`, does not join | dynamic_subscriber_with_bounded_or_fixed_bytes_does_not_join, typed_subscriber_without_byte_fields_does_not_join | |
| A schema cannot tell `byte[]` from `uint8[]`, so a message with an unbounded `byte[]` also joins | not tested | Code reading of the schema type. No test builds a schema with a `byte` field |
| A joined subscriber that is not needed costs one idle subscription | not tested | Code reading. No test counts the subscriptions |
| Call `with_buffer_group` on the subscriber builder to decide for yourself: `BufferGroup::Cpu` joins whatever the message is, and `BufferGroup::Off` does not join | cpu_joins_a_type_without_byte_fields, opt_out_announces_no_backends, opt_out_stays_out_without_a_schema | |
| These subscribers never join, whatever you ask: | transient_local_stays_out_even_when_forced, custom_key_format_stays_out_even_when_forced | Lead-in of the list below it |
| Transient-local subscribers | transient_local_stays_out_even_when_forced | |
| `rmw_zenoh` serves them on the base key only | not tested | Behavior of the upstream publisher |
| Subscribers on a custom key expression format | custom_key_format_stays_out_even_when_forced | |
| These subscribers do not join: | not tested | Lead-in of the list below it |
| Raw subscribers | not tested | No test at this commit asserts that a default raw subscriber stays out. The raw tests belong to the PR that adds the opt-in |
| There is no way to opt them in | not tested | Code reading. This sentence changes with the raw opt-in PR |
| `rmw-zenoh-rs` subscriptions | not tested | Code reading of the one call site in `rmw-zenoh-rs`. No test runs it |
| They pass `BufferGroup::Off` | not tested | Code reading of the same call site |
| The Python binding and the feedback and status subscribers of an action client have no message definition, so they join under `Auto` | not tested | The general rule is tested by typed_subscriber_without_schema_joins. No test runs the Python binding or an action client at the token level |
| The Python binding has no way to opt out | not tested | Code reading. `hiroz-py` has no call to `with_buffer_group` |
| hiroz writes one `info` log line for each group key in a process, after it subscribes to the key | joining_logs_the_group_key_once_per_key | |
| If you use access control, allow `<topic key>/_buf_cpu` wherever you allow `<topic key>`, and list every network interface in the subject of the rule | not tested | Advice. The router test denies the group key but does not vary the interface list of a rule subject |
| A router rule that denies the group key makes a joined subscriber on a buffer topic receive nothing | denied_group_key_starves_silently_while_the_base_key_flows | |
| No error is raised, and the base key still delivers | denied_group_key_starves_silently_while_the_base_key_flows | |
| A topic that is not a buffer topic is not affected | non_buffer_topic_still_delivers_under_a_group_key_deny_rule | |
