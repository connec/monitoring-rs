# Metric collection part 1 – database design

We want to introduce metric collection to our project.
[Previously](06-log-collection-part-5-recentring.md) we conceived the following architecture:

!["Kubernetes API" and "UI" now sandwich a "DaemonSet". The "DaemonSet" is underpinned by a "Kubernetes node". The "DaemonSet" includes a box for "log files volume" (arrow from "Kubernettes node") and a box for `monitoring_rs`. `monitoring_rs` has the same contents as before, in reverse order and with "Kubernetes API" and "log files volume" now feeding into `log_collector`.](../media/17-metric-collection-part-1-database-design/post-6-architecture.png)

This is pretty close to what we implemented, except we so far have a `Deployment` rather than a `DaemonSet`, and the UI is served by the `api` module.

Let's take the opportunity of introducing a significant new functional area to think a bit more about our architecture and try to design one to account for metric collection.

## Thinking about a common data store

Back in our very first post, [Discovery](00-discovery.md), we talked about a "common data store", and concluded:

> This sounds like some kind of time-series database, with additional support for sub-tree free-text search. I wonder if that's even sensible?

We also spent some time [waxing philosophical](14-log-collection-part-13-waxing-philosophical.md) at the beginning of that eponymous post, where we considered separatin 'log files' from 'collectors' in order to deduplicate metadata.
The insight driving that tangent was that metadata is really associated with log files, rather than log entries.
Since log files come and go very slowly relative to log entries, leaning into this separation in our design could make our log collector more efficient.

Now that we're thinking about introducing metrics, we could think about joining these two loose ends by considering how we might design a database for both logs and metrics that accounts for the separation between metadata and entries.

### "Time series database"

