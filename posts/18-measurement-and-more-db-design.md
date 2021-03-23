# Measurement and more DB design

So far, we've taken a pretty feature-driven and tactical approach to building our monitoring system.
The fruits of our labor are a functional and fairly efficient, albeit incomplete, single-node log collector.
We shouldn't under-sell this too hard – we've gone from nothing at all to a full-stack log collector, including scripted deployment et-al.
Well job 🎉

Now we want to introduce metric collection.
We've decided we want to store logs and metrics in the same database, and want to focus on that as our starting point for introducing metric support to the system.
However, this has led us down a rabbithole of endless design considerations and possible techniques.

This didn't stop us doing *something* – we created a `database` module and imbued it with a naive implementation of a persistent `Vec<(Labels, Timestamp, Vec<u8>)>`.
We can make educated guesses about the limitations of our implementation, and make further guesses at what might remove them, but doing so may lead us back down the rabbithole of good database design and prevent us from reaching good *enough*.

We've decided to take a step back and try a more methodical approach.
Starting by describing the scenarios in which our database should perform well, we can ensure feature-completeness.
We can then benchmark our database against alternatives – including our existing `log_database` and external embedded databases such as RocksDB or sled – and decide what it would mean to "perform well".
We can then make reasoned trade-offs about complexity vs. performance, and hopefully remain on the "minimum viable" knife-edge.

## Scenarios

We've talked a bit about use-cases before, way back in [Discovery](00-discovery.md).
We talked about log retrieval use-cases:

> 1. Following the live logs for a specific service or component in order to debug it or otherwise observe its behaviour.
>   For this use-case, latency and specificity are key – obtaining the latest log entries based on precise criteria needs to be fast.
>   Latency is not just a retrieval issue, but the retrieval performance would affect the overall latency.
>   Reliably filtering logs to those from a specific source would require field-based filtering, rather than text-based filtering which may lead to false positives.
>
> 1. Retrieving logs with potentially vague criteria in order to understand an alert with limited context – perhaps only the service name and a rough timeframe, or even just a string of text from an error message.
>   For this use-case, query flexibility and plain-text search are important.
>   The known criteria should be combineable into a single query which can be further refined to exclude noise and hone in on relevant entries.
>
> 1. Tracing activity through multiple services based on structured log data (e.g. a transaction ID or user ID).
>   For this use-case, it's important that there are minimal or no required constraints when searching logs.
>   Relevant logs may be from arbitrary time periods and sources, and all must be retrieved in order to offer a complete answer to the query.

And also metrics retrieval use-cases:

> 1. Generate a graph of a metrics over time to visualise whether recent changes have had the desired effects.
>   For this use-case, performant retrieval and aggregation of past values of specific metrics is important.
>   It may also be necessary to filter metrics to particular dimensions to get the graph you want, so this needs to be be performant as well.
>
> 1. Constantly monitor critical system metrics in order to quickly detect unplanned service disruption.
>   For this use-case, being able to repeatedly sample recent metrics without impacting concurrent metric ingestion or other monitoring queries is vitaly important.
>   This might need to scale to hundreds or more constantly evaluated queries/thresholds.
>
> 1. Interactively query and visualise multiple metrics across multiple dimensions in order to debug a performance bug.
>   For this use-case, the dimensionality of metrics is important, along with the discoverability of metrics and their dimensions.
>   It needs to be possible to filter metrics by complex queries on their dimensions in order to narrow in on meaningful metrics about only the affected components.

Of course, this is from when we were young and naïve, so the use-cases are quite broad in the requirements they impose on the system.
In particular, event-specific searching out of scope (free text search, tracing), as is alerting.
We want to scope these down and split them up into some representative usage scenarios that we could implement against a database candidate.

### Defining write scenarios

#### Log collection

Let's start with what we know – passive log collection from a fairly quiet single-node cluster.
This is the Real World™ scenario the system is being tested in: a single-node cluster with \~20 pods and no real churn.
It would probably be a complete ghost town if it wasn't for some components being quite noisy, particularly `nginx-ingress-controller` and `coredns`.

In this scenario we have a steady trickle of around 20 entries per minute, overwhelmingly to a few 'hot' streams.
There are very few reads.

We can already see a few variables here when it comes to the write-side of log collection:

- The overall rate of events.
- The number of active streams.
- How the overall rate is spread across streams.

We could define this scenario in terms of these variables:

> A quiet node has \~20 events per minute from \~20 active streams.
> 70% of the events are from 15% of the streams, 25% are from the next 10%, 4% are from the next 10%, and the remaining 1% of events are spread uneavenly across the remaining 65% of streams (with some having none at all).

It's easy to see how we could scale this up in a couple of different ways:

- We can raise the overall event rate to simulate more verbose workloads.
- We can raise the number of active streams to represent more workloads.
- We can even the spread of event rate to simulate more consistent workloads.

Of course changing these variables doesn't change the *features* that are required, so this is more relevant to benchmarking.
When benchmarking we might prefer to vary just the number of active streams and the spread of event rate, and measure the maximum event rate.

