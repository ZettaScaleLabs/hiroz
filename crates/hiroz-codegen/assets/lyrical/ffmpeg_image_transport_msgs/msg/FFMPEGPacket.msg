std_msgs/Header header
int32 width       # original image width
int32 height      # original image height

# The "encoding" message field is a string with semicolon-separated tokens that
# indicate the codec (first token) and the image formats used at various stages of the
# encoding process: "codec;av_source_pixel_format;cv_bridge_target_format;ros_encoding".
# - "ros_encoding" is the pixel format used by the original ROS message that was passed to
#   the "ffmpeg_encoder_decoder" module, e.g. "bgr8". This token allows the decoder later to restore
#   the original message format if desired.
# - "cv_bridge_target_format" is the pixel format that the cv_bridge converts
#   the "ros_encoding" into, e.g. . This is typically a format that is more suited for ffmpeg compression.
# - "av_source_pixel_format" is the pixel format that libswscale converts the "cv_bridge_target_format"
#   into. This token uses libav convention (e.g. "yuv420p"), and denotes the pixel format
#   that is fed into the libav encoder.
# - "codec" is the codec used for ultimately encoding the packet, e.g. "h264", "hevc" etc.
#   Follows libav convention.
# See the ffmpeg_encoder_decoder package for more details

string encoding	  # encoding used, see above
uint64 pts        # packet pts
uint8  flags      # packet flags
bool is_bigendian # true if machine stores in big endian format
uint8[] data      # ffmpeg compressed payload
