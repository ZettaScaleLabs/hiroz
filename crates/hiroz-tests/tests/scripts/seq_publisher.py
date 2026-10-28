"""A released rclpy publisher for the buffer group interop tests. It needs a ROS 2 install with rclpy and rmw_zenoh_cpp."""

import sys
import time

import rclpy

kind, topic, n, size, subs = (
    sys.argv[1],
    sys.argv[2],
    int(sys.argv[3]),
    int(sys.argv[4]),
    int(sys.argv[5]),
)
rclpy.init()
node = rclpy.create_node("seq_pub")
if kind == "uint8":
    from std_msgs.msg import UInt8MultiArray as Msg
else:
    from sensor_msgs.msg import PointCloud2 as Msg
pub = node.create_publisher(Msg, topic, 10)
t0 = time.time()
while pub.get_subscription_count() < subs and time.time() - t0 < 20:
    rclpy.spin_once(node, timeout_sec=0.1)
print("PUB matched=%d" % pub.get_subscription_count(), flush=True)
time.sleep(1.0)
for seq in range(n):
    data = bytearray(size)
    data[0:4] = seq.to_bytes(4, "little")
    m = Msg()
    if kind != "uint8":
        m.header.frame_id = "cloud"
        m.height = 1
        m.width = size
        m.point_step = 1
        m.row_step = size
        m.is_dense = True
    m.data = list(data)
    pub.publish(m)
    rclpy.spin_once(node, timeout_sec=0.2)
print("PUB published=%d" % n, flush=True)
time.sleep(1.0)
node.destroy_node()
rclpy.shutdown()
