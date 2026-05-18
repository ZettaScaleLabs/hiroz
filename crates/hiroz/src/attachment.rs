use zenoh::bytes::ZBytes;
use zenoh_ext::{ZDeserializer, ZSerializer};

use crate::time::{ZClock, ZTime};

const RMW_GID_STORAGE_SIZE: usize = 16;

pub type GidArray = [u8; RMW_GID_STORAGE_SIZE];

#[derive(Clone, Hash)]
pub struct Attachment {
    pub sequence_number: i64,
    pub source_timestamp: i64,
    pub source_gid: GidArray,
}

impl Attachment {
    pub fn new(sn: i64, gid: GidArray) -> Self {
        Self::with_source_time(
            sn,
            gid,
            ZTime::from_system_time(std::time::SystemTime::now()),
        )
    }

    pub fn with_clock(sn: i64, gid: GidArray, clock: &ZClock) -> Self {
        Self::with_source_time(sn, gid, clock.now())
    }

    pub fn with_source_time(sn: i64, gid: GidArray, source_time: ZTime) -> Self {
        Self {
            sequence_number: sn,
            source_timestamp: source_time.as_unix_nanos(),
            source_gid: gid,
        }
    }

    pub fn source_time(&self) -> ZTime {
        ZTime::from_unix_nanos(self.source_timestamp)
    }
}

impl TryFrom<&ZBytes> for Attachment {
    type Error = zenoh::Error;
    fn try_from(value: &ZBytes) -> Result<Self, Self::Error> {
        let mut des = ZDeserializer::new(value);
        let sequence_number = des.deserialize::<i64>()?;
        let source_timestamp = des.deserialize::<i64>()?;
        let source_gid = des.deserialize::<GidArray>()?;
        Ok(Attachment {
            sequence_number,
            source_timestamp,
            source_gid,
        })
    }
}

impl From<Attachment> for ZBytes {
    fn from(value: Attachment) -> Self {
        let mut ser = ZSerializer::new();
        ser.serialize(value.sequence_number);
        ser.serialize(value.source_timestamp);
        ser.serialize(value.source_gid);
        ser.finish()
    }
}
