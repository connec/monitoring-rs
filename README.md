# monitoring-rs

An adventure in building a minimal monitoring pipeline, in Rust.

## Preface

(Or jump to [Posts](#posts).)

### Why?

Because I want a "minimal productive Kubernetes cluster", and existing logging machinery feels too heavy and complicated to satisfy the "minimal" part of that.

To wit, a de-facto monitoring pipeline in a production Kubernetes cluster today might include:

- [Fluent Bit](https://fluentbit.io/) for log collection and forwarding to...
- [Elasticsearch](https://www.elastic.co/elasticsearch/) for storage and performing queries made by...
- [Kibana](https://www.elastic.co/kibana) for visualisation and analysis.

*Plus:*

- [Prometheus](https://prometheus.io/) for metric collection, storage, alerts, and performing queries made by...
- [Grafana](https://grafana.com/grafana/) for visualisation and analysis.

Five distinct services, from multiple vendors, each of which has multiple possible approaches to installation and configuration, as well as their own configuration languages and operational gotchas.
That seems excessive, when all I want is to see some metrics and logs from a handful of apps.

Whilst these satisfy the good architectural practices of single responsibility and composability (most of those components can be swapped with alternatives), it makes setting up a "minimal" monitoring pipeline feel like a significant investment.
This repository is an ongoing experiment to see if something more minimal and integrated is possible and compelling, and if not to understand *why*.

### What?

There must be a reason the current landscape is so bloated.
Presumably, each possible solution for collection, aggregation, and visualisation offers something unique and valuable that the others don't?
Or, perhaps, there's so much churn in the infrastructure space these days that the best bet is to build generic solutions that can integrate with other generic solutions?

Frankly, I don't care.
I just want something that's:

- **Kubernetes only.**
  It wouldn't be a failure if it happened to work on other platforms, but only if that doesn't mean extra features and extra bloat.

- **Incredibly lightweight.**
  I don't want to pay anything for my monitoring pipeline, but nothing is free, so I'll settle for minimal CPU, memory, and storage, with no dependence on additional infrastructure.
  Rust should help here.

- **Trivial to deploy.**
  I don't want to have to think about setting up a monitoring pipeline, I just want my apps to be monitored.
  I also don't want to have to read through 1,000s of Helm chart knobs and dials to know what is *actually* being deployed, or whether it's *really* doing what I want.

- **Lowest common denominator configuration.**
  Today's deployment automation landscape is a nightmareish hellscape of convoluted and derivative configuration languages and formats.
  Send help!
  In the meantime, since this is Kubernetes-oriented, well-specified YAML configuration formats and Kubernetes CRDs, will hopefully offer a familiar and approachable interface.

- **Reliable *enough*.**
  An unrelenting focus on reliability can be the enemy of minimalism and simplicity.

- **Invented here.**
  I'll admit it – this is mostly fueled by hubris.
  Surely the massive communities around the above tools have it all wrong, and if only their vision was as clear as mine they could see there was a Better Way!
  Or, more probably, I'll learn why things are the way they are by attempting to reinvent the wheel.

These requirements are likely to lead to something that's strongly opinionated, a bit inflexible, and probably not appropriate if you have your own opinions on, or requirements for, how logging and monitoring should work.
Hopefully, however, it will be ideal if you just want your apps to be monitored and want to think as little as possible about what that means and how to do it.


### How?

Good question.
This will probably change, but a reasonable order of priorities could look like:

1. Some discovery work about how logs and metrics are handled by Kubernetes out of the box, in order to understand the minimal integration that could collect them.

1. Create a Rust project, potentially with multiple binaries, to serve as the agent(s) for collection and querying.

1. Create a web UI (in Rust? Elm?) that can perform simple queries and visualisation.

I'm taking bets (with myself I guess, since this is a private repo) on which bullet at which I will give up and use [loki-stack](https://github.com/grafana/loki/tree/master/production/helm/loki-stack)... My money is on 1.

## Posts

- [Discovery](posts/0-discovery.md)
- [Log collection part 1](posts/1-log-collection-part-1.md)
