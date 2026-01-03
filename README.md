# SimpleBus

A **stupidly fast**, single-node **message bus** with **multiple topics**, **durable persistence**, and **crash recovery**.

Events are tagged with:
- a **universal, orderable ID** (time-sortable, globally unique), and
- a **high-resolution timestamp**.

Consumers can subscribe to topics and replay **from a timestamp** or **from an event ID**, then continue streaming new events.

> This README is intentionally “spec-first”: it’s a detailed requirements document for a 2-day exploratory build.

---

## 1) Goals

### Functional goals
- **Publish** events to a named **topic**.
- **Persist** every event to disk (append-only) so the bus can:
  - **restart without data loss** (within the chosen durability mode), and
  - **recover cleanly** from partial writes / crashes.
- **Subscribe** to a topic:
  - starting **from the beginning**, **from a timestamp**, **from an event ID**, or **from “now”**,
  - replay backlog, then **tail live** events.
- Support **multiple topics** concurrently.
- Provide **monotonic ordering** per topic based on the orderable ID (and/or append order).
- Provide a minimal, ergonomic Rust API for embedding into other programs.

### Performance goals (qualitative)
- Optimize for **very high throughput** and **low overhead** on a single machine.
- Avoid unnecessary allocations in the hot path.
- Use **append-only logs**, batched flush, and cache-friendly in-memory indices.

---

## 2) Non-goals

- No distribution, replication, leader election, or consensus.
- No consumer groups / partition balancing / offsets committed to the server.
- No exactly-once processing end-to-end (the bus is at-least-once for replays).
- No cross-topic transactions.
- No pluggable storage backends beyond local filesystem.

---

## 3) Key Concepts

### Topics
A **topic** is an independent append-only event stream.

Each topic has:
- one active **append log** (and optionally rotated segments),
- a **cursor index** enabling lookup by:
  - event_id → file offset
  - timestamp → nearest file offset (approximate is OK for MVP)

### Event
An **event** is an immutable record:
- `event_id`: globally unique + orderable (time-sortable)
- `ts_unix_nanos`: unix timestamp (nanoseconds)
- `topic`: topic name (or topic id)
- `payload`: bytes (arbitrary)
- optional `headers`: small metadata map (string → string) *(optional for MVP)*

---

## 4) Event ID Specification

The bus must generate a **universal, orderable** identifier (in addition to a timestamp).

### Recommended options
Pick **one** for the MVP:

1) **UUIDv7** (time-ordered UUID)
- Pros: standardized format, widely recognized.
- Cons: requires a UUIDv7 generator implementation.

2) **ULID** (Universally Unique Lexicographically Sortable Identifier)
- Pros: simple, sortable, human-friendly.
- Cons: not a UUID, but still universal + sortable.

### Requirements for event_id
- **Globally unique** with extremely low collision risk.
- **Orderable**: if `A` is created before `B`, then `A < B` in lexicographic/byte order in almost all cases.
- Prefer **monotonicity within the same timestamp**:
  - If multiple events are created in the same millisecond/nanosecond bucket, IDs should still sort in creation order, if feasible.

### Representation
- Store as **16 bytes** (preferred) or **fixed 128-bit** representation.
- Expose in API as:
  - `EventId([u8; 16])` plus helper:
    - `to_string()` (base32/base16) for logs/debugging
    - `from_string()` for client input

---

## 5) Persistence & Crash Recovery

### Storage layout (filesystem)
A suggested directory structure:

```
data/
  bus.meta.json
  topics/
    metrics/
      log-00000001.seg
      log-00000002.seg
      index.snapshot
    logs/
      log-00000001.seg
      index.snapshot
```

- Each topic has **segment files** (or just a single log file for MVP).
- Each topic may have an **index snapshot** (optional) to speed recovery.

### Append-only log format (record framing)
Each record is appended as a framed binary blob:

```
[ MAGIC u32 ][ VERSION u16 ][ FLAGS u16 ]
[ RECORD_LEN u32 ]  // bytes after this field
[ EVENT_ID 16 bytes ]
[ TS_UNIX_NANOS i64 ]
[ TOPIC_LEN u16 ][ TOPIC bytes ]
[ PAYLOAD_LEN u32 ][ PAYLOAD bytes ]
[ HEADERS_LEN u32 ][ HEADERS bytes ]  // optional; can be 0 for MVP
[ CRC32 u32 ]  // checksum of the record body (from EVENT_ID to end)
```

