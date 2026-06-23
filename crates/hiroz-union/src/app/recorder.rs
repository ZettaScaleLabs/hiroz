//! SQLite-backed recording of topic metrics over time.

use std::sync::Arc;

use parking_lot::Mutex;
use rusqlite::Connection;

pub struct Recorder {
    conn: Arc<Mutex<Connection>>,
    active: bool,
    recording_id: Option<i64>,
}

impl Recorder {
    pub fn open(path: &str) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let conn = Connection::open(path)?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS metrics (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp INTEGER NOT NULL,
                topic TEXT NOT NULL,
                msgs_sec REAL,
                bytes_sec REAL,
                avg_payload INTEGER
            )",
            [],
        )?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp INTEGER NOT NULL,
                topic TEXT NOT NULL,
                payload BLOB,
                size INTEGER
            )",
            [],
        )?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS recordings (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                start_time INTEGER NOT NULL,
                end_time INTEGER,
                topics TEXT NOT NULL
            )",
            [],
        )?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS recording_samples (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                recording_id INTEGER NOT NULL,
                timestamp INTEGER NOT NULL,
                topic TEXT NOT NULL,
                rate REAL,
                bandwidth REAL,
                FOREIGN KEY (recording_id) REFERENCES recordings(id)
            )",
            [],
        )?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            active: false,
            recording_id: None,
        })
    }

    pub fn start(
        &mut self,
        topics: &[String],
    ) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
        if topics.is_empty() {
            return Err("No topics to record. Add topics with 'm' in Topics tab first.".into());
        }
        let conn = self.conn.lock();
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs() as i64;
        let topics_json = serde_json::to_string(&topics)?;
        conn.execute(
            "INSERT INTO recordings (start_time, topics) VALUES (?1, ?2)",
            rusqlite::params![timestamp, topics_json],
        )?;
        let recording_id = conn.last_insert_rowid();
        drop(conn);
        self.recording_id = Some(recording_id);
        self.active = true;
        Ok(topics.len())
    }

    pub fn stop(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let recording_id = self.recording_id.take();
        self.active = false;
        if let Some(id) = recording_id {
            let conn = self.conn.lock();
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs() as i64;
            conn.execute(
                "UPDATE recordings SET end_time = ?1 WHERE id = ?2",
                rusqlite::params![timestamp, id],
            )?;
        }
        Ok(())
    }

    pub fn toggle(
        &mut self,
        topics: &[String],
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        if self.active {
            self.stop()?;
            Ok("Recording stopped".to_string())
        } else {
            let count = self.start(topics)?;
            Ok(format!("Recording {} topics", count))
        }
    }

    pub fn record_sample(&self, topic: &str, rate: f64, bandwidth: f64) {
        let Some(recording_id) = self.recording_id else {
            return;
        };
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let conn = self.conn.lock();
        let _ = conn.execute(
            "INSERT INTO recording_samples (recording_id, timestamp, topic, rate, bandwidth)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![recording_id, timestamp, topic, rate, bandwidth],
        );
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn recording_id(&self) -> Option<i64> {
        self.recording_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recorder_start_stop_creates_row() {
        let mut r = Recorder::open(":memory:").unwrap();
        let count = r.start(&["/chatter".to_string()]).unwrap();
        assert_eq!(count, 1);
        assert!(r.is_active());
        r.stop().unwrap();
        assert!(!r.is_active());
        assert!(r.recording_id().is_none());
    }

    #[test]
    fn recorder_toggle_idempotent() {
        let mut r = Recorder::open(":memory:").unwrap();
        let msg1 = r.toggle(&["/foo".to_string()]).unwrap();
        assert!(msg1.contains("Recording"));
        let msg2 = r.toggle(&["/foo".to_string()]).unwrap();
        assert!(msg2.contains("Stopped") || msg2.contains("stopped"));
    }

    #[test]
    fn recorder_empty_topics_errors() {
        let mut r = Recorder::open(":memory:").unwrap();
        let result = r.start(&[]);
        assert!(result.is_err());
    }
}