We could also come at this the other way and imagine a synthetic event source that can be set to emit events at a particular rate.
We could then operate multiple synthetic event sources, with different event rates, to cover all these variables.
This sounds like something we could actually implement, so for now let's consider log writing in these terms.

#### Metrics collection

We haven't implement metrics collection yet, but we expect it to be quite different.
Specifically we expect to scrape metrics from services in the cluster for compatibility with Prometheus metrics sources.

From the database's perspective this could still be modelled as synthetic event sources, except in this scenario the events would be emitted in bursts rather than at a constant rate.
We could add an extra parameter to our synthetic sources indicating the period at which events should emit.
When operating multiple sources, we could simulate sources all synchronised to the same period or sources with varying periods representing unpredictable bursts.

#### Churn

Another scenario we need to consider that's relevant to both log and metric collection is what Prometheus calls series churn.
Churn refers to the emergence of new streams and the stopping of current streams over time.
It's especially relevant to Kubernetes, where labels contain identifiers and deployments, scaling events, restarts etc. cause identifiers to change.
This is illustrated really well in [Writing a Time Series Database from Scratch](https://fabxc.org/tsdb/) with the following figure:

> ```
> series
>   ^
>   │   . . . . . .
>   │   . . . . . .
>   │   . . . . . .
>   │               . . . . . . .
>   │               . . . . . . .
>   │               . . . . . . .
>   │                             . . . . . .
>   │                             . . . . . .
>   │                                         . . . . .
>   │                                         . . . . .
>   │                                         . . . . .
>   v
>     <-------------------- time --------------------->
> ```

Note that churn doesn't necessarily increase the number of active streams (from a write-only perspective), it just changes which streams are active.
This framing suggests that we could simulate churn by maintaining a constant number of synthetic sources, but giving each source a duration after which it will stop writing and a new source will be added (potentially with a different event rate, period, and/or duration).

By adjusting the factors controlling how synthetic sources are configured we should be able to simulate quite diverse workloads, and this seems like it could be reasonable to implement!

## Synthetic event source

We want to create a type that periodically 'emits events'.
We'd like to be orchestrate many of these concurrently.
This sounds like an ideal use case for an async `Stream`.
~Let's do a little experiment in our `test` module:~
Here's one we made off-camera:

```
$ cargo new --lib loadgen
$ cd loadgen
$ echo '.gitignore' > .gitignore
$ cargo add smol
$ cargo check
...
    Finished dev [unoptimized + debuginfo] target(s) in 2.80s
```

```diff
--- a/loadgen/Cargo.toml
+++ b/loadgen/Cargo.toml
@@ -7,3 +7,4 @@ edition = "2018"
 # See more keys and their definitions at https://doc.rust-lang.org/cargo/reference/manifest.html

 [dependencies]
+smol = "1.2.5"
```

```rust
// loadgen/src/lib.rs
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use smol::future::Future;
use smol::stream::{Stream, StreamExt};
use smol::Timer;

pub struct Generator {
    streams: Vec<Pin<Box<dyn Stream<Item = ()>>>>,
}

impl Generator {
    pub fn new<E: Fn() + 'static>(
        duration: Duration,
        stream_count: u32,
        event_count: u32,
        distribution: Distribution,
        event: E,
    ) -> Self {
        let mut streams = Vec::new();
        let event: Arc<dyn Fn() + 'static> = Arc::new(event);

        for event_count in distribution.distribute(event_count, stream_count) {
            let events_per_second = f64::from(event_count) / duration.as_secs_f64();
            let interval = Duration::from_secs_f64(1.0 / events_per_second);
            let event = Arc::clone(&event);

            let emitter = Timer::interval(interval)
                .map(move |_| event())
                .take(event_count as usize);

            let stream: Pin<Box<dyn Stream<Item = _>>> = Box::pin(emitter);
            streams.push(stream);
        }

        Generator { streams }
    }

    pub async fn run(self) {
        GeneratorRun {
            streams: self.streams,
        }
        .await
    }
}

struct GeneratorRun {
    streams: Vec<Pin<Box<dyn Stream<Item = ()>>>>,
}

impl Future for GeneratorRun {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.streams
            .iter_mut()
            .fold(Poll::Ready(()), |poll, stream| {
                if let (Poll::Pending, _) | (_, Poll::Pending) | (_, Poll::Ready(Some(_))) =
                    (poll, stream.poll_next(cx))
                {
                    Poll::Pending
                } else {
                    poll
                }
            })
    }
}

#[derive(Clone, Copy)]
pub enum Distribution {
    Uniform,
    Linear,
}

impl Distribution {
    fn distribute(self, event_count: u32, stream_count: u32) -> Vec<u32> {
        let event_count_f64 = f64::from(event_count);
        let stream_count_f64 = f64::from(stream_count);

        match self {
            Self::Uniform => {
                // Same number of events to each stream. Casts is OK because u32 / u32 is u32,
                // we just want the division rounded rather than floored.
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let per_stream = (event_count_f64 / stream_count_f64).round() as u32;

                vec![per_stream; stream_count as usize]
            }
            Self::Linear => {
                // Calculate the height of a triangle with `base = stream_count + 1` and `area =
                // event_count`.
                let max = 2.0 * event_count_f64 / (1.0 + stream_count_f64);

                // We want progress evenly up to `max`.
                let inc = max / stream_count_f64;

                (1..=stream_count)
                    .map(|i| {
                        let stream_events = (f64::from(i) * inc).round();

                        // Cast is OK because `stream_events` cannot be negative, greater than
                        // `u32::MAX`, and has been rounded.
                        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                        let stream_events = stream_events as u32;

                        stream_events
                    })
                    .filter(|i| *i != 0)
                    .collect()
            }
        }
    }
}
```

We've created a new crate inside our existing crate, in which we've implemented a simple load generator.
A `loadgen::Generator` is constructed with a `duration`, `stream_count`, `event_count`, `distribution`, and `event` callback.
At construction, `stream_count` streams are constructed that will together call `event` `event_count` times.
The streams call `event` periodically for `duration`, and the rate for each stream is determined based on `loadgen::Distribution`.
The two distributions available are:

- `Uniform`: each stream emits `event_count / stream_count` events.
- `Linear`: each stream emits a constant amount more events than the stream before.

In future we might add `Geometric`, `Exponential` etc.

Since we want to profile our database only, we would ideally minimise the amount of non-database work that's happening, including things like the `cargo test` harness.
As such, it makes most sense for our profile scenarios to run as separate binaries.
We can create these binaries in the `loadgen` crate to keep our root crate tidy:

```diff
--- a/loadgen/Cargo.toml
+++ b/loadgen/Cargo.toml
@@ -7,4 +7,7 @@ edition = "2018"
 # See more keys and their definitions at https://doc.rust-lang.org/cargo/reference/manifest.html

 [dependencies]
