set -e
S=/src/rmw_zenoh_cpp/src/detail
V=/opt/ros/lyrical
Z=$V/opt/zenoh_cpp_vendor
INC="-I$S -I$Z/include $(for d in rmw rcutils rcpputils rosidl_runtime_c rcl; do echo -n "-I$V/include/$d "; done) -I$V/include"
g++ -std=c++17 -O1 -DZENOHCXX_ZENOHC $INC /w/oracle.cpp $S/liveliness_utils.cpp $S/qos.cpp $S/logging.cpp $S/simplified_xxhash3.cpp \
  -L$Z/lib -L$V/lib -Wl,-rpath,$Z/lib:$V/lib -lzenohc -lrmw -lrcutils -lrcl_logging_interface -lrcpputils -o /w/oracle 2>&1 | tail -40
ls -la /w/oracle
