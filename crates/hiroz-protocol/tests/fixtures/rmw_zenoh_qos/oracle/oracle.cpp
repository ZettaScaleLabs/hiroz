// White-box oracle: the production rmw_zenoh liveliness code, unmodified, driven by a tiny main.
//   oracle qos          -> TSV of a QoS grid: fields, qos_to_keyexpr(), canonical token, gid
//   oracle canon        -> stdin tokens, one per line -> canonical token and gid
//   oracle emit         -> stdin entity rows -> the token rmw_zenoh itself writes, and its gid
//   oracle decode       -> stdin QoS chunks, one per line -> keyexpr_to_qos fields
#include <array>
#include <cstdint>
#include <iomanip>
#include <iostream>
#include <cstring>
#include <sstream>
#include <unordered_map>
#include <string>
#include <vector>

#include "liveliness_utils.hpp"
#include "qos.hpp"
#include "rmw/qos_profiles.h"

using namespace rmw_zenoh_cpp;

struct MakeId : zenoh::Id {explicit MakeId(::z_id_t z) : zenoh::Id(z) {}};

static std::string gid_hex(const liveliness::Entity & e)
{
  std::ostringstream o;
  o << std::hex << std::setfill('0');
  for (auto b : e.copy_gid()) {o << std::setw(2) << int(b);}
  return o.str();
}

static void emit(const std::string & declared)
{
  auto e = liveliness::Entity::make(declared);
  if (!e) {std::cout << "NULL\t-\t-\n"; return;}
  std::cout << declared << '\t' << e->liveliness_keyexpr() << '\t' << gid_hex(*e) << '\n';
}

int main(int argc, char ** argv)
{
  const std::string mode = argc > 1 ? argv[1] : "canon";
  const std::string head =
    "@ros2_lv/0/c9fa0e9e9d8c5744cef446c8628762dc/9/100/MS/%/%/oracle/%u8/"
    "std_msgs::msg::dds_::UInt8MultiArray_/RIHS01_5687e861b8d307a5e48b7515467ae7a5fc2daf805bd0ce6d8e9e604bade9f385/";
  if (mode == "canon") {
    for (std::string line; std::getline(std::cin, line);) {emit(line);}
    return 0;
  }
  if (mode == "decode") {
    // stdin: one QoS chunk per line -> parsed fields (keyexpr_to_qos), or NULL
    for (std::string line; std::getline(std::cin, line);) {
      auto q = liveliness::keyexpr_to_qos(line);
      if (!q) {std::cout << line << "\tNULL\n"; continue;}
      std::cout << line << '\t' << q->reliability << ' ' << q->durability << ' ' << q->history << ' ' << q->depth << ' '
                << q->deadline.sec << ' ' << q->deadline.nsec << ' ' << q->lifespan.sec << ' ' << q->lifespan.nsec << ' '
                << q->liveliness << ' ' << q->liveliness_lease_duration.sec << ' ' << q->liveliness_lease_duration.nsec << '\n';
    }
    return 0;
  }
  if (mode == "emit") {
    // stdin TSV: domain zidhex32 nid id ns name enclave topic type hash | 11 qos ints | backends ("-" or n=m|n=m)
    // The real C++ writer builds the token: Entity::make(zid, nid, id, type, node, topic).
    for (std::string line; std::getline(std::cin, line);) {
      std::vector<std::string> f; std::stringstream ss(line); std::string c;
      while (std::getline(ss, c, '\t')) {f.push_back(c);}
      if (f.size() != 22) {std::cout << "BADROW\n"; continue;}
      ::z_id_t zraw{};
      for (int i = 0; i < 16; ++i) {zraw.id[i] = static_cast<uint8_t>(std::stoi(f[1].substr(2 * i, 2), nullptr, 16));}
      zenoh::Id zid = MakeId(zraw);
      rmw_qos_profile_t q = rmw_qos_profile_default;
      q.reliability = static_cast<rmw_qos_reliability_policy_e>(std::stoi(f[10]));
      q.durability = static_cast<rmw_qos_durability_policy_e>(std::stoi(f[11]));
      q.history = static_cast<rmw_qos_history_policy_e>(std::stoi(f[12]));
      q.depth = std::stoul(f[13]);
      q.deadline = {std::stoull(f[14]), std::stoull(f[15])};
      q.lifespan = {std::stoull(f[16]), std::stoull(f[17])};
      q.liveliness = static_cast<rmw_qos_liveliness_policy_e>(std::stoi(f[18]));
      q.liveliness_lease_duration = {std::stoull(f[19]), std::stoull(f[20])};
      std::optional<std::unordered_map<std::string, std::string>> bm;
      if (f[21] != "-") {
        bm.emplace();
        std::stringstream bs(f[21]); std::string e;
        while (std::getline(bs, e, '|')) {
          auto eq = e.find('='); (*bm)[e.substr(0, eq)] = e.substr(eq + 1);
        }
      }
      liveliness::NodeInfo node(std::stoul(f[0]), f[4], f[5], f[6]);
      liveliness::TopicInfo topic(std::stoul(f[0]), f[7], f[8], f[9], q, bm);
      auto e = liveliness::Entity::make(zid, f[2], f[3], liveliness::EntityType::Subscription, node, topic);
      std::cout << (e ? e->liveliness_keyexpr() : "NULL") << '\t' << (e ? gid_hex(*e) : "-") << '\t' << zid.to_string() << '\n';
    }
    return 0;
  }
  const rmw_time_t inf = RMW_DURATION_INFINITE;
  const rmw_time_t t1{1, 0}, tq{0, 500000000}, t2{2, 0}, t3{3, 0};
  std::cout << "# rel dur hist depth dl_s dl_ns ls_s ls_ns lv lease_s lease_ns\tqos\tcanonical_token\tgid\n";
  for (int rel : {1, 2}) for (int dur : {1, 2})
    for (auto h : {std::pair<int, size_t>{1, 1}, {1, 5}, {1, 10}, {1, 42}, {1, 100}, {2, 42}})
      for (auto dl : {inf, t1, tq}) for (auto ls : {inf, t2})
        for (int lv : {1, 3}) for (auto lease : {inf, t3}) {
          rmw_qos_profile_t q = rmw_qos_profile_default;
          q.reliability = static_cast<rmw_qos_reliability_policy_e>(rel);
          q.durability = static_cast<rmw_qos_durability_policy_e>(dur);
          q.history = static_cast<rmw_qos_history_policy_e>(h.first);
          q.depth = h.second; q.deadline = dl; q.lifespan = ls;
          q.liveliness = static_cast<rmw_qos_liveliness_policy_e>(lv);
          q.liveliness_lease_duration = lease;
          const std::string qos = liveliness::qos_to_keyexpr(q);
          std::cout << rel << ' ' << dur << ' ' << h.first << ' ' << h.second << ' ' << dl.sec << ' ' << dl.nsec
                    << ' ' << ls.sec << ' ' << ls.nsec << ' ' << lv << ' ' << lease.sec << ' ' << lease.nsec << '\t' << qos << '\t';
          auto e = liveliness::Entity::make(head + qos + "/backends:cpu:");
          std::cout << (e ? e->liveliness_keyexpr() : "NULL") << '\t' << (e ? gid_hex(*e) : "-") << '\n';
        }
}
