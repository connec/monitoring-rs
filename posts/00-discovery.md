# Discovery

So, you want to build a monitoring pipeline?
First, let's define what we mean by that.

## What is a monitoring pipeline?

Sadly, [Googling](https://www.google.com/search?q=What+is+a+monitoring+pipeline%3F) the above query mostly gives me results for oil & gas pipeline monitoring... which is definitely not what we mean.
Asking [What is a logging pipeline?](https://www.google.com/search?q=What+is+a+logging+pipeline%3F) gives better results, including a few interesting blog posts and more than a few salesy articles from vendors.
Many of them feature flow diagrams containing several non-trivial applications ([Apache Kafka](https://kafka.apache.org/) shows up a lot).
This is not a good start for our purposes.

One of the results is refreshingly down-to-earth: [So you need a logging pipeline](https://darakian.github.io/2019/09/14/so-you-need-a-logging-pipeline.html).
In the post, the author talks about [MozDef](https://github.com/mozilla/MozDef) and frames their requirements as compatible with the features of "[SIEM](https://en.wikipedia.org/wiki/Security_information_and_event_management)" (Security information and event management) – a term coined by Gartner, and as such one to avoid if we hope to remain grounded in reality.

The post is worth a read by itself, and touches on some points that are useful for our discovery:

- **[Application Logging vs System Logging](https://darakian.github.io/2019/09/14/so-you-need-a-logging-pipeline.html#application-logging-vs-system-logging)**\
  It's not only the logs from the application that we want to collect, we also want the logs from the system the application is running on.
  In VM-oriented architectures, this would include logs from the OS and system daemons on which the application depends.
  Is this distinction still useful in container-oriented platforms like Kubernetes?

- **[Transport options](https://darakian.github.io/2019/09/14/so-you-need-a-logging-pipeline.html#transport-options)**\
  The author recommends JSON over HTTPS on the grounds that it's flexible, unlikely to be interfered with by firewalls, and is easy to process and emit.
  Whilst firewalls are entirely under our control in a Kubernetes context, the general advantages of flexibility and convenience are compelling.
  That said, something like [gRPC](https://grpc.io/) might offer better efficiency.

- **[Mozilla’s Marvellous MozDef](https://darakian.github.io/2019/09/14/so-you-need-a-logging-pipeline.html#mozillas-marvellous-mozdef)**\
  [MozDef](https://mozdef.readthedocs.io/en/latest/overview.html) (The Mozilla Defense Platform) describes itself as "a set of micro-services you can use as an open source Security Information and Event Management (SIEM) overlay on top of Elasticsearch".
  Whilst the goals are admirable, the scope and architecture definitely exceeds what we're looking for from a *minimal* monitoring pipeline.
  That said, their interpretation of the SIEM functionality sounds useful for us:

  - "Accepting events/logs from a variety of systems." We want to be conservative with *variety*, but otherwise this is clearly in our requirements.
  - "Storing events/logs." Yup.
  - "Facilitating searches." Check.
  - "Facilitating alerting." Pertinent.
  - "Facilitating log management (archiving, restoration)." Anything non-trivial in this category probably goes beyond our requirements, but a mechanism for exporting logs in a reasonable format may be desireable.

  The features go on to include a variety of integrations and plug-ins, all of which are explicitly out of scope for us.

### Our requirements

In fact, the MozDef descriptions of functionality feel like a good summary of the high level requirements we might want from a monitoring pipeline.
In our own words:

- Accept logs/metrics from Kubernetes.
- Store those logs/metrics in a format suitable for searching and alerting.
- Provide an API and UI for searching and visualising ingested logs/metrics.
- Provide an API and UI for configuring alert rules based on incoming logs/metrics.

Additionally, while it's not in the core requirements, we should try not to make decisions that would prohibit future implementation of data management features such as retention, import/export, and forwarding.

## "Accept logs/metrics from Kubernetes"?

That first requirement is still a bit too vague for my tastes.
What does it mean to accept logs/metrics from Kubernetes?
How does Kubernetes emit logs/metrics?
What format are they in?
Predictably, the Kubernetes documentation has our backs with a good page on [Logging Architecture](https://kubernetes.io/docs/concepts/cluster-administration/logging/).
Some salient points:

- Kubernetes built-in logging functionality is based on the functionality provided by the container engine.
  Container engines redirect the `stdout` and `stderr` of the containers they run, and on Kubernetes that redirection is to a file on the host node.

- Kubernetes is not technically responsible for performing log rotation, and a separate solution should be configured when a node is deployed.
  Since the environment we're interested in is an already up-and-running cluster, we can assume log files are being rotated.

- The [`kubectl logs`](https://kubernetes.io/docs/reference/generated/kubectl/kubectl-commands#logs) command essentially reads directly from the latest log file and streams it back to the client (TIL).

- Kubernetes *does* distinguish between normal container logs and "system component" logs.
  System components that do not run in a container (kubelet and the container runtime itself) write either to [systemd-journald](https://www.freedesktop.org/software/systemd/man/systemd-journald.service.html), or directly to `.log` files in `/var/log` if the node does not have systemd.
  System components that do run inside a container bypass the container runtime's log handling and log directly to `/var/log`.
  Since systemd-journald *also* writes to `/var/log`, ultimately all the system component logs will end up there, though possibly in different formats.

- "Cluster-level logging" is the Kubernetes term for separating the storage and lifecycle of logs from that of nodes, pods, and containers in order for logs to remain accessible as the cluster scales, workloads move, and log files rotate.
  This is one of the main things we want from our monitoring pipeline.

- Three architectures are discussed for cluster-level logging:

  - [Using a node logging agent](https://kubernetes.io/docs/concepts/cluster-administration/logging/#using-a-node-logging-agent).
    In this architecture, a logging agent is deployed on each node in the cluster by means of a `DaemonSet`, and exposes or pushes the logs on the node to a back-end.
    This is the encouraged approach as it has a light footprint (one agent per node) and requires no changes to applications – so long as the application uses `stdout` and `stderr` for logging.
    This is consistent with the [twelve-factor methodology](https://12factor.net/) (for what that's worth), and is the general convention for containerised applications.

  - [Using a sidecar container with the logging agent](https://kubernetes.io/docs/concepts/cluster-administration/logging/#using-a-sidecar-container-with-the-logging-agent).
    In this architecture, a logging agent is deployed with each pod as a 'sidecar' container, and is responsible for exposing or pushing logs from the application to a back-end.
    This approach offers more flexibility and supports application log outputs besides `stdout`/`stderr`.
    However, it requires the sidecar container to be configured or injected for every pod, leading to a heavier footprint and burden on application authors (or additional weight in the form of a controller to automatically inject the logging agent).

  - [Exposing logs directly from the application](https://kubernetes.io/docs/concepts/cluster-administration/logging/#exposing-logs-directly-from-the-application).
    In this architecture, application containers are directly responsible for exposing or pushing their logs to a back-end.
    This approach offers maximum flexibility at the cost of log management fully infecting the application (see the configuration guide for any Java logging framework for why we don't want this).

  Given our goals of **incredibly lightweight** and **trivial to deploy**, the first approach (using a node logging agent) is the obvious choice.

This is good stuff!
The following more detailed requirements now seem pretty clear:

- One component of our monitoring pipeline will be a logging agent.
- The logging agent will be deployed as a `DaemonSet`.
- The logging agent will need to discover log files in `/var/log` on the host, parse them appropriately, and ship them off to a back-end.

There's still a small bombshell in there though...

### "Parse them appropriately"?

Alas, this explains some of the complex configuration available in logging agents: parsing entries from log files.
A naive implementation, such as that taken by [Docker's JSON file logging driver](https://docs.docker.com/config/containers/logging/json-file/), would simply treat each line in the file as a separate event.
Whilst this works a lot of the time (arguably all the time if you define an event as a line in the input...), it fails in the face of some common situations:

- Human-readable output.
  It's fairly common to render structured data across multiple lines for human consumption (e.g. the `Debug` format traits in Rust).
  Whilst it would be possible for the application to detect when it's running in a non-interactive context and compact the output, having to think about that at all is a bit of a bind.

- Stack traces.
  Arguably a specific instance of "human-readable output", stack traces from programming languages are almost always formatted across multiple lines.
  Furthermore, in many languages it isn't possible to 'catch' all possible stack traces before they're rendered (e.g. internal runtime exceptions), meaning compacting to a single line may not be possible.

Existing logging agents have various solutions to this:

- Fluent Bit supports [multiline configuration parameters](https://docs.fluentbit.io/manual/pipeline/inputs/tail#multiline).
- Fluentd features a [`multiline` parser](https://docs.fluentd.org/parser/multiline).
- Logstash features a [multiline codec plugin](https://www.elastic.co/guide/en/logstash/current/plugins-codecs-multiline.html).
- Vector supports [multi-line messages](https://vector.dev/docs/reference/sources/file/#multi-line-messages).
- ...etc.

This seems to be a well understood area, but disappointingly it means that some applications may wish to tailor how their logs are processed in order to improve their appearance in later analyses.
Could we avoid introducing configuration into our logging agent by requiring applications with these requirements to run a sidecar container to post-process the application's logs?
Sadly not, as the application would still write its unmodified logs to `stdout`/`stderr`, meaning the sidecar would merely duplicate the logs in most cases unless either:

- The application was modified to log somewhere else.
  This feels in conflict with our **trivial to deploy** goal.
- The original application's logs were filtered from the log agent.
  This would just be configuration of a different kind (or at a different layer), so it doesn't avoid the problem.

As such, we may have to concede that some amount of configuration for how log files are processed would be required in order to support multi-line log events.
Hopefully we can keep this minimal and simple (of the above, Vector's and Logstash's approaches seem the most flexible and direct).
Perhaps a solution could be to allow the logging agent to discover configuration from `ConfigMap`s?
TBD!

### "Accept logs/*metrics* from Kubernetes"?

The Kubernetes docs also have a page on [Metrics For Kubernetes System Components](https://kubernetes.io/docs/concepts/cluster-administration/system-metrics/).
The highlights of this are:

- Kubernetes system components emit metrics in [Prometheus' text-based format](https://prometheus.io/docs/instrumenting/exposition_formats/#text-format-example).

- Metrics are typically exposed at `/metrics` on the component's HTTP server.
  The `kubelet` system component exposes additional metrics at `/metrics/cadvisor`, `/metrics/resource`, and `/metrics/probes`.
  The metrics endpoints are subject to Kubernetes RBAC.

- Kubernetes does not itself maintain a history of metric values.
  They recommend scraping the metrics periodically, and storing them in a time-series database.

- Kubernetes system metrics follow a well-defined lifecycle: Alpha → Stable → Deprecated → Hidden → Deleted.
  The meanings are typical:

  - **Alpha** metrics have no stability guarantees at all, and may be modified or removed at any time.
  - **Stable** metrics are guaranteed not to be deleted, renamed, or have their data types changed.
  - **Deprecated** metrics have been scheduled for eventual deletion.
    The metric's [help text](https://prometheus.io/docs/instrumenting/exposition_formats/#comments-help-text-and-type-information) will say since which version the metric is deprecated.
  - **Hidden** metrics are no longer published by default, but can still be shown using additional configuration.
    Metrics are hidden one minor version after they are deprecated.
  - **Deleted** metrics are no longer published, and this cannot be configured – the code to collect and publish them has probably been deleted.
    Metrics are deleted two minor versions after they are deprecated.

  This formalisation of system metrics appears to be a very recent thing.
  Notably, the document only exists for the latest Kubernetes version (v1.19), and the linked [stable Kubernetes metrics](https://github.com/kubernetes/kubernetes/blob/master/test/instrumentation/testdata/stable-metrics-list.yaml) list is empty!

Whilst this only describes Kubernetes' approach to publishing system metrics, the same pattern has become prevalent in the ecosystem as a whole, and many Kubernetes applications can be configured to export metrics in the same way.
Since we're aiming to create a lightweight, opinionated monitoring pipeline, for our purposes this is sufficient to add some requirements for how we will handle metrics:

- One component of our monitoring pipeline will be a metrics scraper.
- The metrics scraper will periodically perform HTTP requests to known metrics endpoints, parse the response using Prometheus' text-based format, and send the parsed metrics to a back-end.

### Oh metrics, where art thou?

How will our metrics scraper know from whence to scrape metrics?
Here lies the raison d'être for projects such as [Prometheus Operator](https://github.com/prometheus-operator/prometheus-operator), or Prometheus' built-in [Kubernetes service discovery configuration](https://prometheus.io/docs/prometheus/latest/configuration/configuration/#kubernetes_sd_config).
In abstract, the metrics scraper needs to be able to discover scrape targets from the cluster configuration – either based on custom resources (like Prometheus Operator), resource annotations (like Prometheus' Kubernetes service discovery), or something else like a `ConfigMap`.
This discovery could be performed by the metrics scraper itself, or by a separate component that then updates the configuration of the metrics scraper.

As for logging, we will leave this TBD for now and revisit once we have a more complete picture of our requirements.

## "Store those logs/metrics"

Our requirements for a logging agent and metrics scraper both include sending the data they parse to a back-end for storage and retrieval.
Let's summarise the characteristics of the incoming data and retrieval modes for both logs and metrics, and see what jumps out.

### Those logs

#### Storage

Log entries could be thought of simply as lines of plain text, but the Kubernetes ecosystem is very fond of structured logging, where log entries are considered to be structured events rather than plain text.
In the context of a monitoring pipeline, it's usually the case that *all* logs are structured log events, since the logging agent would report discovered log entries along with metadata such as the source of the entry and the date/time at which the entry was discovered.
In that view, a plain text log entry would likely be recorded in a `message` field of a structured log event, alongside the source and date/time:

```
[2020-09-20T12:59:32Z] Server started
```

Would become:

```json
{
  "timestamp": "2020-09-20T12:59:35Z",
  "source": "/var/log/containers/...",
  "message": "[2020-09-20T12:59:32Z] Server started"
}
```

It would be the job of the logging agent to perform this conversion, as well as possibly adding additional metadata, and/or merging structured data from the log entry itself for applications that write structured log events.

With that, we have the following characteristics for log data that are pertinent how logs are stored:

- The data is structured and may contain a variety of data types.
- The data is schema-less – the structure of events logged by applications cannot be known in advance.
- The data may contain sizeable plain text values, such as unstructured log lines or stack traces.
- The data is fundamentally append-only.

#### Retrieval

Lets focus on three plausible log retrieval use-cases:

1. Following the live logs for a specific service or component in order to debug it or otherwise observe its behaviour.
  For this use-case, latency and specificity are key – obtaining the latest log entries based on precise criteria needs to be fast.
  Latency is not just a retrieval issue, but the retrieval performance would affect the overall latency.
  Reliably filtering logs to those from a specific source would require field-based filtering, rather than text-based filtering which may lead to false positives.

1. Retrieving logs with potentially vague criteria in order to understand an alert with limited context – perhaps only the service name and a rough timeframe, or even just a string of text from an error message.
  For this use-case, query flexibility and plain-text search are important.
  The known criteria should be combineable into a single query which can be further refined to exclude noise and hone in on relevant entries.

1. Tracing activity through multiple services based on structured log data (e.g. a transaction ID or user ID).
  For this use-case, it's important that there are minimal or no required constraints when searching logs.
  Relevant logs may be from arbitrary time periods and sources, and all must be retrieved in order to offer a complete answer to the query.

#### Requirements

Based on the above data characteristics and retrieval modes, we can summarise that a storage system for logging would ideally have the following characteristics:

- Support for storing and querying structured documents, with efficient text storage.
- Support for complex queries, including conjunction ("and"), disjunction ("or"), negation ("not"), and free-text search.
- Support for cross-partition queries.
- Optimised for append-only writes.
- Optimised for time-ordered, filtered reads with simple (`key=value`) criteria.
- Reasonably performant for complex or cross-partition queries.
- Support for reasonable retention/archiving strategies, due to append-only operation.

Ooft.
This seems quite idealistic.
Let's continue to look at metric storage then consolidate.

### Those metrics

#### Storage

Based on our requirements so far, we would have to periodically poll a bunch of HTTP endpoints, each of which would tell us about some metrics (potentially thousands), and the current value(s) thereof.
The purpose of this scraping is to persist the history of metrics over time.
The main considerations this gives us for our metrics storage are:

- The write load will be quite heavy and 'bursty', though often the only new data would be the latest metric value.
- By volume, the data would mostly be metadata (metric name and labels).
  The value is nominal in comparison.
- The data is append-only.

#### Retrieval

The value of storing a history of metric values comes from the ability to query and visualise them in order to understand how a system is performing over time.
As with logging, let's pick a few specific use-cases to frame this:

1. Generate a graph of a metrics over time to visualise whether recent changes have had the desired effects.
  For this use-case, performant retrieval and aggregation of past values of specific metrics is important.
  It may also be necessary to filter metrics to particular dimensions to get the graph you want, so this needs to be be performant as well.

1. Constantly monitor critical system metrics in order to quickly detect unplanned service disruption.
  For this use-case, being able to repeatedly sample recent metrics without impacting concurrent metric ingestion or other monitoring queries is vitaly important.
  This might need to scale to hundreds or more constantly evaluated queries/thresholds.

1. Interactively query and visualise multiple metrics across multiple dimensions in order to debug a performance bug.
  For this use-case, the dimensionality of metrics is important, along with the discoverability of metrics and their dimensions.
  It needs to be possible to filter metrics by complex queries on their dimensions in order to narrow in on meaningful metrics about only the affected components.

#### Requirements

Based on the above data characteristics and retrieval modes, we can summarise that a storage system for metrics would ideally have the following characteristics:

- Support for storing and querying metrics: data points with a name and multiple labels/dimensions.
- Support for complex queries (conjunction, disjunction, negation) based on metric names and labels.
- Support for aggregation and sampling in queries.
- Optimised for high-volume, append-only writes of data points.
- Heavily optimised for retrieving the latest values for a query.
- Reasonably performant for retrieving the history of values for a query.
- Support for reasonable retention/archiving strategies, due to append-only operation.

### Consolidation

The requirements for storing logs and metrics have some similarities.
In fact, let's try to unify them and see where we land.

Let's imagine both log and metrics are stored as JSON documents.

- Log entries could look like:

  ```
  {
    "timestamp": "2020-09-24T19:15:32Z",
    "message": {...} | "..."
  }
  ```

- Metrics could look like:

  ```
  {
    "timestamp": "2020-09-24T19:15:32Z",
    "name": "nginx_requests_per_second",
    "value": 34,
    "labels": {...}
  }
  ```

We can frame our requirements for a common data store thus:

- Support for storing and querying JSON documents.
- Support for complex JSON queries, including specific `key <op> value`, free-text search of sub-trees, conjunction, disjunction, and negation of expressions.
- Support for aggregation and sampling in queries.
  This is mostly relevant to metrics, but being able to retrieve specific fields or sample values from logs may also be useful.
- Optimised for high-volume append-only writes, ideally deduplicating key information.
  Deduplication would have the greatest value for metrics, where most of the document is keys.
- Optimised for retrieving ordered entries, with the most recent being the most performant.
- Support for retention/archiving strategies.

This sounds like some kind of time-series database, with additional support for sub-tree free-text search.
I wonder if that's even sensible?

## More things to discover...

We could keep going into the following areas:

- Specific API details for storage, retrieval, alerting.
- Visualisation of logs/metrics.
- Configuration approaches.

I'm a bit burned out on this now, however, and would like to write some code.
Before getting into it though, I'm going to go through a [Writing your own time-series database](http://inkblotsoftware.com/articles/writing-your-own-time-series-database/) tutorial to check how insane this could be.

[Back to the README](../README.md#posts)
