# Input rows for `oracle emit`: bash emit_rows.sh depths|durations
# One row per profile, 22 tab-separated columns: the fixed entity head (10 columns),
# the 11 rmw integers `rel dur hist depth dl_s dl_ns ls_s ls_ns lv lease_s lease_ns`,
# and `-` for no backends. The fixed head makes the chunk the only thing that varies.
Z=c9fa0e9e9d8c5744cef446c8628762dc
H=RIHS01_5687e861b8d307a5e48b7515467ae7a5fc2daf805bd0ce6d8e9e604bade9f385
S=9223372036 N=854775807
row() { printf '0\t%s\t9\t100\t/\toracle\t/\t/u8\tstd_msgs::msg::dds_::UInt8MultiArray_\t%s\t' "$Z" "$H"; echo "$* " | sed 's/ $//; s/ /\t/g; s/$/\t-/'; }
case "${1:?depths|durations}" in
  depths)
    # depth 0 to 100 for keep-last, then keep-all with depth 0, for each reliability and durability
    for rel in 1 2; do for dur in 1 2; do
      for d in $(seq 0 100); do row $rel $dur 1 $d $S $N $S $N 1 $S $N; done
      row $rel $dur 2 0 $S $N $S $N 1 $S $N
    done; done ;;
  durations)
    # one component of a duration at the infinite default, the other not: deadline, lifespan, lease
    for c in "$S 0" "$S 5" "0 $N" "1 $N"; do
      row 1 2 1 10 $c $S $N 1 $S $N
      row 1 2 1 10 $S $N $c 1 $S $N
      row 1 2 1 10 $S $N $S $N 1 $c
    done ;;
  *) echo "usage: emit_rows.sh depths|durations" >&2; exit 2 ;;
esac
