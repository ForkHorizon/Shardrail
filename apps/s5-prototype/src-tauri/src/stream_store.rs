use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tauri::ipc::Channel;

#[derive(Default)]
pub struct StreamStore {
    rows: Vec<Value>,
    version: u64,
    last_notified: u64,
    notification_outstanding: bool,
    agent_chunk_count: usize,
    channel: Option<Channel<Value>>,
}

impl StreamStore {
    pub fn set_channel(&mut self, channel: Channel<Value>) {
        self.channel = Some(channel);
    }

    pub fn append(&mut self, frame: Value) {
        if frame["event"]["kind"] == "agent_message_chunk" {
            self.agent_chunk_count += 1;
        }
        self.rows.push(frame);
        self.version += 1;
    }

    pub fn window(&self, start: usize, limit: usize) -> Value {
        let total = self.rows.len();
        let start = start.min(total);
        let end = start.saturating_add(limit.min(256)).min(total);
        json!({ "start": start, "total": total, "version": self.version, "agentChunkCount": self.agent_chunk_count, "rows": self.rows[start..end] })
    }

    pub fn notify(&mut self) -> Result<(), String> {
        let Some(channel) = self.channel.clone() else {
            return Ok(());
        };
        let Some(notice) = self.begin_notification() else {
            return Ok(());
        };
        if let Err(error) = channel.send(notice) {
            self.notification_outstanding = false;
            return Err(error.to_string());
        }
        Ok(())
    }

    pub fn acknowledge(&mut self, version: u64) -> Result<(), String> {
        if self.notification_outstanding && version >= self.last_notified && version <= self.version
        {
            self.notification_outstanding = false;
            self.last_notified = version;
        }
        self.notify()
    }

    fn begin_notification(&mut self) -> Option<Value> {
        if self.notification_outstanding || self.last_notified == self.version {
            return None;
        }
        self.last_notified = self.version;
        self.notification_outstanding = true;
        Some(
            json!({ "version": self.version, "total": self.rows.len(), "agentChunkCount": self.agent_chunk_count }),
        )
    }
}

pub type SharedStreamStore = Arc<Mutex<StreamStore>>;

#[cfg(test)]
mod tests {
    use super::StreamStore;
    use serde_json::json;
    use std::time::Instant;

    #[test]
    fn only_one_notice_is_outstanding_and_acknowledgement_collapses_to_latest_version() {
        let mut store = StreamStore::default();
        store.append(json!({ "cursor": 1 }));
        assert_eq!(store.begin_notification().unwrap()["version"], 1);
        for cursor in 2..=50_000 {
            store.append(json!({ "cursor": cursor }));
        }
        assert!(store.begin_notification().is_none());
        store.acknowledge(1).unwrap();
        let next = store.begin_notification().unwrap();
        assert_eq!(next["version"], 50_000);
        assert_eq!(next["total"], 50_000);
    }

    #[test]
    #[ignore = "release throughput target; run with cargo test --release -- --ignored"]
    fn burst_retains_all_rows_and_range_queries_stay_bounded() {
        let mut store = StreamStore::default();
        let started = Instant::now();
        for cursor in 0_usize..50_000 {
            let encoded = format!(r#"{{"cursor":{cursor}}}"#);
            store.append(serde_json::from_str(&encoded).unwrap());
            let _ = store.window(cursor.saturating_sub(63), 64);
        }
        let first = store.window(0, 256);
        let last = store.window(49_744, 256);
        let elapsed = started.elapsed().as_secs_f64();
        let operations_per_second = 50_000.0 / elapsed;
        eprintln!("S5 store burst: 50000 appends + 50000 range queries in {:.3} ms ({operations_per_second:.0} append/query pairs/s)", elapsed * 1000.0);
        assert!(
            operations_per_second > 50_000.0,
            "{operations_per_second:.0} appends/s"
        );
        assert_eq!(first["total"], 50_000);
        assert_eq!(last["rows"].as_array().unwrap().len(), 256);
        assert_eq!(last["rows"][0]["cursor"], 49_744);
        assert_eq!(
            store.window(0, usize::MAX)["rows"]
                .as_array()
                .unwrap()
                .len(),
            256
        );
    }
}