The Wikipedia strapline for [Time series database](https://en.wikipedia.org/wiki/Time_series_database) says:

> A time series database (TSDB) is a software system that is optimized for storing and serving time series through associated pairs of time(s) and value(s).

And [Time series](https://en.wikipedia.org/wiki/Time_series) itself is described:

> A time series is a series of data points indexed (or listed or graphed) in time order.

This obviously seems like a very good fit for metrics, and indeed [Prometheus](https://prometheus.io/) – arguably the de-facto standard monitoring tool for Kubernetes – describes itself (in the page title) as a "Monitoring system & time series database".
Prometheus describes [its data model](https://prometheus.io/docs/concepts/data_model/) thus:

> Prometheus fundamentally stores all data as time series: streams of timestamped values belonging to the same metric and the same set of labeled dimensions.

Here we see the first mention of what we've so far called "metadata", which Prometheus calls "labeled dimensions".
"Dimension" makes sense for metrics, whose values can be considered measurements.
Let's steal "labels" to replace what we've so far called "metadata" to bring our terminology more in-line with the context (as Sun Tzu said, "to know your enemy, you must become your enemy" – surely the same lesson applies if we want to become a time series database?).
This seems equally applicable for logs and metrics.

The same document talks about "samples":

> Samples form the actual time series data. Each sample consists of:
>
> - a float64 value
> - a millisecond-precision timestamp

Again this seems grand for metrics, but are logs so different?
Perhaps we can just be generic over the value type, and consider a 'sample' to be a millisecond-precision timestamp and a value – with all samples from a particular series having the same type.
That way, metrics could be peristed as samples with 64-bit floating point values and logs could be persisted as samples with string values.
This would definitely introduce some complexity in the persistence since strings can be of arbitrary length, so we might expect storing logs this way to be less efficient than storing metrics, but we will worry about that another time ("he will win who knows when to fight and when not to fight").

Terminology-wise, "samples" doesn't feel great for logs.
For that matter, neither does "series".
Typical logging terminology would be "events" and "streams", however these are fairly overload terms themselves.
We've also considered "source" before when thinking log files.

### Common terminology

The problem is there's a conceptual rift between logs and metrics in terms of how they are sourced:

- Logs are *pushed* by services as relevant events happen.
  Describing them as events has the right implications for their characteristics – they are emitted in response to phenomena, may be arbitrarily detailed, not periodic, interpretation is context-sensitive, cardinality matters, reliability may be critical for a series to be consistent.

- Metrics are *pulled* by monitoring.
  Describing them as samples has the right implications for their characteristics – they are taken periodically, represent a snapshot of a particular measure, are simple values, interpretation is context-free, reliability is rarely critical.

Overall, the characteristics of log events look more challenging from a persistence perspective, particularly the need for detail and reliability.
If we want our design to work, we may want to focus on designing for logging, and hope that we may later specialise the simpler case of metric collection.
Let's use this to guide our terminology and settle for:

- "Event" – a discreet entry in our database, consisting of a value and the timestamp at which that value was observed.
- "Labels" – a set of key-value pairs that describe context or metadata for an event.
- "Stream" – a sequence of events matching specific labels, ordered by observation time.

This seems general enough.
Let's check that we can comfortably map what we know about logs and metrics to this terminology:

- Log events and metric samples are "events".
  The value for log events will be a string, whereas for metrics the value will be a 64-bit float.
- File system metadata, Kubernetes metadata, and metric dimensions are "labels".
– Log files can viewed as a "stream", labelled by the associated file system and Kubernetes metadata and containing the log events written to the file.
  Similarly, metric series are "streams" labelled by the associated dimensions and containing the values observed for that metric.

### Prior art

Before giving into the desire to run off and design an entirely new database system, we should look at what exists already.
The leading databases for logs and metrics today may well be Elasticsearch for logs and Prometheus for metrics.

Elasticsearch, as the name might suggest, was originally designed to support horizontally scalable document search.
The data model is document-oriented.
Document indexing and searchin is driven by Apache Lucene.
Elasticsearch itself adds structures and protocols for managing the distribution of Lucene indexes across multiple nodes.
That said, these index management capabilities can be used to make log ingestion efficient by creating new indexes over time, such that the 'current' index is the only one being written to whilst historical indices are only read.
More recently this has been formalised into the Elasticsearch [data streams](https://www.elastic.co/guide/en/elasticsearch/reference/current/data-streams.html) feature.

Prometheus, with no clues in the name, is, as noted above, a monitoring system & time series database.
The [Writing a Time Series Database from Scratch](https://fabxc.org/tsdb/) write-up of the motivation and design of a new storage layer used in Prometheus 2.x is very useful for understanding how metrics are persisted.
The data directory layout gives a pretty good summary:

> ```
> ./data
> ├── b-000001
> │   ├── chunks
> │   │   ├── 000001
> │   │   ├── 000002
> │   │   └── 000003
> │   ├── index
> │   └── meta.json
> ├── b-000004
> │   ├── chunks
> │   │   └── 000001
> │   ├── index
> │   └── meta.json
> ├── b-000005
> │   ├── chunks
> │   │   └── 000001
> │   ├── index
> │   └── meta.json
> └── b-000006
>     ├── meta.json
>     └── wal
>         ├── 000001
>         ├── 000002
>         └── 000003
> ```

The database is split into non-overlapping blocks by time, which can make querying and deleting old data very efficient – you only need to visit/delete the relevant blocks.
Within a block, data is stored in multiple chunks which are described in the article as "raw chunks of data points for various series".
Chunks are sized to optimise compression, and are designed to be held open with `mmap` so that Prometheus can offload more memory management to the OS, which is pretty good at it.
The index "contains a lot of black magic allowing us to find labels, their possible values, entire time series and the chunks holding their data points".
The [index format documentation](https://github.com/prometheus/prometheus/blob/master/tsdb/docs/format/index.md) describes the contents in more detail:

1. The file begins with a symbol table, which is used to store all the distinct label names and values.
  This serves to reduce the storage required by the index, as other index sections can refer to the labels by their index in the symbol table.

2. Next is the 'series' section.
  Each series is described by its labels (as references to the symbol table) and chunk metadata, including the minimum and maximum timestamps in the chunk and a reference to the chunk.
  The [chunk format documentation](https://github.com/prometheus/prometheus/blob/master/tsdb/docs/format/chunks.md) tells us that the upper 4 bytes indicate the segment sequence number (presumably what are referred to as "chunk files" in the write-up), and the lower 4 bytes indicate the offset into the file.
  The format of the chunks themselves is not specified, but the write-up suggests they would be batches of samples from a single series that have been heavily compressed using techniques from Facebook's [Gorilla TSDB](https://www.vldb.org/pvldb/vol8/p1816-teller.pdf).

3. Next comes the label index, which contains the known values for each label name appearing in the block.
  The documentation notes that this is no longer used.

4. Next up, "postings" sections, each of which is a list of series references that contain a particular label pair.
  The label pairs themselves are stores in the postings offset table later in the index file.

5. Next, the label offset table, which is used to track label index sections and as with those sections is no longer used.

6. Penultimately, the postings offset table contains entries with a label pair and an offset into the postings sections, which will list the series references that contain that label pair.

7. Finally, there is a table of contents which is a fixed size and includes pointers to each other section of the index.

The other point of interest is that the 'head' block has a different structure.
The [head chunk format documentation](https://github.com/prometheus/prometheus/blob/master/tsdb/docs/format/head_chunks.md) shows they include a series reference and minimum and maximum timestamps for the chunk, since the head chunk has no index for that information.
The write-up indicates there's also a write ahead log, which has its own [format documenation](https://github.com/prometheus/prometheus/blob/master/tsdb/docs/format/wal.md).
The WAL records describe series (with a unique ID and the series' labels), samples (with the series ID, timestamp, and value), and tombstones (with the series ID, and minimum and maximum timestamps specifying an interval of samples that have been deleted).
The write-up suggests that the head block is maintained primarily in memory, with the WAL allowing recovery on crashes.
The format documentation and source suggests that the head chunks are also written to disk – it looks like this was introduced more recently ([prometheus/prometheus#6679](https://github.com/prometheus/prometheus/pull/6679)).

### Baby steps

OK, let's take a stab at an API.
We'll start by creating a new module:

```diff
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -21,6 +21,7 @@
 )]

 pub mod api;
+pub mod database;
 pub mod log_collector;
 pub mod log_database;

```

```rust
// src/database/mod.rs
//! A time-series-esque database for storing and querying append-only streams of events.
```

Now let's think about the types we'll need, starting from the top and working down.
Firstly, we should have a `Database` struct:

```diff
--- a/src/database/mod.rs
+++ b/src/database/mod.rs
@@ -1,2 +1,5 @@
 // src/database/mod.rs
 //! A time-series-esque database for storing and querying append-only streams of events.
+
+/// A time-series-esque database for storing and querying append-only stream of events.
+pub struct Database;
```

We've duplicated our module documentation for now, but these could diverge in future.
What inherent methods does our `Database` need?
We'll start with the obvious – `open`:

```diff
--- a/src/database/mod.rs
+++ b/src/database/mod.rs
@@ -1,5 +1,17 @@
 // src/database/mod.rs
 //! A time-series-esque database for storing and querying append-only streams of events.

+use std::path::Path;
+
 /// A time-series-esque database for storing and querying append-only stream of events.
 pub struct Database;
+
+/// Possible error situations when opening a database.
+pub type OpenError = std::io::Error;
+
+impl Database {
+    /// Open a database at the given `path`.
+    pub fn open(path: impl AsRef<Path>) -> Result<Self, OpenError> {
+        todo!()
+    }
+}
```

We've created an `OpenError` type alias in the hope that this will put us on track for more dilligent error handling.
Now, what should `open` do?
Let's try and write the documentation for the behaviour:

```diff
--- a/src/database/mod.rs
+++ b/src/database/mod.rs
@@ -11,6 +11,19 @@ pub type OpenError = std::io::Error;

 impl Database {
     /// Open a database at the given `path`.
+    ///
+    /// If `path` doesn't exist, it is created and an empty `Database` is constructed that will
+    /// write its data to `path`. If `path` exists, a `Database` is restored from its contents and
+    /// returned.
+    ///
+    /// # Errors
+    ///
+    /// - Any [`io::Error`]s that occur when reading or writing directories or files are propagated.
+    /// - If `path` is not a directory, a [`NotDirectory`] error is returned.
+    /// - If restoring from `path` fails, a [`RestoreError`] is returned.
+    ///
+    /// [`io::Error`]: std::io::Error
+    /// [`NotDirectory`]: OpenError::NotDirectory
     pub fn open(path: impl AsRef<Path>) -> Result<Self, OpenError> {
         todo!()
     }
```

In documenting the function we've identified a number of additional error scenarios that need types:

```diff
--- a/src/database/mod.rs
+++ b/src/database/mod.rs
@@ -7,7 +7,14 @@ use std::path::Path;
 pub struct Database;

 /// Possible error situations when opening a database.
-pub type OpenError = std::io::Error;
+pub enum OpenError {
+    Io(std::io::Error),
+    NotDirectory,
+    Restore(RestoreError),
+}
+
+/// Possible error situations when restoring a database.
+pub type RestoreError = std::io::Error;

 impl Database {
     /// Open a database at the given `path`.
```

We'll think about `RestoreError` more later, for now it's safe to assume IO errors will be possible.
We might also want to fold `RestoreError` into `OpenError`, but we'll worry about that later as well.
We'll leave `open` there for now and think about the rest of the `Database` API.

A key consideration for the `Database` API is whether we want to deal with "streams" explicitly or implicitly.

- If we want to manage streams explicitly, we would need methods to create and lookup streams based on labels.
  Methods for writing could appear on a `Stream` struct or on the `Database`, using some kind kind of stream identifier.

- Alternatively, we could let the `Database` manage streams implicitly.
  This is closer to our current `log_database`, where each log entry specifies its `metadata` which is used to compute the key for the 'stream' to which the entry belongs.

The downside of the 'implicit' approach that we've used so far is that writing ends up quite complicated.
We need to hash the metadata, check if a matching data file exists, create one if necessary, and finally write the entry to the file.
Hashing the metadata shouldn't be outrageously expensive, but we still need to iterate all the key-values and hash them, which isn't free.
This is mildly annoying, since we know that the metadata comes from log files which turn over *very* slowly compared to incoming log entries (in a fairly quiet cluster, after a week there were 14 log files and ~180,000 log entries).
This translates to ~180,000 clones of just 14 distinct `HashMap`s – very wasteful!

It could be possible to make the implicit approach more efficient by hashing metadata on the client side of the API, e.g. have `Database::insert` take the `key` as a parameter rather than the metadata.
We would rather keep the hashing as an implementation detail, however, and have the API maintain the idea that streams are identified by their labels.

Another consideration for 'explicit' vs. 'implicit' interfaces would be how we enable concurrency.
At first glance, we might hope that an 'explicit' interface would enable an interface like the following:

```rust
impl Database {
    pub fn open_stream(&mut self, labels: Labels) -> Result<Stream, _> {
        todo!()
    }
}

struct Stream;

impl Stream {
    pub fn push(&mut self, event: Event) -> Result<(), _> {
        todo!()
    }
}
```

This API is appealing because mutable access to the database would only be necessary when opening new streams, and stream insertion only needs mutable access to the relevant stream.
Contrast this with the current implementation, where every insertion needs to take a write lock.
This would mean only giving out a single instance of `Stream` for each actual stream though, or else we would need some other mechanism for protecting the underlying resources from concurrent mutation.
Furthermore, it wouldn't really work for queries, since queries might need to search multiple streams.

A variation of the current 'implicit' API that could be used more efficiently would be to simply split the `LogEntry` struct into two arguments: `&metadata` and `&line`.
This would allow clients to deduplicate the storage of `metadata` and use references to them when inserting.

Quite a few options!
Let's try to enumerate the ones we've discussed:

1. Explicit streams:
   1. Insertion via separate `Stream` resources, managed by label references.
   1. Insertion via separate `Stream` resources, managed by stream identifiers.
   1. Insertion via `Database` with stream identifiers.
1. Implicit streams:
   1. Insertion via `Database` with label reference.

Let's rule out 1.ii – this API would require consumers to manage both `Stream` resources and stream identifiers which seems sub-optimal.
The approach of handing out sub-interfaces for streams also sounds difficult to manage, and given the requirement to maintain appropriate invariants under concurrency we might reasonably assume that our `Stream` resource would need to contain references to parts of the `Database` state, limiting the value of this separation.
In fact, in order to maintain concurrency invariants a `Stream` resource would probably end up looking something like a `(&StreamId, &Database)` tuple.
Interacting with the `Stream` would, internally, be interacting with the `Database` via the included `StreamId`.
Similarly, insertion via `Database` with a label reference (2.i) could be built on top of insertion with stream identifiers (1.iii) by converting the label reference to a stream identifier, ideally via some `O(1)` lookup table rather than re-hashing them every time.

Another way we might try to think about all of this is by considering the closest equivalent `std` structure, and what we hope to gain over it.
From what we've talked about so far, this could look something like:

```rust
// We use a `BTreeMap` for labels because it implements `Hash`.
type alias Labels = BTreeMap<String, String>;

// This is a bit awkward – u32 can only represent ~50 days in milliseconds, whereas u64 can
// represent ~585 million years. There's no type inbetween. We would probably want to use delta
// encoding when persisting!
type alias Timestamp = u64;

// Nothing surprising here – a `Database` simply maps `Labels` to a logical `Stream`.
type alias Database = HashMap<Labels, Stream>;

// We can break a `Stream` into chunks, making it more efficient to drop old data (we can drop an
// entire `Chunk` at a time). The `(Timestamp, Timestamp)` key would be the maximum and minimum
// timestamps in the `Chunk`, and these must be non-overlapping.
type alias Stream = BTreeMap<(Timestamp, Timestamp), Chunk>;

// A chunk represents a contiguous sequence of `Event`s in a `Stream`.
type alias Chunk = Vec<Event>;

// An event is just a tuple of `Timestamp` and value, which is a `Vec<u8>`.
type alias Event = (Timestamp, Vec<u8>);
```

What would be the problems of using this directly as our database?

- Most importantly, the data needs to be persisted to disk.
  Most naively, we could periodically serialize the whole `Database` structure into a file.
  Towards the less-naive end of the spectrum, we might do something similar to Prometheus and serialize `Chunk`s from multiple `Stream`s into immutable blocks, along with indexes that would allow the overall structure to be restored.
  [`mmap`](https://man7.org/linux/man-pages/man2/mmap.2.html) is used heavily in Prometheus for persistance.

- The entire database should not live in memory.
  It might be good for performance to keep the latest `Chunk` for each `Stream` in memory, and if we
  were especially clever we might cache `Chunk`s that are frequently read.
  In general though, the bulk of the data should be on disk.
  Again, `mmap` could help here by offloading the responsibility for efficiently loading and storing to the OS, but advice suggests we would have to benchmark to know if it was really more performant.

- Concurrent reads and writes would ideally be possible.
  In particular, we would expect to query the database from different contexts than where we write the database, and ideally one shouldn't block the other – at least not from the API.

- With `Database` as the entrypoint, there's is a lot of indirection for writes.
  An `insert` function could look like:

  ```rust
  fn insert(database: &mut Database, labels: &Labels, event: Event) {
      // First we need to get the `Stream`, or otherwise insert it.
      let stream = match database.get_mut(labels) {
          Some(stream) => stream,
          None => database.entry(labels.clone()).or_insert(Stream::default()),
      };

      // Next we need to find the latest chunk in the stream, or otherwise insert one.
      let chunk = match stream.iter_mut().last() {
          Some((&(min, max), chunk)) if min <= event.0 && event.0 <= max => chunk,
          _ => stream
              .entry((event.0, std::u64::MAX))
              .or_insert(Chunk::default()),
      };

      // Finally, we can push the event.
      chunk.push(event);
  }
  ```

  Ideally, our `Database` should keep the latest chunks for each series with less indirection.
  Alternatively, our API could give out a `Stream`-oriented handle to consumers that would somehow write directly to the latest chunk with no indirection:

  ```rust
  impl Database {
      pub fn stream(&'a self, labels: &Labels) -> StreamWriter<'a> {
          // Magic to get a mutable `Stream` (and/or `Chunk`) from `&self` and also return it.
          todo!()
      }
  }
  ```

  This particular structure would probably make it difficult to batch multiple `Chunk`s from multiple series into blocks for persistence, but you get the idea.

Hrm.
This is all very complicated.

Thinking about concurrency:

- `Stream`s for logs could easily be written to from a single thread.
  This is because log streams are fundamentally backed by a single append-only file – there is no advantage to parallelism.

- `Stream`s for metrics could also be written to from a single thread, depending on how the collector works.
  We could assume similar behaviour as logs, in that the collector would enrich the metrics with per-source labels which would ultimately mean that metric streams would also be backed by a single source.

However, both of these lines of reasoning go out the window if we think about adding an HTTP API for writing logs/metrics (this might also give us the additional headache of having to write to historical chunks).

Alright, for now let's just add a `Database::push` method:

```diff
--- a/src/database/mod.rs
+++ b/src/database/mod.rs
@@ -34,4 +34,9 @@ impl Database {
     pub fn open(path: impl AsRef<Path>) -> Result<Self, OpenError> {
         todo!()
     }
+
+    /// Push a new `event` into the stream identified by `labels`.
+    pub fn push(&self, labels: _, event: _) -> Result<(), PushError> {
+        todo!()
+    }
 }
```

This won't compile, of course, because we don't have types for `labels` or `event`.
Ultimately these could probably live at the crate root, but for now let's define them in `database`:

```diff
--- a/src/database/mod.rs
+++ b/src/database/mod.rs
@@ -1,11 +1,31 @@
 // src/database/mod.rs
 //! A time-series-esque database for storing and querying append-only streams of events.

+use std::collections::BTreeMap;
 use std::path::Path;

 /// A time-series-esque database for storing and querying append-only stream of events.
 pub struct Database;

+/// Labels used to identify a stream.
+///
+/// For now this is just a type alias, but our requirements may diverge from `BTreeMap` in future.
+pub type Labels = BTreeMap<String, String>;
+
+/// The type used for timestamps.
+///
+/// `u64` gives us ~585 million years at millisecond resolution. This is obviously more than we
+/// need, but `u32` only gives us 50 days which is obviously too few!
+///
+/// This is not public. The alias just exists to make changing the timestamp type easier.
+type Timestamp = u64;
+
+/// An event that can be stored by [`Database`].
+pub struct Event {
+    timestamp: Timestamp,
+    data: Vec<u8>,
+}
+
 /// Possible error situations when opening a database.
 pub enum OpenError {
     Io(std::io::Error),
@@ -36,7 +56,7 @@ impl Database {
     }

     /// Push a new `event` into the stream identified by `labels`.
-    pub fn push(&self, labels: _, event: _) -> Result<(), PushError> {
+    pub fn push(&self, labels: &Labels, event: Event) -> Result<(), PushError> {
         todo!()
     }
 }
```

Now we just need to think about `PushError`.
In our `std`-based example above, there would be no `PushError` since none of the insertion logic is fallible.
In contrast, our current `log_database` writes straight to the log file meaning that `io::Error`s are possible.
We don't yet know how our new implementation will behave (or even really how it should behave).
In general we'd like writes to go to memory first and be sync'd to disk periodically.
As such, we might get away with making this operation infallible:

```diff
--- a/src/database/mod.rs
+++ b/src/database/mod.rs
@@ -56,7 +56,7 @@ impl Database {
     }

     /// Push a new `event` into the stream identified by `labels`.
-    pub fn push(&self, labels: &Labels, event: Event) -> Result<(), PushError> {
+    pub fn push(&self, labels: &Labels, event: Event) {
         todo!()
     }
 }
```

OK...
This is going very slowly, and we haven't even implemented any functionality yet!

Let's not despair – next up: reading the database.
We previously used `query(&self, key: &str, value: &str) -> io::Result<Option<Vec<String>>>` which is quite a mouthful, and also requires us to clone all the values we read from the database even if the client doesn't need owned values.
This is another area where future cleverness could get us some performance wins, e.g. by returning an iterator of values from `mmap`'d database files.
We'll also want to support more interesting queries in future than single `key=value` ones.
Let's take a first step on that journey by creating a `database::Query` type:

```diff
--- a/src/database/mod.rs
+++ b/src/database/mod.rs
@@ -7,6 +7,12 @@ use std::path::Path;
 /// A time-series-esque database for storing and querying append-only stream of events.
 pub struct Database;

+/// A structure describing database queries.
+pub enum Query {
+    /// A query that will find events from streams with a particular label.
+    Label { name: String, value: String },
+}
+
 /// Labels used to identify a stream.
 ///
 /// For now this is just a type alias, but our requirements may diverge from `BTreeMap` in future.
```

Now let's write `Database::query` in terms of that:

```diff
--- a/src/database/mod.rs
+++ b/src/database/mod.rs
@@ -42,6 +42,9 @@ pub enum OpenError {
 /// Possible error situations when restoring a database.
 pub type RestoreError = std::io::Error;

+/// Possible error situations when querying a database.
+pub type QueryError = std::io::Error;
+
 impl Database {
     /// Open a database at the given `path`.
     ///
@@ -65,4 +68,13 @@ impl Database {
     pub fn push(&self, labels: &Labels, event: Event) {
         todo!()
     }
+
+    /// Find events in the database matching the given `query`.
+    ///
+    /// # Errors
+    ///
+    /// Any [`io::Error`]s encountered when running the query are returned.
+    pub fn query(&self, query: &Query) -> Result<Vec<Event>, QueryError> {
+        todo!()
+    }
 }
```

We've assumed that querying can return `io::Error`s since we may need to read from disk.

### You've got to iterate to accumulate

In the interests of justifying all the complexity we've seen, let's try some test- and benchmark-driven development.

We'll starts with a 'fresh database' test that will open a `Database` in a new directory, write a value, and read it back:

```diff
--- a/src/database/mod.rs
+++ b/src/database/mod.rs
@@ -78,3 +78,33 @@ impl Database {
         todo!()
     }
 }
+
+#[cfg(test)]
+mod tests {
+    use std::error::Error;
+
+    use super::{Database, Event, Query};
+
+    #[test]
+    fn fresh_database() -> Result<(), Box<dyn Error>> {
+        let tempdir = tempfile::tempdir()?;
+        let db = Database::open(tempdir.path().join("data"))?;
+
+        let labels = vec![("hello".to_string(), "world".to_string())]
+            .into_iter()
+            .collect();
+        let event = Event {
+            timestamp: 0,
+            data: "message".as_bytes().into(),
+        };
+        db.push(&labels, event.clone());
+
+        let query = Query::Label {
+            name: "hello".to_string(),
+            value: "world".to_string(),
+        };
+        assert_eq!(db.query(&query)?, vec![event]);
+
+        Ok(())
+    }
+}
```

First things first, we're missing a bunch of trait implementations:

```diff
--- a/src/database/mod.rs
+++ b/src/database/mod.rs
@@ -27,18 +27,28 @@ pub type Labels = BTreeMap<String, String>;
 type Timestamp = u64;

 /// An event that can be stored by [`Database`].
+#[derive(Clone, Debug, Eq, PartialEq)]
 pub struct Event {
     timestamp: Timestamp,
     data: Vec<u8>,
 }

 /// Possible error situations when opening a database.
+#[derive(Debug)]
 pub enum OpenError {
     Io(std::io::Error),
     NotDirectory,
     Restore(RestoreError),
 }

+impl std::fmt::Display for OpenError {
+    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
+        write!(f, "error opening database")
+    }
+}
+
+impl std::error::Error for OpenError {}
+
 /// Possible error situations when restoring a database.
 pub type RestoreError = std::io::Error;

```

Now we can run the test and get a panic (this will also run our old database tests, but that's fine):

```
$ cargo test database
...
---- database::tests::fresh_database stdout ----
thread 'database::tests::fresh_database' panicked at 'not yet implemented', src/database/mod.rs:74:9
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    database::tests::fresh_database
...
```

So now we need to implement `open`.
Let's do nothing for now and add functionality when we need it:

```diff
--- a/src/database/mod.rs
+++ b/src/database/mod.rs
@@ -71,7 +71,7 @@ impl Database {
     /// [`io::Error`]: std::io::Error
     /// [`NotDirectory`]: OpenError::NotDirectory
     pub fn open(path: impl AsRef<Path>) -> Result<Self, OpenError> {
-        todo!()
+        Ok(Database)
     }

     /// Push a new `event` into the stream identified by `labels`.
```

```
$ cargo test database
...
---- database::tests::fresh_database stdout ----
thread 'database::tests::fresh_database' panicked at 'not yet implemented', src/database/mod.rs:79:9
...
```

Now we're up to `push`.
Once again, we can cheat and do nothing here:

```diff
--- a/src/database/mod.rs
+++ b/src/database/mod.rs
@@ -75,9 +75,7 @@ impl Database {
     }

     /// Push a new `event` into the stream identified by `labels`.
-    pub fn push(&self, labels: &Labels, event: Event) {
-        todo!()
-    }
+    pub fn push(&self, labels: &Labels, event: Event) {}

     /// Find events in the database matching the given `query`.
     ///
```

```
$ cargo test database
...
---- database::tests::fresh_database stdout ----
thread 'database::tests::fresh_database' panicked at 'not yet implemented', src/database/mod.rs:86:9
...
```

And now the meat of the issue – `Database::query`.
Let's be really cheap for now and stick a `Vec<Event>` directly into `Database`:

```diff
--- a/src/database/mod.rs
+++ b/src/database/mod.rs
@@ -5,7 +5,9 @@ use std::collections::BTreeMap;
 use std::path::Path;

 /// A time-series-esque database for storing and querying append-only stream of events.
-pub struct Database;
+pub struct Database {
+    events: Vec<Event>,
+}

 /// A structure describing database queries.
 pub enum Query {
@@ -71,11 +73,13 @@ impl Database {
     /// [`io::Error`]: std::io::Error
     /// [`NotDirectory`]: OpenError::NotDirectory
     pub fn open(path: impl AsRef<Path>) -> Result<Self, OpenError> {
-        Ok(Database)
+        Ok(Database { events: Vec::new() })
     }

     /// Push a new `event` into the stream identified by `labels`.
-    pub fn push(&self, labels: &Labels, event: Event) {}
+    pub fn push(&self, labels: &Labels, event: Event) {
+        self.events.push(event);
+    }

     /// Find events in the database matching the given `query`.
     ///
@@ -83,7 +87,7 @@ impl Database {
     ///
     /// Any [`io::Error`]s encountered when running the query are returned.
     pub fn query(&self, query: &Query) -> Result<Vec<Event>, QueryError> {
-        todo!()
+        Ok(self.events.clone())
     }
 }

```

This is obviously logically flawed, but I'm tired and this kind of compiler/test whac-a-mole is a reasonable way of making progress without having to think too hard.
In this case it's the compiler that's unhappy, since `push` takes only a shared reference to `self` but `event.push` wants exclusivity.
Rather than change our API let's wrap the `events` in `RefCell`:

```diff
--- a/src/database/mod.rs
+++ b/src/database/mod.rs
@@ -1,12 +1,13 @@
 // src/database/mod.rs
 //! A time-series-esque database for storing and querying append-only streams of events.

+use std::cell::RefCell;
 use std::collections::BTreeMap;
 use std::path::Path;

 /// A time-series-esque database for storing and querying append-only stream of events.
 pub struct Database {
-    events: Vec<Event>,
+    events: RefCell<Vec<Event>>,
 }

 /// A structure describing database queries.
@@ -73,12 +74,14 @@ impl Database {
     /// [`io::Error`]: std::io::Error
     /// [`NotDirectory`]: OpenError::NotDirectory
     pub fn open(path: impl AsRef<Path>) -> Result<Self, OpenError> {
-        Ok(Database { events: Vec::new() })
+        Ok(Database {
+            events: RefCell::new(Vec::new()),
+        })
     }

     /// Push a new `event` into the stream identified by `labels`.
     pub fn push(&self, labels: &Labels, event: Event) {
-        self.events.push(event);
+        self.events.borrow_mut().push(event);
     }

     /// Find events in the database matching the given `query`.
@@ -87,7 +90,7 @@ impl Database {
     ///
     /// Any [`io::Error`]s encountered when running the query are returned.
     pub fn query(&self, query: &Query) -> Result<Vec<Event>, QueryError> {
-        Ok(self.events.clone())
+        Ok(self.events.borrow().clone())
     }
 }

```

```
$ cargo test
...
test database::tests::fresh_database ... ok
...
```

OK, now let's change our test to require more from our implementation, starting with some writes into alternative series:

```diff
--- a/src/database/mod.rs
+++ b/src/database/mod.rs
@@ -96,6 +96,7 @@ impl Database {

 #[cfg(test)]
 mod tests {
+    use std::collections::BTreeMap;
     use std::error::Error;

     use super::{Database, Event, Query};
@@ -105,21 +106,30 @@ mod tests {
         let tempdir = tempfile::tempdir()?;
         let db = Database::open(tempdir.path().join("data"))?;

-        let labels = vec![("hello".to_string(), "world".to_string())]
-            .into_iter()
-            .collect();
-        let event = Event {
-            timestamp: 0,
-            data: "message".as_bytes().into(),
-        };
-        db.push(&labels, event.clone());
+        db.push(&make_labels(&[("l1", "v1")]), make_event(0, "e1"));
+        db.push(&make_labels(&[("l1", "v2")]), make_event(1, "e2"));
+        db.push(&make_labels(&[("l2", "v1")]), make_event(2, "e3"));

         let query = Query::Label {
-            name: "hello".to_string(),
-            value: "world".to_string(),
+            name: "l1".to_string(),
+            value: "v2".to_string(),
         };
-        assert_eq!(db.query(&query)?, vec![event]);
+        assert_eq!(db.query(&query)?, vec![make_event(1, "e2")]);

         Ok(())
     }
+
+    fn make_labels(labels: &[(&str, &str)]) -> BTreeMap<String, String> {
+        labels
+            .iter()
+            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
+            .collect()
+    }
+
+    fn make_event(timestamp: u64, data: impl AsRef<[u8]>) -> Event {
+        Event {
+            timestamp,
+            data: data.as_ref().into(),
+        }
+    }
 }
```

We've created some helpers and `push`ed some more events.
Now our test is failing again:

```
$ cargo test database
...
---- database::tests::fresh_database stdout ----
thread 'database::tests::fresh_database' panicked at 'assertion failed: `(left == right)`
  left: `[Event { timestamp: 0, data: [101, 49] }, Event { timestamp: 1, data: [101, 50] }, Event { timestamp: 2, data: [101, 51] }]`,
 right: `[Event { timestamp: 1, data: [101, 50] }]`', src/database/mod.rs:117:9
...
```

So, we need to actually record the labels when we insert.
The most naive way we can do this is by storing tuples of `(Labels, Event)` in `Database.events`, so let's do that:

```diff
--- a/src/database/mod.rs
+++ b/src/database/mod.rs
@@ -7,7 +7,7 @@ use std::path::Path;

 /// A time-series-esque database for storing and querying append-only stream of events.
 pub struct Database {
-    events: RefCell<Vec<Event>>,
+    events: RefCell<Vec<(Labels, Event)>>,
 }

 /// A structure describing database queries.
@@ -81,7 +81,7 @@ impl Database {

     /// Push a new `event` into the stream identified by `labels`.
     pub fn push(&self, labels: &Labels, event: Event) {
-        self.events.borrow_mut().push(event);
+        self.events.borrow_mut().push((labels.clone(), event));
     }

     /// Find events in the database matching the given `query`.
@@ -90,7 +90,22 @@ impl Database {
     ///
     /// Any [`io::Error`]s encountered when running the query are returned.
     pub fn query(&self, query: &Query) -> Result<Vec<Event>, QueryError> {
-        Ok(self.events.borrow().clone())
+        let results = match query {
+            Query::Label { name, value } => self
+                .events
+                .borrow()
+                .iter()
+                .filter_map(|(labels, event)| {
+                    if labels.get(name) == Some(value) {
+                        Some(event.clone())
+                    } else {
+                        None
+                    }
+                })
+                .collect(),
+        };
+
+        Ok(results)
     }
 }

```

```
$ cargo test database
...
test database::tests::fresh_database ... ok
...
```

Now we'd like to test an restored database!
We'll use the same setup, but we'll `drop` the database and re-open it before asserting:

```diff
--- a/src/database/mod.rs
+++ b/src/database/mod.rs
@@ -134,6 +134,27 @@ mod tests {
         Ok(())
     }

+    #[test]
+    fn restored_database() -> Result<(), Box<dyn Error>> {
+        let tempdir = tempfile::tempdir()?;
+        let db = Database::open(tempdir.path().join("data"))?;
+
+        db.push(&make_labels(&[("l1", "v1")]), make_event(0, "e1"));
+        db.push(&make_labels(&[("l1", "v2")]), make_event(1, "e2"));
+        db.push(&make_labels(&[("l2", "v1")]), make_event(2, "e3"));
+        drop(db);
+
+        let db = Database::open(tempdir.path().join("data"))?;
+
+        let query = Query::Label {
+            name: "l1".to_string(),
+            value: "v2".to_string(),
+        };
+        assert_eq!(db.query(&query)?, vec![make_event(1, "e2")]);
+
+        Ok(())
+    }
+
     fn make_labels(labels: &[(&str, &str)]) -> BTreeMap<String, String> {
         labels
             .iter()
```

```
$ cargo test database
...
---- database::tests::restored_database stdout ----
thread 'database::tests::restored_database' panicked at 'assertion failed: `(left == right)`
  left: `[]`,
 right: `[Event { timestamp: 1, data: [101, 50] }]`', src/database/mod.rs:153:9
...
```

Of course, we're not actually restoring anything so there's nothing there.
What's the most naive way that we could make this persistent?
Why don't we simply write the whole thing to a file during `drop`?

```diff
--- a/src/database/mod.rs
+++ b/src/database/mod.rs
@@ -3,10 +3,13 @@

 use std::cell::RefCell;
 use std::collections::BTreeMap;
-use std::path::Path;
+use std::fs::File;
+use std::path::{Path, PathBuf};

 /// A time-series-esque database for storing and querying append-only stream of events.
+#[derive(serde::Serialize)]
 pub struct Database {
+    path: PathBuf,
     events: RefCell<Vec<(Labels, Event)>>,
 }

@@ -30,7 +33,7 @@ pub type Labels = BTreeMap<String, String>;
 type Timestamp = u64;

 /// An event that can be stored by [`Database`].
-#[derive(Clone, Debug, Eq, PartialEq)]
+#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
 pub struct Event {
     timestamp: Timestamp,
     data: Vec<u8>,
@@ -75,6 +78,7 @@ impl Database {
     /// [`NotDirectory`]: OpenError::NotDirectory
     pub fn open(path: impl AsRef<Path>) -> Result<Self, OpenError> {
         Ok(Database {
+            path: path.as_ref().to_path_buf(),
             events: RefCell::new(Vec::new()),
         })
     }
@@ -109,6 +113,13 @@ impl Database {
     }
 }

+impl Drop for Database {
+    fn drop(&mut self) {
+        let file = File::create(&self.path).expect("create file");
+        serde_json::to_writer(file, &self).expect("serialize database");
+    }
+}
+
 #[cfg(test)]
 mod tests {
     use std::collections::BTreeMap;
```

We've not used `serde` directly yet so we also need to add that to our `Cargo.toml`:

```
$ cargo add serde
  Adding serde v1.0.123 to dependencies
```

```diff
--- a/Cargo.toml
+++ b/Cargo.toml
@@ -28,6 +28,7 @@ kube = "0.48.0"
 kube-runtime = "0.48.0"
 k8s-openapi = { version = "0.11.0", default-features = false, features = ["v1_20"] }
 tokio = { version = "1.1.1", features = ["rt"] }
+serde = "1.0.123"

 [target.'cfg(target_os = "linux")'.dependencies]
 inotify = { version = "0.8.3", default-features = false }
```

Our tests still fail, of course, since we also need to read the file on `open`:

```diff
--- a/src/database/mod.rs
+++ b/src/database/mod.rs
@@ -4,10 +4,11 @@
 use std::cell::RefCell;
 use std::collections::BTreeMap;
 use std::fs::File;
+use std::io;
 use std::path::{Path, PathBuf};

 /// A time-series-esque database for storing and querying append-only stream of events.
-#[derive(serde::Serialize)]
+#[derive(serde::Deserialize, serde::Serialize)]
 pub struct Database {
     path: PathBuf,
     events: RefCell<Vec<(Labels, Event)>>,
@@ -33,7 +34,7 @@ pub type Labels = BTreeMap<String, String>;
 type Timestamp = u64;

 /// An event that can be stored by [`Database`].
-#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
+#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
 pub struct Event {
     timestamp: Timestamp,
     data: Vec<u8>,
@@ -56,7 +57,11 @@ impl std::fmt::Display for OpenError {
 impl std::error::Error for OpenError {}

 /// Possible error situations when restoring a database.
-pub type RestoreError = std::io::Error;
+#[derive(Debug)]
+pub enum RestoreError {
+    Io(std::io::Error),
+    Deserialize(serde_json::Error),
+}

 /// Possible error situations when querying a database.
 pub type QueryError = std::io::Error;
@@ -77,10 +82,22 @@ impl Database {
     /// [`io::Error`]: std::io::Error
     /// [`NotDirectory`]: OpenError::NotDirectory
     pub fn open(path: impl AsRef<Path>) -> Result<Self, OpenError> {
-        Ok(Database {
-            path: path.as_ref().to_path_buf(),
-            events: RefCell::new(Vec::new()),
-        })
+        match File::open(path.as_ref()) {
+            Ok(mut file) => {
+                let mut contents = Vec::new();
+                io::Read::read_to_end(&mut file, &mut contents)
+                    .map_err(RestoreError::Io)
+                    .map_err(OpenError::Restore)?;
+                serde_json::from_slice(&contents)
+                    .map_err(RestoreError::Deserialize)
+                    .map_err(OpenError::Restore)
+            }
+            Err(error) if matches!(error.kind(), io::ErrorKind::NotFound) => Ok(Database {
+                path: path.as_ref().to_path_buf(),
+                events: RefCell::new(Vec::new()),
+            }),
+            Err(error) => Err(OpenError::Io(error)),
+        }
     }

     /// Push a new `event` into the stream identified by `labels`.
```

It ain't beautiful, but once again our tests pass:

```
$ cargo test database
...
test database::tests::fresh_database ... ok
test database::tests::restored_database ... ok
...
```

Before making any improvements we should test our errors as well:

```diff
--- a/src/database/mod.rs
+++ b/src/database/mod.rs
@@ -3,8 +3,7 @@

 use std::cell::RefCell;
 use std::collections::BTreeMap;
-use std::fs::File;
-use std::io;
+use std::fs::{self, File};
 use std::path::{Path, PathBuf};

 /// A time-series-esque database for storing and querying append-only stream of events.
@@ -43,8 +42,6 @@ pub struct Event {
 /// Possible error situations when opening a database.
 #[derive(Debug)]
 pub enum OpenError {
-    Io(std::io::Error),
-    NotDirectory,
     Restore(RestoreError),
 }

@@ -82,21 +79,19 @@ impl Database {
     /// [`io::Error`]: std::io::Error
     /// [`NotDirectory`]: OpenError::NotDirectory
     pub fn open(path: impl AsRef<Path>) -> Result<Self, OpenError> {
-        match File::open(path.as_ref()) {
-            Ok(mut file) => {
-                let mut contents = Vec::new();
-                io::Read::read_to_end(&mut file, &mut contents)
-                    .map_err(RestoreError::Io)
-                    .map_err(OpenError::Restore)?;
-                serde_json::from_slice(&contents)
-                    .map_err(RestoreError::Deserialize)
-                    .map_err(OpenError::Restore)
-            }
-            Err(error) if matches!(error.kind(), io::ErrorKind::NotFound) => Ok(Database {
-                path: path.as_ref().to_path_buf(),
+        let path = path.as_ref();
+        if path.exists() {
+            let contents = fs::read(&path)
+                .map_err(RestoreError::Io)
+                .map_err(OpenError::Restore)?;
+            serde_json::from_slice(&contents)
+                .map_err(RestoreError::Deserialize)
+                .map_err(OpenError::Restore)
+        } else {
+            Ok(Database {
+                path: path.to_path_buf(),
                 events: RefCell::new(Vec::new()),
-            }),
-            Err(error) => Err(OpenError::Io(error)),
+            })
         }
     }

@@ -141,8 +136,10 @@ impl Drop for Database {
 mod tests {
     use std::collections::BTreeMap;
     use std::error::Error;
+    use std::fs::{self, File};
+    use std::os::unix::fs::PermissionsExt;

-    use super::{Database, Event, Query};
+    use super::{Database, Event, OpenError, Query, RestoreError};

     #[test]
     fn fresh_database() -> Result<(), Box<dyn Error>> {
@@ -183,6 +180,39 @@ mod tests {
         Ok(())
     }

+    #[test]
+    fn restore_io_error() -> Result<(), Box<dyn Error>> {
+        let tempdir = tempfile::tempdir()?;
+        let path = tempdir.path().join("data");
+
+        // Make `Database::open` return an `io::Error` by making `data.json` unreadable.
+        File::create(&path)?.set_permissions(fs::Permissions::from_mode(0o200))?;
+
+        let error = Database::open(&path).err().unwrap();
+        assert!(matches!(error, OpenError::Restore(RestoreError::Io(_))));
+        assert_eq!(&format!("{}", error), "error opening database");
+
+        Ok(())
+    }
+
+    #[test]
+    fn restore_deserialize_error() -> Result<(), Box<dyn Error>> {
+        let tempdir = tempfile::tempdir()?;
+        let path = tempdir.path().join("data");
+
+        // Cause a deserialize error by writing invalid JSON.
+        fs::write(&path, "oh dear")?;
+
+        let error = Database::open(&path).err().unwrap();
+        assert!(matches!(
+            error,
+            OpenError::Restore(RestoreError::Deserialize(_))
+        ));
+        assert_eq!(&format!("{}", error), "error opening database");
+
+        Ok(())
+    }
+
     fn make_labels(labels: &[(&str, &str)]) -> BTreeMap<String, String> {
         labels
             .iter()
```

We've ended up rewriting `Database::open` to correctly return `RestoreError`s when the error occurs whilst trying to restore.
This also happens to be the only error that can happen currently, so the others have been removed.

We've also ended up with some clippy warnings that we should clean up:

```diff
--- a/src/database/mod.rs
+++ b/src/database/mod.rs
@@ -16,7 +16,13 @@ pub struct Database {
 /// A structure describing database queries.
 pub enum Query {
     /// A query that will find events from streams with a particular label.
-    Label { name: String, value: String },
+    Label {
+        /// The label name to match.
+        name: String,
+
+        /// The label value to match.
+        value: String,
+    },
 }

 /// Labels used to identify a stream.
@@ -42,6 +48,7 @@ pub struct Event {
 /// Possible error situations when opening a database.
 #[derive(Debug)]
 pub enum OpenError {
+    /// An error occurred when trying to restore from an existing database.
     Restore(RestoreError),
 }

@@ -56,7 +63,14 @@ impl std::error::Error for OpenError {}
 /// Possible error situations when restoring a database.
 #[derive(Debug)]
 pub enum RestoreError {
+    /// An I/O error occurred when restoring (e.g. permission denied).
+    ///
+    /// This may be fixable by ensuring correct permissions etc.
     Io(std::io::Error),
+
+    /// An error occurred when deserializing the database file.
+    ///
+    /// If this happens the database is corrupt and would need to be manually repaired or deleted.
     Deserialize(serde_json::Error),
 }

```

Finally we should use our `test::Result` alias to save some bytes:

```diff
--- a/src/database/mod.rs
+++ b/src/database/mod.rs
@@ -149,14 +149,15 @@ impl Drop for Database {
 #[cfg(test)]
 mod tests {
     use std::collections::BTreeMap;
-    use std::error::Error;
     use std::fs::{self, File};
     use std::os::unix::fs::PermissionsExt;

+    use crate::test;
+
     use super::{Database, Event, OpenError, Query, RestoreError};

     #[test]
-    fn fresh_database() -> Result<(), Box<dyn Error>> {
+    fn fresh_database() -> test::Result {
         let tempdir = tempfile::tempdir()?;
         let db = Database::open(tempdir.path().join("data"))?;

@@ -174,7 +175,7 @@ mod tests {
     }

     #[test]
-    fn restored_database() -> Result<(), Box<dyn Error>> {
+    fn restored_database() -> test::Result {
         let tempdir = tempfile::tempdir()?;
         let db = Database::open(tempdir.path().join("data"))?;

@@ -195,7 +196,7 @@ mod tests {
     }

     #[test]
-    fn restore_io_error() -> Result<(), Box<dyn Error>> {
+    fn restore_io_error() -> test::Result {
         let tempdir = tempfile::tempdir()?;
         let path = tempdir.path().join("data");

@@ -210,7 +211,7 @@ mod tests {
     }

     #[test]
-    fn restore_deserialize_error() -> Result<(), Box<dyn Error>> {
+    fn restore_deserialize_error() -> test::Result {
         let tempdir = tempfile::tempdir()?;
         let path = tempdir.path().join("data");

```

### Motivating improvements

Our new `database` is probably significantly worse than our old one.
In particular, we'd expect query performance, memory usage, and reliability to suffer substantially.
But how can we know?
As an exercise for ourselves, let's try to avoid over-engineering by creating representative scenarios and testing or benchmarking them to motivate improvments to our database – if they're even necessary!

We'll do this by:

1. Defining some representative scenarios.
   We'd like these to cover both 'typical' usage and more extreme scenarios.
1. Creating a binary to execute each scenario using our database.
1. Creating additional binaries (and/or use flags) to execute each scenario using some other databases.
1. Iteratively testing, benchmarking, and optimising our database against others.

Do we really want to go down this rabbithole?
E.g. if we can implement representative scenarios using sled or RocksDB, why not just use them?
Do we expect to beat the performance and reliability of crazy-fast production-grade systems?

No, the point is to start getting a bit more serious about measurement so that we can be deliberate about where we simplify and where we don't.
We want this system to be "minimal" but also "viable" for real world cluster monitoring.
At the moment we don't really know what that means, so we're staring down the barrel of decades of cutting edge database design, agonising over API design, and otherwise yak-shaving ourselves into oblivion without some measure of what it means for our system to be "viable".
In the proper write-up, we'll do this earlier – although it was valuable to push through the super basic log collector to validate the project was even tractable.

## Wrapping up

Anyway, this is getting very meta.
We have the shape of a common database, but no idea how it will perform or whether the functionality is sufficient.
In the next post we will attempt to define some representative scenarios that we'd like our system to cover.
We'll then create some integration test binaries to implement these scenarios using our database, which will let us flesh out missing functionality.
Then we'll create some integration test binaries to implement the same scenarios using *other* databases, which will let us compare the performance of our database with others.

In the end, we hope this will drive us to a 'minimum viable database' for our minimum viable monitoring system!
Or, perhaps we'll realise that sled will work just fine and decide to just use that!
