//! A ROS 2 bag in rosbag2's SQLite storage (`.db3`), read without ROS.
//!
//! The file holds two tables: `topics(id, name, type, serialization_format, ...)` and
//! `messages(id, topic_id, timestamp, data)`, with one serialized message per row and
//! `timestamp` the receive time in nanoseconds. A file-compressed bag (`.db3.zstd`) must
//! be uncompressed first.

use std::path::Path;

use rusqlite::{Connection, OpenFlags};

/// One topic of a bag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BagTopic {
    /// The row id the messages name it by.
    pub id: i64,
    /// The topic name.
    pub name: String,
    /// The ROS 2 type, `package/msg/Name`.
    pub ros2_type: String,
    /// The serialization, `cdr` for every bag rosbag2 records.
    pub serialization_format: String,
    /// How many messages the bag holds of it.
    pub message_count: u64,
}

/// One message of a bag, borrowed while the read visits it.
#[derive(Debug, Clone, Copy)]
pub struct BagMessage<'a> {
    /// The id of its topic, [`BagTopic::id`].
    pub topic_id: i64,
    /// The receive time, in nanoseconds since 1970.
    pub timestamp_nanos: i64,
    /// The serialized message.
    pub data: &'a [u8],
}

/// An open bag, read-only.
#[derive(Debug)]
pub struct Ros2Bag {
    connection: Connection,
}

fn sql_error(path: &Path, error: &rusqlite::Error) -> String {
    format!("{}: {error}", path.display())
}

impl Ros2Bag {
    /// Open the bag at `path`, read-only.
    ///
    /// # Errors
    /// A message when the file does not open as SQLite or has no rosbag2 tables.
    pub fn open(path: &Path) -> Result<Self, String> {
        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let connection =
            Connection::open_with_flags(path, flags).map_err(|error| sql_error(path, &error))?;
        let tables: i64 = connection
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name IN ('topics', 'messages')",
                [],
                |row| row.get(0),
            )
            .map_err(|error| sql_error(path, &error))?;
        if tables != 2 {
            return Err(format!(
                "{}: not a rosbag2 SQLite bag: it has no topics and messages tables",
                path.display()
            ));
        }
        Ok(Self { connection })
    }

    /// Every topic, with its message count, in the order of its id.
    ///
    /// # Errors
    /// A message when the tables do not read as rosbag2's.
    pub fn topics(&self) -> Result<Vec<BagTopic>, String> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT t.id, t.name, t.type, t.serialization_format, count(m.id) \
                 FROM topics t LEFT JOIN messages m ON m.topic_id = t.id \
                 GROUP BY t.id ORDER BY t.id",
            )
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map([], |row| {
                let count: i64 = row.get(4)?;
                Ok(BagTopic {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    ros2_type: row.get(2)?,
                    serialization_format: row.get(3)?,
                    message_count: u64::try_from(count).unwrap_or(0),
                })
            })
            .map_err(|error| error.to_string())?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|error| error.to_string())
    }

    /// Visit every message in the order of its receive time, then of its row id, until
    /// `visit` returns an error, which the read returns.
    ///
    /// # Errors
    /// A message when a row does not read, or the error of `visit`.
    pub fn for_each_message(
        &self,
        mut visit: impl FnMut(BagMessage<'_>) -> Result<(), String>,
    ) -> Result<(), String> {
        let mut statement = self
            .connection
            .prepare("SELECT topic_id, timestamp, data FROM messages ORDER BY timestamp, id")
            .map_err(|error| error.to_string())?;
        let mut rows = statement.query([]).map_err(|error| error.to_string())?;
        while let Some(row) = rows.next().map_err(|error| error.to_string())? {
            let topic_id: i64 = row.get(0).map_err(|error| error.to_string())?;
            let timestamp_nanos: i64 = row.get(1).map_err(|error| error.to_string())?;
            let data = row.get_ref(2).map_err(|error| error.to_string())?;
            let data = data.as_blob().map_err(|error| error.to_string())?;
            visit(BagMessage { topic_id, timestamp_nanos, data })?;
        }
        Ok(())
    }
}
