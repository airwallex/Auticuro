---
sidebar_position: 1
---

# Overview
Comprehensive tests are conducted against the **single-shard Auticuro **to assess its correctness, robustness, and performance.

## Chaos Test
Chaos test, also known as fault injection, in which we intentionally introduce failures into a system to evaluate its ability to maintain function and recover gracefully.

The test deployment:
* A 5-replica **Auticuro** k8s stateful set on the GCP
* A request sender that keeps sending requests to the Auticuro @ **200TPS**
* A consistency checker that checks the consistency of the raft/event logs and wallet state machine among the five replicas.

Chaos test scenarios:
* Injecting pod failure in a round-robin way every 10 minutes
* Injecting network loss failure in a round-robin way every 10 minutes

Test Result:
* More than **100M** requests are processed by the Auticuro cluster without duplicate or loss, during which there are more than **1000** times pod/network chaos happened.

## Stress Test
We use 8 vCPUs * 5 node clusters on GCP, which are attached with SSD persistent disks (pd-ssd). 
The **P99 latency <= 20ms** when **TPS = 10K**.

Different workloads are tested, and the detailed latency distribution vs. throughput is shown below:
![image info](@site/static/img/evaluation/latency_distribution.gif)

In summary, Auticuro has proven to be a low-latency, high-performance, and reliable storage engine that suits mission-critical financial applications. We aim to provide an exceptional wallet service solution and contribute to the fintech industry and open-source community by sharing our insights, knowledge, and experiences.