+monitoring-rs = { path = ".." }
 smol = "1.2.5"
+structopt = "0.3.21"
+tempfile = "3.2.0"
```

```rust
// loadgen/src/main.rs
use std::collections::BTreeMap;
use std::error::Error;
use std::rc::Rc;
use std::time::Duration;

use structopt::StructOpt;

use loadgen::{Distribution, Generator};
use monitoring_rs::database::{Database, Event, Query};

#[derive(StructOpt)]
struct Args {
    #[structopt(long)]
    avg_events_per_second: u32,

    #[structopt(long, parse(try_from_str = Self::parse_distribution))]
    distribution: Distribution,

    #[structopt(long)]
    seconds: u64,

    #[structopt(long)]
    streams: u32,
}

impl Args {
    fn parse_distribution(input: &str) -> Result<Distribution, String> {
        match input {
            "uniform" => Ok(Distribution::Uniform),
            "linear" => Ok(Distribution::Linear),
            _ => Err(format!("unrecognised distribution: {}", input)),
        }
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::from_args();

    let tempdir = tempfile::tempdir()?;
    let db = Rc::new(Database::open(tempdir.path().join("data"))?);

    let total_events = args.avg_events_per_second * args.streams;

    let gen = Generator::new(
        Duration::from_secs(args.seconds),
        args.streams,
        total_events,
        args.distribution,
        {
            let db = Rc::clone(&db);
            move || db.push(&make_labels(&[("hello", "world")]), make_event(0, "wow"))
        },
    );

    smol::block_on(gen.run());

    let query = Query::Label {
        name: "hello".to_string(),
        value: "world".to_string(),
    };
    assert_eq!(db.query(&query)?.len(), total_events as usize);

    Ok(())
}

fn make_labels(labels: &[(&str, &str)]) -> BTreeMap<String, String> {
    labels
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect()
}

fn make_event(timestamp: u64, data: impl AsRef<[u8]>) -> Event {
    Event::new(timestamp, data.as_ref().into())
}
```

We also need to expose a public way of constructing `Event`s:

```diff
--- a/src/database/mod.rs
+++ b/src/database/mod.rs
@@ -45,6 +45,14 @@ pub struct Event {
     data: Vec<u8>,
 }

+impl Event {
+    /// Construct a new [`Event`] with a `timestamp` and some `data`.
+    #[must_use]
+    pub fn new(timestamp: Timestamp, data: Vec<u8>) -> Self {
+        Event { timestamp, data }
+    }
+}
+
 /// Possible error situations when opening a database.
 #[derive(Debug)]
 pub enum OpenError {
```

Now we can build the `loadgen` binary (in release mode) and do some profiling:

```
$ cargo build --release
...
    Finished release [optimized] target(s) in 4.06s

$ export TIMEFMT="%J   %U  user %S system %P cpu %*E total
avg shared (code):         %X KB
avg unshared (data/stack): %D KB
total (sum):               %K KB
max memory:                %M KB
page faults from disk:     %F
other page faults:         %R"

$ zsh -c 'time target/release/loadgen \
  --avg-events-per-second 100 \
  --distribution linear \
  --seconds 30 \
  --streams 5'
target/release/loadgen --avg-events-per-second 100 --distribution linear  30    0.05s  user 0.13s system 0% cpu 30.492 total
avg shared (code):         0 KB
avg unshared (data/stack): 0 KB
total (sum):               0 KB
max memory:                2896 KB
page faults from disk:     56
other page faults:         878
```

Noice.

## Back down the database rabbithole

### LSM-trees

Key aspect is the separate in-memory and persistent structures.
Write-optimised: initially, writes just go to memory (and maybe WAL if high reliability).
Memory structure ("memtable") would be best as an ordered map (e.g. B-Tree).
Disk structure is typically "sorted string tables": contiguous, ordered sequences of key-value pairs.
Although it's write-optimised, memtable can serve queries for recently updated keys.

Memtable is flushed to disk periodically (as an SS-table).
SS-tables are compacted once a particular threshold is reached.
Compacting involves merging some SS-tables together into a new SS-table.
During this process new values for keys replace old values and removed values are removed, so the new SS-table is more space-efficient than the old ones (it's not clear to me how/when old tables are dropped).

LSM-trees are often arranged hierarchically.
Deeper levels have larger tables and older data.

> The main advantage of LSM-trees over other indexing structures (such as B-trees) is that they maintain sequential access patterns for writes [Athanassoulis et al. 2015].
> Small updates on B-trees may involve many random writes and are hence not efficient on either solid-state storage devices or hard-disk drives.
>
> To deliver high write performance, LSM-trees batch key-value pairs and write them sequentially.
> Subsequently, to enable efficient lookups (for both individual keys and range queries), LSM-trees continuously read, sort, and write key-value pairs in the background, thus maintaining keys and values in sorted order

Traditional LSM-trees are optimised for spinning disks.

> In HDDs, random I/Os are over 100× slower than sequential ones [Arpaci-Dusseau and Arpaci-Dusseau 2014; ONeil et al. 1996]; thus, performing additional sequential reads and writes to continually sort keys and enable efficient lookups represents an excellent tradeoff.

> An LSM-tree consists of a number of components of exponentially increasing sizes, C0 to Ck, as shown in Figure 1.
> The C0 component is a memory-resident update-in-place sorted tree, while the other components C1 to Ck are disk-resident append-only B-trees.
> During an insert in an LSM-tree, the inserted key-value pair is appended to an ondisk sequential log file, so as to enable recovery in case of a crash.
> Then, the key-value pair is added to the in-memory C0, which is sorted by keys; C0 allows efficient lookups and scans on recently inserted key-value pairs.
> Once C0 reaches its size limit, it will be merged with the on-disk C1 in an approach similar to merge sort; this process is known as compaction.
> The newly merged tree will be written to disk sequentially, replacing the old version of C1.
> Compaction (i.e., merge sorting) also happens for on-disk components, when each Ci reaches its size limit.
> Note that compactions are only performed between adjacent levels (Ci and Ci+1), and they can be executed asynchronously in the background.

> Compared with B-trees, LSM-trees may need multiple reads for a point lookup.
> Hence, LSM-trees are most useful when inserts are more common than lookups [Athanassoulis et al. 2015; ONeil et al. 1996].

Quotes from: https://dl.acm.org/doi/pdf/10.1145/3033273

### Prometheus TSDB

LSM-ish – separate in-memory and persistent structures, persistent structures are compacted over time.
Can exploit characteristics of the workload: append-only, time-ordered.

They've done something to reduce the impact on SSDs.
Perhaps there's only one level of compaction?

### WiscKey

To be investigated, but could guess it's related to storing pointers to data in LSM-tree, rather the data itself, to keep LSM-tree small.

> The central idea behind WiscKey is the separation of keys and values [Nyberg et al. 1994]; only keys are kept sorted in the LSM-tree, while values are stored separately in a log.

This makes a lot of sense for log data (large values), less sense for metrics.

> First, range query (scan) performance may be affected because values are not stored in sorted order anymore.
> WiscKey solves this challenge by using the abundant internal parallelism of SSD devices.
> Second, WiscKey needs garbage collection to reclaim the free space used by invalid values.

An append-only workload would depend less on garbage collection.
We would probably want to organise data such that garbage collection can be retention based.

> Third, separating keys and values makes crash consistency challenging; WiscKey leverages an interesting property in modern file systems, whose appends never result in garbage data on a crash, to realize crash consistency correctly and efficiently.

Need to know if this would hold on a mounted volume in Kubernetes.
Looks like it probably would, but might need more expensive disks.

> First, WiscKey separates keys from values, keeping only keys in the LSM-tree and
the values in a separate log file.
> Second, to deal with unsorted values (which necessitate random access during range queries), WiscKey uses the parallel random-read characteristic of SSD devices.
> Third, WiscKey utilizes unique crash consistency and garbage collection techniques to efficiently manage the value log.
> Finally, WiscKey optimizes performance by removing the LSM-tree log without sacrificing consistency, thus reducing system-call overhead from small writes.

> Keys are stored in an LSM-tree while values are stored in a separate value-log file, the vLog.
> The artificial value stored along with the key in the LSM-tree is the address of the actual value in the vLog.

Pretty much an LSM-tree where the value is just a fixed-size address for the actual value for the key.
Actual values are stored in a separate log structure (tbi).

> However, since the LSM-tree of WiscKey is much smaller than LevelDB (for the same database size), a lookup will likely search fewer levels of table files in the LSM-tree; furthermore, a significant portion of the LSM-tree can be easily cached in memory.
> Hence, each lookup only requires a single random read (for retrieving the value) and thus achieves better lookup performance than LevelDB.
> For example, assuming 16B keys and 1KB values, if the size of the entire key-value dataset is 100GB, then the size of the LSM-tree is roughly 2GB (assuming a 12B cost for a value’s location and size), which readily fits into main memory on modern systems.

2GB memory is a lot for our use-case, but 100GB is also a very large data set.
If it's proportional, a retention window of say \~10GB might only need 200MB of memory to cache the whole tree!

> Once a contiguous sequence of key-value pairs is requested, WiscKey starts reading a number of following keys from the LSM-tree sequentially.
> The corresponding value addresses retrieved from the LSM-tree are inserted into a queue; multiple threads fetch these values from the vLog concurrently.

> Garbage collection can be triggered rarely for workloads with few deletes and for environments with overprovisioned storage space.

So, probably we would not have garbage collection and instead rely on retention to free data.

> To reduce overhead, WiscKey buffers values in a user-space buffer and flushes the buffer only when the buffer size exceeds a threshold or when the user requests a synchronous insertion.
> Thus, WiscKey only issues large writes and reduces the number of write() system calls.
> For a lookup, WiscKey first searches the vLog buffer, and if not found there, actually reads from the vLog.

The paper proposes sharding as a way to scale the database and potentially improve performance:

> One solution is to partition the database and related memory structures into multiple smaller shards.
> Each shard’s keys will not overlap with others.
> Under this design, writes to different key-value ranges can be done concurrently to different shards.

We could reasonably shard the database across time (based on some heuristic) to avoid GC and make eviction efficient.

### LevelDB

> Experimental measurements show that generating an sstable from a 1MB log file takes \~12ms, which seems like an acceptable latency hiccup to add infrequently to a log write.

E.g., in-memory structures should be sized based on how quickly they can be persisted, and what is a reasonable delay to add to a random write that triggers the persistence.

### InfluxDB

> The new engine has similarities with LSM Trees (like LevelDB and Cassandra’s underlying storage).
> It has a write ahead log, index files that are read only, and it occasionally performs compactions to combine index files.
> We’re calling it a Time Structured Merge Tree because the index files keep contiguous blocks of time and the compactions merge those blocks into larger blocks of time.
>
> Compression of the data improves as the index files are compacted.
> Once a shard becomes cold for writes it will be compacted into as few files as possible, which yield the best compression.

#### InfluxDB IOx

This is a new backend for InfluxDB.
It's a more general purpose column-oriented database.
The architecture seems quite simple, and data is ultimately persisted in object storage, with local disk operating as a mere cache.
Performance concerns aside, object storage APIs might allow for much easier atomicity!

### Apache Arrow and/or Parquet

Apache Arrow seems too featureful for our use-case, but it may turn out not to be later (e.g. when we look at query languages/in-value fields/metric expressions etc.).
Parquet needs further investigation.

### LMDB

Looks fast and "simple" – perhaps as simple as a database *can* be, which is discouraging.

### Sanakirja

Originally wrapped LMDB but developed into its own thing – now has benchmarks showing it outperforming LMDB.
Built for an SCM, not clear how it will behave over long time periods.

### Is a tree the right structure?

We need to bear in mind that we kind of have two different structures going on:

1. The actual data, which is better thought of as 'time series' data than key-value data.
2. The indexing structures that allow us to find series by their labels.

This suggests that a key-value store might not be the best fit for all of our data.
In particular, we do not need to be able to lookup every datum by some key – we want find whole series by key and find datums within time ranges.

Something that many of these DBs have in common is that below the key-value level they are managing pages of arbitrary data.
Higher level APIs then read and write key-value data to those pages.
An ideal system built upon some page management machinery might well have two kinds of pages:

1. Time-series pages with compressed, contiguous series data.
2. Index pages that point to time-series pages based on their labels and time-ranges.

Imagine we had typical 4kb (4096 byte) pages, how many uncompressed samples could we fit on them?
If our samples are `(u64, f64)` (e.g. timestamp, measurement) each datum would be 16 bytes meaning only 256 datums per page.
Although, if a single series was being scraped every 5 seconds, that would be about 20 minutes of data, so maybe not so bad.
If we imagine querying 2 hours worth of metrics for such a series, we would need to load \~5.5 pages.
That also doesn't seem too bad.

We could perhaps imagine that, when allocating pages for a series, we always allocate `N` contiguous pages (for `N > 1`).
This might benefit performance if it turns out that we're reading a lot of random pages.

#### `mmap` vs `read`

There's a lot of hearsay about where `mmap` or `read` is more performant, and under what conditions.
The upshot seems to be "profile for your workload", but there's a suggestion that `mmap` can win out if access is random and files are open for a long time.

It's not really obvious whether access would look random from the OS' perspective when it comes to our hypothetical database.
In particular, searching by a common label (e.g. `namespace=kube-system`) would need to read multiple series, which would be spread around in different pages (even with additional contiguous allocation for series).

#### Imagine...

- Two types of files:
  - Data files.
  - Index files.
- Data files:
  - Managed in application as 4KiB pages.
  - Each page contains data from a single series.
  - Pages are allocated by application in contiguous blocks for a series (trade-off: over-allocation for faster iteration – could maybe even tune dynamically for 'hot' series).
  - One data file active for writing at a time
    - Could opt for small number > 1 to support parallel writes in exchange for later combining.
    - Might not be useful – parallel writes already available on SSDs?
  - Files themselves consist of blocks from all series that wrote while the file was active.
  - No internal indexing.

#### The dream

- No WAL – writes are persistent by default, no write amplification.
- No compaction – no wasted disk throughput.

WiscKey 'solves' write amplification by keeping relatively little data in the LSM-tree – just the keys.
Values are written once only to a value log, and maintenance of the log involves cheap page-bounded truncation.

So far we've been thinking of the timestamp as the key, and whilst timestamps are pretty small the keys would be expect to grow continuously forever, thereby creating quite a lot of work for compaction.
Since we would never actually want to look up a sample for a specific timestamp, we should perhaps change our mental model.

What if we think of the entire series as the 'value', and series' labels as keys pointing to that value?
This matches the WiscKey approach much better, the label space is much smaller than the timestamp space (particularly under shorter retention policies).
What would the concrete 'value' be that we write to the value log?
WiscKey is designed for a value that is an offset into the value log.
We couldn't make the offset point to an entire series since we would not know a-priori how many values the series would have (it may indeed keep growing forever).
Thus, we probably need the value to be a book-keeping structure of some kind, which then points to the actual series data.

Let's consider a database instruction `write(labels, timestamp, value)`, a naive implementation might look like:

- Write the new `(timestamp, value)` somewhere and get a reference to it.
- Read the book-keeping structure for `labels`.
- Update the structure to include a pointer to the new value.
- Write back the book-keeping structure.

This could be made a bit more performance with some considerations such as:

- Pre-allocate 'chunks' of series data in the value log.
- Have the `labels` key point at a 'head' chunk.
- Keep head chunks around in memory (in memory maps? there could be a lot...).
- Write samples directly to the head chunk.
- If the head chunk becomes full, only then allocate a new head chunk and update the value pointer.
- When opening a head chunk, we would need to write a bit of a header pointing to the previous chunk.
  We could imagine leaving space in the header to write the max timestamp at some point, making it easier to identify relevant chunks.

We might be able to adapt the WiscKey approach to use a compound address rather than a single offset into a single log file, e.g. use some number of bits to identify a chunk file, and some more bits to identify the offset in the file for the head chunk.

One axis along which we'll need to make trade-offs is restore-performance vs. storage redundancy.
E.g., if chunks where stored on disk as, essentially, `[(timestamp, value); N]` we might have to scan the whole chunk on start-up to work things out.
We might alternatively write a header/footer to each file with things like the min/max timestamp, offsets to different chunks, etc.

Can we pseudocode this?
Let's start with something naive:

```rust
/// A chunk address.
///
/// Assume there is some black magic that can translate this to a file and offset into the file.
type ChunkAddr = u64;

/// A chunk header.
struct ChunkHeader {
    /// The address of the previous chunk.
    ///
    /// If this is `None` then it's the first chunk of the series.
    prev_chunk: Option<ChunkAddr>,

