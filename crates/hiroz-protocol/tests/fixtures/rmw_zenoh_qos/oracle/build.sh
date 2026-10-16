# Builds the oracle and regenerates the fixtures in this directory's parent.
# Run in a container of docker.io/library/ros:lyrical-ros-base:
#   mounts   /src  a checkout of rmw_zenoh at commit dda52d6 (it holds rmw_zenoh_cpp/), read only
#            /w    this directory (oracle.cpp, emit_rows.sh), writable
#   first    apt-get update && apt-get install -y ros-lyrical-zenoh-cpp-vendor
#            (it provides /opt/ros/lyrical/opt/zenoh_cpp_vendor, which the INC and -L paths below use)
# Then, inside the container, in /w:
#   bash build.sh                                                     # writes ./oracle
#   ./oracle qos | cut -f1,2 > qos_grid.tsv                           # the oracle prints 4 columns; the fixture keeps the first 2
#   bash emit_rows.sh depths > depths.in                              # one row per profile for `oracle emit`
#   ./oracle emit < depths.in > depths.emit                           # the token rmw_zenoh writes; the chunk is after the last '/'
#   { echo '# rel dur hist depth dl_s dl_ns ls_s ls_ns lv lease_s lease_ns<TAB>qos'
#     paste -d'\t' <(cut -f11-21 depths.in | tr '\t' ' ') <(cut -f1 depths.emit | sed 's#.*/##'); } > qos_depths.tsv
#   same with `emit_rows.sh durations` for qos_durations.tsv
#   ./oracle decode < chunks.txt > qos_decode.tsv                     # chunks.txt: one QoS chunk per line
# qos_grid.tsv and qos_depths.tsv regenerate byte for byte from these commands.
set -e
S=/src/rmw_zenoh_cpp/src/detail
V=/opt/ros/lyrical
Z=$V/opt/zenoh_cpp_vendor
INC="-I$S -I$Z/include $(for d in rmw rcutils rcpputils rosidl_runtime_c rcl; do echo -n "-I$V/include/$d "; done) -I$V/include"
g++ -std=c++17 -O1 -DZENOHCXX_ZENOHC $INC /w/oracle.cpp $S/liveliness_utils.cpp $S/qos.cpp $S/logging.cpp $S/simplified_xxhash3.cpp \
  -L$Z/lib -L$V/lib -Wl,-rpath,$Z/lib:$V/lib -lzenohc -lrmw -lrcutils -lrcl_logging_interface -lrcpputils -o /w/oracle 2>&1 | tail -40
ls -la /w/oracle