**Requirements:**
- `MAGIC` and `VERSION` allow future upgrades.
- `RECORD_LEN` allows skipping records quickly.
- `CRC32` (or similar) allows detecting corruption and partial writes.
- On recovery, if the final record is:
  - truncated, or
  - CRC mismatch,
  the bus must **truncate the log** back to the last valid record and continue.

### Durability modes
Expose a configurable durability setting:

- `Durability::FsyncAlways`
  - `fsync`/`flush` after every publish.
  - safest, slowest.
- `Durability::FsyncBatch { max_events: N, max_millis: M }`
  - fsync periodically or after N events.
  - recommended default for performance.
- `Durability::OSBuffered`
  - rely on OS page cache (fastest, least safe).
  - acceptable for local dev/testing.

### Recovery procedure
On startup:
1. Load `bus.meta.json` (or create it if new).
2. For each topic:
   - open log segments in order
   - scan records sequentially:
     - validate framing + CRC
     - rebuild in-memory indices:
       - `event_id → offset`
       - `timestamp → offset` (coarse)
   - if corruption/truncation detected at tail:
     - truncate to last valid offset
3. After recovery, the bus is ready to:
   - accept new publishes
   - allow replays from id/timestamp

---

## 6) Subscription Model & Cursors

### Cursor types
A consumer subscribes with a `StartFrom` cursor:

- `StartFrom::Beginning`
- `StartFrom::Timestamp(i64 /* unix nanos */)`
- `StartFrom::EventId(EventId)`
- `StartFrom::Now` *(tail only; starts from end-of-log)*

### Behavior
- If subscribing from a timestamp:
  - start from the **first event whose timestamp >= requested timestamp**.
  - If only coarse timestamp index exists, it may start slightly earlier and filter.
- If subscribing from an event id:
  - start from the **event with that id**, or the next event if “exclusive” mode is enabled.
  - Provide a config flag:
    - `Inclusive` (default): include the matched event
    - `Exclusive`: start after the matched event
- After backlog replay, subscription continues **live**.

### Delivery semantics
- **At-least-once** for replay scenarios:
  - after a restart, consumers replaying from an older cursor may see duplicates.
- In a single runtime (no restart), live delivery should be **exactly-once per subscriber** (best effort).

---

## 7) Concurrency Model (Recommended)

To keep it fast and simple:

### Per-topic single writer
- Each topic runs a **single writer task/thread** responsible for:
  - generating event IDs
  - appending records to log
  - updating in-memory indices
  - publishing to subscribers

Producers:
- send `PublishRequest` to the topic writer via an MPSC channel.

Consumers:
- receive events via a `broadcast`-like mechanism or per-subscriber MPSC channels.

**Benefits:**
- avoids lock contention on file handles
- preserves total order per topic naturally
- keeps indices consistent

---

## 8) Public Rust API (MVP)

### Types
```rust
pub struct SimpleBus { /* ... */ }
pub struct TopicHandle { /* ... */ }

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct EventId([u8; 16]);

pub struct EventRecord {
    pub event_id: EventId,
    pub ts_unix_nanos: i64,
    pub topic: String,
    pub payload: Vec<u8>,
    // pub headers: Option<HashMap<String, String>>, // optional
}

pub enum StartFrom {
    Beginning,
    Now,
    Timestamp(i64),
    EventId(EventId),
}

pub enum Durability {
    FsyncAlways,
    FsyncBatch { max_events: usize, max_millis: u64 },
    OSBuffered,
}
```

### Bus lifecycle
```rust
impl SimpleBus {
    pub fn open(data_dir: impl AsRef<Path>, durability: Durability) -> anyhow::Result<Self>;
    pub fn topic(&self, name: &str) -> anyhow::Result<TopicHandle>;
    pub fn shutdown(self) -> anyhow::Result<()>;
}
```

### Publishing
```rust
impl TopicHandle {
    pub async fn publish(&self, payload: Vec<u8>) -> anyhow::Result<EventId>;
    // optional:
    // pub async fn publish_with_headers(&self, payload: Vec<u8>, headers: HashMap<String, String>) -> anyhow::Result<EventId>;
}
```

### Subscribing
Return an async stream/receiver for `EventRecord`:
```rust
impl TopicHandle {
    pub async fn subscribe(&self, start: StartFrom) -> anyhow::Result<Subscription>;
}

pub struct Subscription {
    // receive events
    pub async fn next(&mut self) -> Option<EventRecord>;
}
```

**Requirement:** A subscription must support:
- backlog replay from disk (based on cursor)
- then live tail

---

## 9) Indexing Requirements (MVP-Friendly)