    /// The minimum timestamp that appears in this chunk.
    ///
    /// This is used to avoid scanning chunks if they're out of range.
    min_timestamp: u64,

    /// The maximum timestamp that appears in this chunk.
    ///
    /// This is used to avoid scanning chunks if they're out of range. If the value is `None` then
    /// this is the head chunk for the series, and we don't know what the max timestamp is yet.
    max_timestamp: Option<u64>,

    /// The number of bytes that are occupied in the chunk data.
    ///
    /// This assumes that chunks are fixed-size whilst samples might be variable-length-encoded to
    /// save space. Therefore there might be some bytes at the end into which the next sample
    /// wouldn't fit.
    valid_len: u64,
}

/// A single sample in a series.
///
/// For now we assume samples decompress into a pair of 8-byte values.
struct Sample {
    /// The timestamp at which the sample was recorded.
    ///
    /// This can store `18,446,744,073,709,551,615` values. If those values were seconds that would
    /// be around 584 billion years. So, milliseconds would be 584 million years, microseconds
    /// would be 584 thousand years, or nanoseconds would be 584 years. This is way more than we
    /// need (both in time range and precision), but `u32` can only store `4,294,967,296` values
    /// which is just 136 years in seconds. We could probably get away with that time range but more
    /// precision would be desireable to order samples that occur within the same second.
    timestamp: u64,

    /// The value of the sample.
    ///
    /// This is assumed to be an arbitrary 8 bytes that the application knows how to handle. For
    /// metrics this would probably be an `f64`, for logs it would be an address for the log value
    /// (which would need its own black magic, like `ChunkAddr`).
    value: u64,
}

/// A chunk of samples from a single series.
struct Chunk {
    /// The chunk header.
    header: ChunkHeader,

    /// Samples from a single series.
    samples: Vec<Sample>,
}
```

We've not covered any index structures yet.
What we have so far is conceptually a bit like:

```rust
type Series = LinkedList<(u64, Option<u64>, Vec<(u64, u64)>)>;
```

E.g., a `LinkedList` of nodes with a `min_timestamp: u64`, `max_timestamp: Option<u64>`, and a vector of samples.
This is alright for us since we can cheaply add chunks to the beginning, and we can linearly scan the chunks whilst skipping the samples for any that fall out of the time range.

What we can't do with this kind of structure is any fancy searching, such as binary search.
We also don't get any lookup benefits from any tree-like structures.
As an optimisation, we could imagine laying out something like a `Vec<(u64, u64, ChunkAddr)>` in memory so that we could binary search chunks by timestamp, but that might not actually be very useful.

#### Indexing

Most of our investigating so far has focused on various key-value stores, but what actually *are* the keys and values in our system?
We've already talked about how the series themselves are better imagined as a linked list of contiguous chunks of samples.
Perhaps key-value stores are a better fit for the index?

Well, the series themselves are identified by their labels.
What are labels?
They are a distinct set of keys and their associated values, essentially:

```rust
type Labels = BTreeMap<String, String>;
// or
type Labels = HashMap<String, String>;
```

So perhaps simply the keys are label keys and the values are label values?
Probably not, since we need to store many distinct sets of labels.
Also, none of the key-value operations are especially applicable to labels – we would never 'insert' a particular label key-value, or update a value for a key, we would always store an entirely new set of labels.
Hence, it would really be the entire `Labels` map that would be the key, something like:

```rust
type Index = BTreeMap<Labels, Chunk>;
```

That's only half the story, however.
An index like that would help us when writing – given some `Labels` and a sample, we can quickly locate the head chunk for the series with those labels.
But what about reading?

Let's start by considering an open wildcard query – e.g. scan all the samples.
For this we are well-placed to simple iterate the values of the index, iterate through the chunks, and scan the samples for each chunk, something like:

```rust
for head in index.values() {
    for chunk in head.chunks() {
        for sample in chunk {
            println!("hello!");
        }
    }
}
```

This is clearly `O(LMN)`, where `L` is the number of entries in the index, `M` is the greatest number of chunks in any series, and `N` is the greatest number of samples in any chunk.
In terms of I/O it might not be too horrible, since we'd expect the samples themselves to be contiguous, but there would be a lot of seeking between chunks (`O(LM)` seeks).

Anyway, we might expect scanning all samples for all time to be quite expensive.
What about something marginally more realistic, like looking at all metrics for the past hour?
Assuming a query structure like:

```rust
struct Query<R: RangeBounds<u64>> {
    timestamp: R,
}

let last_hour = Query {
    timestamp: UNIX_EPOCH.elapsed()?.as_millis().try_into()?..
};
```

We could use `min_timestamp` and `max_timestamp` information on `Chunk` and the sample's timestamps to do something like:

```rust
for head in index.values() {
    for chunk in head.chunks() {
        if !chunk.timestamp_overlaps(&query.timestamp) {
            break;
        }
        for sample in chunk {
            if sample.timestamp < query.timestamp.start_bound() {
                break;
            }
            if sample.timestamp < query.timestamp.end_bound() {
                println!("hello!");
            }
        }
    }
}
```

This doesn't change the theoretical performance really, but we could imagine every head chunk containing the last hour of samples, in which case `M = 1` and the time complexity becomes `O(LN)`.
We would also not have to visit every sample in the head chunk (this assumes we're iterating them in descending order of timestamp).

But what is more interesting than time-bounded queries are label-based queries.
Let's start by considering a single `key=value`, like `namespace=kube-system`.
We'll alter our hypothetical `Query` struct:

```rust
struct Query {
    label: (String, String),
}
```

Now, with the current index structure we have no choice but to scan every entry in the index:

```rust
for (labels, head) in &index {
    if labels.get(&query.label.0) != Some(&query.labels.1) {
        continue;
    }
    for chunk in head.chunks() {
        for sample in chunk {
            println!("hello!");
        }
    }
}
```

So this has time complexity `O(LMN)`, although we would expect `MN` to be adjusted down depending on how common the label is (`namespace=kube-system` would probably be quite common!).

We could introduce a secondary index, something like:

```rust
type LabelIndex = HashMap<(String, String), HashSet<Rc<Labels>>>;
```

If we had such a structure though our query would look something like:

```rust
if let Some(label_set) = label_index.get(&query.label) {
    for labels in label_set {
        if let Some(head) = index.get(labels) {
            for chunk in head.chunks() {
                for sample in chunk {
                    println!("hello!");
                }
            }
        }
    }
}
```

This looks a bit more complicated, but we can now find exactly the labels that contain the queried label and only scan their series.
We could make the `LabelIndex` map from `(String, String)` to `HashSet<Rc<Chunk>>` (e.g.), but that would create more book-keeping when pushing a new head chunk for a series.

Putting this all together we might have the following indexing types:

```rust
/// Labels identifying a series.
///
/// We use `Rc` so that labels can be used in multiple index structures without cloning or explicit
/// hashing.
type Labels = Rc<BTreeMap<String, String>>;

/// An index from `Labels` to head `Chunk`s.
type SeriesHeads = BTreeMap<Labels, Chunk>;

/// An index from label pairs to a set of `Labels` containing those pairs.
///
/// It would be an invariant of our database that the `Labels` in this structure also appear in
/// `SeriesHeads`.
type LabelIndex = BTreeMap<(String, String), BTreeSet<Labels>>;
```

And what about persistence?
Should `SeriesHeads` and `LabelIndex` be persisted directly into a key-value database?
That might not work very well, due to `Labels` appearing in keys and values.
For persistence we would probably want a flattened structure.
In fact, we're getting close to reinventing something like Promtheus' TSDB index structure, which includes something like a `Vec<Labels, ChunkAddr>`, a `Vec<Vec<ChunkAddr>>`, and `Vec<(String, String, u64)>` (where the last structure is an index into the penultimate structure).

## What year is it?

That's another month lost to the database rabbit hole.
As well as the above, I started a small spike to create a kind of page-aligned time-series buffer, but it took longer than anticipated to get very far and it's probably a misuse/misunderstanding of why/how page-alignment affects performance.
The spike used [`rkyv`](https://docs.rs/rkyv/0.4.2/rkyv/index.html) for (de)serialization, which was sadly a bit awkward due to the trait- and generic-heavy API – but it did work.

The spike consisted roughly of the following API:

```rust
const PAGE_SIZE: usize = 4096;

struct SeriesId(NonZeroU64);

struct Sample {
    timestamp: NonZeroU64,
    data: [u8; 8],
}

struct ChunkMut {
    file: File,
    head_page: PageMut,
    open_pages: HashMap<SeriesId, PageMut>,
    buf: Box<[u8; PAGE_SIZE]>
}

impl ChunkMut {
    fn create(path: impl AsRef<Path>) -> io::Result<Self> {
        todo!()
    }