### In-memory indices
For each topic keep:
- `HashMap<EventId, u64 /* offset */>`
- a timestamp index (coarse):
  - `Vec<(i64 /* ts */, u64 /* offset */)>` sampled every `K` events

### Index snapshot (optional but recommended)
Periodically write:
- `index.snapshot` containing:
  - last segment id
  - last offset
  - compact representation of indices

On startup:
- load snapshot if present
- validate it against log tail (by comparing last known offset/event_id)
- continue scanning from there

**Acceptable MVP shortcut:** no snapshot; rebuild indices by scanning logs (still OK for 2-day scope).

---

## 10) File Rotation (Optional, but Nice)

To avoid giant files:
- rotate segment when it reaches `max_segment_bytes` (e.g., 256MB)
- naming: `log-{segment_no:08}.seg`
- keep segments immutable once rotated
- active segment is append-only

---

## 11) Configuration

Provide a `SimpleBusConfig`:
- `data_dir`
- `durability`
- `max_segment_bytes` (optional)
- `timestamp_index_stride` (e.g., sample every 10_000 events)
- `channel_capacity` (producer → writer)
- `subscriber_buffer` size

---

## 12) Observability

### Required logs (MVP)
- bus startup + recovered topics
- detected truncation/corruption and truncation offsets
- segment rotation
- publish rates (optional)
- subscriber join/leave (optional)

### Optional metrics
- events/sec per topic
- fsync latency
- queue depth per topic writer
- subscriber lag (approx)

---

## 13) Testing Requirements

### Unit tests
- event id ordering (monotonic behavior)
- record encoding/decoding roundtrip
- CRC detects corruption
- “truncate tail” recovery

### Integration tests
- publish N events, restart, replay from:
  - beginning
  - timestamp
  - event id
- simulate crash during write (write partial record), restart, ensure recovery truncates and continues

### Property tests (optional)
- random event sequences and restart points

---

## 14) Benchmarking (Exploratory)

Provide a small benchmark harness (even a CLI mode is fine):
- single topic publish throughput
- multiple topics publish throughput
- subscribe + tail throughput

Measure:
- events/sec
- p50/p99 publish latency (rough)
- CPU usage (rough)

---

## 15) CLI (Optional but Helpful)

A minimal CLI can help demo quickly:

- `simple-bus init --dir data/`
- `simple-bus publish --topic metrics --file payload.bin`
- `simple-bus tail --topic metrics --from now`
- `simple-bus replay --topic metrics --from-ts 1700000000000000000`

---

## 16) Security / Safety

- Do not execute payloads; treat as opaque bytes.
- Validate all lengths during decode to avoid OOM.
- Put sane limits:
  - max topic name length
  - max payload size (configurable)

---

## 17) 2-Day MVP Checklist

### Day 1
- [ ] Define record format + encode/decode + CRC
- [ ] Implement per-topic writer task/thread
- [ ] Implement `publish()` with durable append
- [ ] Implement basic subscribe:
  - [ ] replay from beginning
  - [ ] tail live
- [ ] Basic recovery: scan log, truncate bad tail

### Day 2
- [ ] Implement `StartFrom::Timestamp` (coarse index OK)
- [ ] Implement `StartFrom::EventId` (hashmap lookup)
- [ ] Add multi-topic support + topic directory layout
- [ ] Add durability modes (batch fsync)
- [ ] Integration tests: restart + replay
- [ ] Simple benchmark or CLI demo

---

## 18) Open Questions (Decide Early)

- Which event_id format?
  - UUIDv7 vs ULID (choose one for MVP)
- Do we store topic name per record or deduce by file?
  - MVP: deduce by file (omit topic bytes in record) to reduce overhead.
- Do we support headers?
  - MVP: no; add later if needed.
- Timestamp precision:
  - `unix_nanos` recommended (but monotonic clock vs system clock considerations).

---

## 19) Example Usage (Conceptual)

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let bus = SimpleBus::open(
        "data",
        Durability::FsyncBatch { max_events: 10_000, max_millis: 50 }
    )?;
    let metrics = bus.topic("metrics")?;

    // publish
    let id = metrics.publish(b"cpu=0.82".to_vec()).await?;
    println!("published {id:?}");

    // subscribe from timestamp
    let mut sub = metrics.subscribe(StartFrom::Timestamp(1700000000000000000)).await?;
    while let Some(evt) = sub.next().await {
        println!("{:?} {:?}", evt.event_id, evt.payload);
    }

    Ok(())
}
```

---

## 20) License

Choose one:
- MIT
- Apache-2.0

(For an exploratory repo, MIT is a common default.)