    fn open(path: impl AsRef<Path>) -> Result<Self, OpenError> {
        todo!()
    }

    fn push<'s, S>(
        &mut self,
        series_id: &SeriesId,
        samples: &'s [S],
    ) -> io::Result<Result<(), &'s [S]>>
    where
        S: Borrow<Sample>,
    {
        todo!()
    }
}
```

There were a couple of key ideas:

- The `ChunkMut` was intended to be fully persistent (e.g. no WAL needed).
- The `ChunkMut` was intended to be sealed into a read-only `Chunk` once full.
  `Chunk` was not part of the spike, but it was expected that they could be `mmap`'d which was hoped to work well with the page-aligned data layout (though page access would probably look random).
- The page-based layout was intended as a compromise to allow `Chunk` and `ChunkMut` to have the same layout whilst still supporting partially contiguous reads.
- The thought occurred that runtime statistics could be used to allocate additional contiguous pages for 'hot' series to mitigate otherwise observationally random reads.
- The pages themselves used null-/boundary-terminated value sequences.
  Since both `SeriesId` and `Sample` begin with a `NonZeroU64`, we can deserialize a page until encountering a `0_u64` and then stop.
- Despite all the thinking about performance, it got nowhere near complete enough to measure.

Some things were learned:

- Code starts getting big once you're working with low-level details like data layout and fine-grained persistence.
- On the flip side, it doesn't seem like changing data layout would necessarily require massive API changes in most cases, so it reinforces the idea that we should make something measurable then optimise it (!).

At the same time, I've been watching the [Influx DB IOx](https://github.com/influxdata/influxdb_iox/) tech talks and learning more about [Apache Arrow](https://arrow.apache.org/) on which Influx DB IOx is being built.
Apache Arrow includes "a language-independent columnar memory format" and a suite of libraries in various languages that support computation on the in-memory format.
The Rust [`arrow` crate](https://docs.rs/arrow/3.0.0/arrow/) seems like a pretty solid, and there's also a [`parquet` crate](https://docs.rs/parquet/3.0.0/parquet/) for reading and writing data to Apache Parquet files.
Finally, there's the [datafusion](https://github.com/apache/arrow/tree/master/rust/datafusion) project, part of the Rust Arrow libraries, which is a query engine that works natively with Apache Arrow and Parquet.
The InfluxDB IOx team are betting on these libraries because they solve a lot of the 'boring complexity' of building a columnar database system (data layout, querying).

Whilst IOx is a pretty feature-complete DBMS, including its own server and API components, it does seem like the same triumvirate could be used to build an embedded database.
This approach would rule out some of the more aggressive optimisations when working with pure `(u64, f64)` samples, but that could already have proven problematic for our use-case since we want to store logs as well.

At any rate, this should move to a future post.
